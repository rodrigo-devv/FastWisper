//! Bloqueio de rede do daemon com Landlock.
//!
//! `IPAddressDeny` na unidade systemd não vale em serviço de **usuário** sem
//! root, então o bloqueio fica no próprio processo: regras de rede do
//! Landlock (ABI 4, Linux 6.7+), `connect` TCP só para a porta do endpoint
//! LLM configurado e nenhum `bind` TCP. Sockets Unix (Hyprland, PipeWire,
//! D-Bus, IPC) não passam por essas regras.
//!
//! Limites do kernel: a regra é por **porta**, não por host (o host fixo
//! continua garantido pelo cliente em `llm.rs`), e UDP não é coberto. Kernel
//! sem suporte não derruba o daemon: o estado vai para o log e o `doctor`.

use std::fmt;

use landlock::{
    ABI, Access, AccessNet, CompatLevel, Compatible, LandlockStatus, NetPort, RestrictSelfAttr,
    Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetError, RulesetStatus,
};
use serde::{Deserialize, Serialize};

use crate::config::{LlmConfig, LlmEngineKind};
use crate::llm::check_endpoint;
use crate::models::is_ollama_cloud_name;

/// Estado do bloqueio de rede do processo, para log, `status` e `doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NetworkSandbox {
    /// TCP só para `connect_ports` (vazio = nenhum); `bind` TCP negado.
    Enforced { abi: u32, connect_ports: Vec<u16> },
    /// Kernel sem Landlock ou sem regras de rede: segue sem bloqueio.
    Unavailable { reason: String },
    /// Erro inesperado ao aplicar: segue sem bloqueio.
    Failed { error: String },
}

impl NetworkSandbox {
    pub fn is_enforced(&self) -> bool {
        matches!(self, NetworkSandbox::Enforced { .. })
    }
}

impl fmt::Display for NetworkSandbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkSandbox::Enforced { abi, connect_ports } if connect_ports.is_empty() => {
                write!(f, "Landlock ativo (ABI {abi}): nenhuma conexão TCP")
            }
            NetworkSandbox::Enforced { abi, connect_ports } => {
                let ports: Vec<String> = connect_ports.iter().map(u16::to_string).collect();
                write!(
                    f,
                    "Landlock ativo (ABI {abi}): TCP só para a porta {}",
                    ports.join(", ")
                )
            }
            NetworkSandbox::Unavailable { reason } => {
                write!(
                    f,
                    "Landlock indisponível ({reason}): daemon sem bloqueio de rede"
                )
            }
            NetworkSandbox::Failed { error } => {
                write!(f, "Landlock falhou ({error}): daemon sem bloqueio de rede")
            }
        }
    }
}

/// Portas TCP que o daemon pode conectar: só a do endpoint LLM que o cliente
/// realmente usaria. LLM desligado, endpoint inválido, remoto sem
/// `remote_consent` ou combinação que o cliente recusa → nenhuma.
pub fn allowed_connect_ports(config: &LlmConfig) -> Vec<u16> {
    if !config.enabled {
        return Vec::new();
    }
    let Ok(endpoint) = check_endpoint(&config.endpoint, config.remote_consent) else {
        return Vec::new();
    };
    if endpoint.remote && !config.remote_consent {
        return Vec::new();
    }
    if config.engine == LlmEngineKind::Ollama
        && (endpoint.remote || is_ollama_cloud_name(config.model.trim()))
    {
        return Vec::new();
    }
    endpoint.url.port_or_known_default().into_iter().collect()
}

/// Restringe **este processo** (todas as threads, quando o kernel tem ABI 8;
/// antes disso, a thread atual e as que ela criar depois): TCP só para
/// `allowed_connect_ports(config)`. Chamar depois de ler a config e antes de
/// criar threads ou aceitar comandos. Não tem volta: trocar o endpoint exige
/// reiniciar o processo. Liga também `no_new_privs`, como o
/// `NoNewPrivileges=true` da unidade.
pub fn restrict_network(config: &LlmConfig) -> NetworkSandbox {
    let ports = allowed_connect_ports(config);
    match apply(&ports) {
        Ok(sandbox) => sandbox,
        Err(e) => NetworkSandbox::Failed {
            error: e.to_string(),
        },
    }
}

fn apply(ports: &[u16]) -> Result<NetworkSandbox, RulesetError> {
    let mut ruleset = Ruleset::default()
        .handle_access(AccessNet::from_all(ABI::V4))?
        .create()?;
    for &port in ports {
        ruleset = ruleset.add_rule(NetPort::new(port, AccessNet::ConnectTcp))?;
    }
    let status = ruleset.all_threads(true)?.restrict_self()?;
    Ok(match (status.ruleset, status.landlock) {
        (RulesetStatus::NotEnforced, landlock) => NetworkSandbox::Unavailable {
            reason: unavailable_reason(landlock),
        },
        (_, landlock) => NetworkSandbox::Enforced {
            abi: abi_number(landlock),
            connect_ports: ports.to_vec(),
        },
    })
}

/// Confere se o kernel aceita regras de rede, sem restringir o processo.
/// Para o `doctor`, que roda fora do daemon.
pub fn probe_kernel() -> Result<(), String> {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessNet::from_all(ABI::V4))
        .and_then(|ruleset| ruleset.create())
        .map(drop)
        .map_err(|e| format!("o kernel não aceita regras de rede do Landlock ({e})"))
}

fn unavailable_reason(landlock: LandlockStatus) -> String {
    match landlock {
        LandlockStatus::NotImplemented => "kernel sem Landlock".into(),
        LandlockStatus::NotEnabled => {
            "Landlock desligado no kernel (fora da lista de LSM ativos)".into()
        }
        other => format!(
            "Landlock ABI {} sem regras de rede; precisa de ABI 4 (Linux 6.7+)",
            abi_number(other)
        ),
    }
}

fn abi_number(landlock: LandlockStatus) -> u32 {
    match landlock {
        LandlockStatus::Available {
            kernel_abi: Some(abi),
            ..
        } => u32::try_from(abi).unwrap_or(0),
        LandlockStatus::Available { effective_abi, .. } => effective_abi as u32,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llm(endpoint: &str, consent: bool) -> LlmConfig {
        LlmConfig {
            endpoint: endpoint.into(),
            remote_consent: consent,
            ..LlmConfig::default()
        }
    }

    #[test]
    fn local_endpoint_allows_only_its_port() {
        assert_eq!(allowed_connect_ports(&LlmConfig::default()), vec![11434]);
        let compat = LlmConfig {
            engine: LlmEngineKind::OpenaiCompatible,
            ..llm("http://[::1]:8080", false)
        };
        assert_eq!(allowed_connect_ports(&compat), vec![8080]);
        assert_eq!(
            allowed_connect_ports(&llm("http://127.0.0.1", false)),
            vec![80]
        );
    }

    #[test]
    fn remote_port_needs_consent_and_compatible_engine() {
        let remote = |consent| LlmConfig {
            engine: LlmEngineKind::OpenaiCompatible,
            ..llm("https://api.example.com", consent)
        };
        assert!(allowed_connect_ports(&remote(false)).is_empty());
        assert_eq!(allowed_connect_ports(&remote(true)), vec![443]);
        // Ollama é só local: o cliente recusa, então a porta também fica fechada.
        assert!(allowed_connect_ports(&llm("https://api.example.com", true)).is_empty());
    }

    #[test]
    fn disabled_or_refused_llm_allows_nothing() {
        let off = LlmConfig {
            enabled: false,
            ..LlmConfig::default()
        };
        assert!(allowed_connect_ports(&off).is_empty());
        assert!(allowed_connect_ports(&llm("localhost:11434", false)).is_empty());
        let cloud = LlmConfig {
            model: "gpt-oss:120b-cloud".into(),
            ..LlmConfig::default()
        };
        assert!(allowed_connect_ports(&cloud).is_empty());
    }

    #[test]
    fn display_is_short_and_in_portuguese() {
        let on = NetworkSandbox::Enforced {
            abi: 8,
            connect_ports: vec![11434],
        };
        assert_eq!(
            on.to_string(),
            "Landlock ativo (ABI 8): TCP só para a porta 11434"
        );
        let json = serde_json::to_value(&on).expect("serializa");
        assert_eq!(json["state"], "enforced");
    }
}
