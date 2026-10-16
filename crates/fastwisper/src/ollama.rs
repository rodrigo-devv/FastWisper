//! `fastwisper ollama`: liga ou desliga a nuvem do Ollama local (modelos
//! cloud, busca web) conforme `llm.ollama_cloud`. Só age quando pedido; o
//! daemon nunca mexe no Ollama.
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::{Args, ValueEnum};
use fw_core::config::{Config, LlmEngineKind};
use fw_core::config_edit;
use fw_core::llm::check_endpoint;
use fw_core::ollama::{CloudStatus, cloud_status};
use toml_edit::Value;

/// Drop-in do systemd com `OLLAMA_NO_CLOUD`; apagar desfaz.
const DROP_IN: &str = "ollama.service.d/fastwisper-cloud.conf";
const SYSTEM_UNITS: &str = "/etc/systemd/system";

#[derive(Debug, Args)]
pub struct OllamaArgs {
    /// offline, online, apply ou models (sem argumento: mostra o estado).
    #[arg(value_parser = crate::localized_enum::<Action>())]
    action: Option<Action>,
    /// Só salva a escolha na config, sem mexer no Ollama.
    #[arg(long)]
    no_apply: bool,
    /// Passos de administrador do serviço do sistema pela janela de senha
    /// do polkit (`pkexec`) em vez do `sudo` no terminal. Sem terminal, é
    /// o caminho usado automaticamente.
    #[arg(long)]
    pkexec: bool,
    /// Estado ou lista de modelos em JSON (para o hub).
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    config: crate::ConfigArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Action {
    /// Desliga a nuvem do Ollama (padrão do FastWisper).
    Offline,
    /// Liga a nuvem do Ollama.
    Online,
    /// Aplica no Ollama o que está na config (usado pelo install.sh).
    // `aplicar`, o nome antigo, continua aceito como apelido (fora da ajuda).
    #[value(alias = "aplicar")]
    Apply,
    /// Modelos instalados no Ollama local (`/api/tags`), sem os de nuvem.
    Models,
}

fn describe_choice(cloud: bool) -> &'static str {
    if cloud {
        "online (llm.ollama_cloud = true)"
    } else {
        "offline (llm.ollama_cloud = false)"
    }
}

/// Cliente do Ollama local: sem proxy nem redirect, timeout curto.
fn local_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// Nomes dos modelos do `/api/tags`, sem os de nuvem (o texto sairia da
/// máquina), em ordem alfabética.
fn parse_tags(body: &str) -> Option<Vec<String>> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let mut names: Vec<String> = json
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(|m| m.get("name").or_else(|| m.get("model"))?.as_str())
        .filter(|name| !fw_core::models::is_ollama_cloud_name(name))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    Some(names)
}

/// `GET /api/tags` do Ollama local.
fn installed_models(endpoint: &str) -> Result<Vec<String>, String> {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let mut res = local_agent()
        .get(&url)
        .call()
        .map_err(|_| "Ollama não respondeu".to_owned())?;
    if !res.status().is_success() {
        return Err(format!("Ollama respondeu HTTP {}", res.status().as_u16()));
    }
    let body = res
        .body_mut()
        .read_to_string()
        .map_err(|_| "resposta ilegível do Ollama".to_owned())?;
    parse_tags(&body).ok_or_else(|| "resposta inesperada de /api/tags".to_owned())
}

/// Como o Ollama desta máquina roda (de onde vem o ambiente dele).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deployment {
    /// `ollama.service` do sistema: drop-in em `/etc`, com sudo.
    System,
    /// `ollama.service` do usuário: drop-in em `~/.config/systemd/user`.
    User,
    /// Outro jeito (`ollama serve`, app): `~/.ollama/server.json`.
    Manual,
}

fn unit_active(user: bool) -> bool {
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    cmd.args(["is-active", "--quiet", "ollama.service"])
        .status()
        .is_ok_and(|s| s.success())
}

fn detect() -> Deployment {
    if unit_active(false) {
        Deployment::System
    } else if unit_active(true) {
        Deployment::User
    } else {
        Deployment::Manual
    }
}

fn drop_in_text() -> &'static str {
    // i18n: dado (comentário do drop-in gerado)
    "# Gerado por `fastwisper ollama offline` (llm.ollama_cloud = false).\n\
     # Apague este arquivo (ou rode `fastwisper ollama online`) para desfazer.\n\
     [Service]\n\
     Environment=OLLAMA_NO_CLOUD=1\n"
}

/// `server.json` com `disable_ollama_cloud` trocado; o resto fica.
fn server_json(existing: Option<&str>, cloud: bool) -> anyhow::Result<String> {
    let mut json: serde_json::Value = match existing.map(str::trim) {
        Some(text) if !text.is_empty() => {
            serde_json::from_str(text).context("server.json do Ollama inválido")?
        }
        _ => serde_json::json!({}),
    };
    let object = json
        .as_object_mut()
        .context("server.json do Ollama não é um objeto")?;
    object.insert("disable_ollama_cloud".into(), (!cloud).into());
    Ok(format!("{}\n", serde_json::to_string_pretty(&json)?))
}

fn home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME ausente")
}

fn run_checked(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("não foi possível executar {program}"))?;
    if !status.success() {
        let command = format!("{program} {}", args.join(" "));
        bail!("`{command}` falhou");
    }
    Ok(())
}

/// Como rodar os passos de administrador do serviço do sistema.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Elevation {
    /// `sudo` em cada passo, pedindo a senha no terminal.
    Sudo,
    /// Uma chamada ao `pkexec` (janela de senha do polkit) para todos os
    /// passos.
    Pkexec(PathBuf),
}

impl Elevation {
    /// `sudo` num terminal; `pkexec` quando pedido ou sem terminal (hub).
    fn choose(force_pkexec: bool) -> Option<Elevation> {
        if !force_pkexec && std::io::stdin().is_terminal() {
            return Some(Elevation::Sudo);
        }
        trusted_pkexec(Path::new(PKEXEC)).map(Elevation::Pkexec)
    }
}

/// Caminho fixo: nunca o `pkexec` que vier primeiro no `PATH`, que poderia
/// ser outro programa recebendo os passos de administrador.
const PKEXEC: &str = "/usr/bin/pkexec";

/// `path` é o `pkexec` do sistema: arquivo do root com setuid (sem o bit,
/// ele não eleva nada e só falharia depois da senha).
fn trusted_pkexec(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.uid() == 0 && meta.mode() & 0o4000 != 0).then(|| path.to_path_buf())
}

/// Os passos de administrador num só `sh -c` (`pkexec` ou `sudo`: uma
/// senha só). O conteúdo do drop-in (fixo, [`drop_in_text`]) e o destino vão
/// como argumentos posicionais (`$1`, `$2`), nunca interpolados no script, e
/// o root grava a partir deles: nenhum arquivo do usuário é lido como root
/// (um temporário em `TMPDIR` podia ser trocado por outro processo do mesmo
/// usuário antes do `install`, A-30). Grava ao lado e renomeia, `0644`.
const SYSTEM_OFFLINE: &str = "install -d -m 0755 \"$(dirname \"$2\")\" && printf '%s' \"$1\" > \"$2.fastwisper-tmp\" && chmod 0644 \"$2.fastwisper-tmp\" && mv -f \"$2.fastwisper-tmp\" \"$2\" && systemctl daemon-reload && systemctl restart ollama.service";
const SYSTEM_ONLINE: &str =
    "rm -f \"$2\" && systemctl daemon-reload && systemctl restart ollama.service";

/// Grava/remove o drop-in do serviço do sistema em `units` (o
/// `/etc/systemd/system`) como administrador e reinicia o Ollama.
fn apply_system(cloud: bool, elevation: Option<Elevation>, units: &Path) -> anyhow::Result<bool> {
    let target = units.join(DROP_IN);
    if cloud && !target.exists() {
        return Ok(false);
    }
    let target_str = target.display().to_string();
    let content = if cloud { "" } else { drop_in_text() };
    let script = if cloud { SYSTEM_ONLINE } else { SYSTEM_OFFLINE };
    let args = ["/bin/sh", "-c", script, "sh", content, &target_str];
    match elevation {
        Some(Elevation::Sudo) => {
            crate::human_print!(
                "O Ollama roda como serviço do sistema: o sudo vai pedir sua senha."
            );
            run_checked("sudo", &args)?;
        }
        Some(Elevation::Pkexec(pkexec)) => {
            crate::human_print!(
                "O Ollama roda como serviço do sistema: o polkit vai pedir sua senha."
            );
            let status = Command::new(&pkexec)
                .args(args)
                .status()
                .with_context(|| format!("não foi possível executar {}", pkexec.display()))?;
            match status.code() {
                Some(0) => {}
                // 126: janela fechada ou senha recusada; 127: sem autorização.
                Some(126 | 127) => bail!("autorização do polkit negada ou cancelada"),
                _ => bail!("os passos de administrador falharam ({status})"),
            }
        }
        None => {
            // Passos para copiar no terminal, com o mesmo conteúdo fixo.
            let mut commands = Vec::new();
            if cloud {
                commands.push(format!("sudo rm -f {target_str}"));
            } else {
                let dir = target.parent().unwrap_or(units).display().to_string();
                commands.push(format!("sudo install -d -m 0755 {dir}"));
                commands.push(format!(
                    "printf '[Service]\\nEnvironment=OLLAMA_NO_CLOUD=1\\n' | sudo tee {target_str}"
                ));
            }
            commands.push("sudo systemctl daemon-reload".into());
            commands.push("sudo systemctl restart ollama.service".into());
            bail!(
                "o Ollama roda como serviço do sistema e precisa de administrador, mas não há terminal e o pkexec do sistema ({PKEXEC}, do root com setuid) não foi encontrado; instale o polkit ou rode `fastwisper ollama apply` num terminal (passos: {})",
                commands.join(" && ")
            );
        }
    }
    Ok(true)
}

fn apply_user(cloud: bool) -> anyhow::Result<bool> {
    let target = home()?.join(".config/systemd/user").join(DROP_IN);
    if cloud {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e).context("não foi possível remover o drop-in"),
        }
    } else {
        config_edit::write_atomic(&target, drop_in_text())
            .context("não foi possível gravar o drop-in")?;
    }
    run_checked("systemctl", &["--user", "daemon-reload"])?;
    run_checked("systemctl", &["--user", "restart", "ollama.service"])?;
    Ok(true)
}

fn apply_manual(cloud: bool) -> anyhow::Result<()> {
    let path = home()?.join(".ollama/server.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("falha ao ler server.json do Ollama"),
    };
    let updated = server_json(existing.as_deref(), cloud)?;
    config_edit::write_atomic(&path, &updated)
        .context("não foi possível gravar server.json do Ollama")?;
    crate::human_print!(
        "Gravado {}. Reinicie o Ollama para valer (ele não roda como serviço systemd).",
        path.display()
    );
    Ok(())
}

/// Espera o Ollama voltar depois do restart.
fn wait_status(endpoint: &str) -> Result<CloudStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match cloud_status(endpoint) {
            Ok(status) => return Ok(status),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// Aplica `cloud` no Ollama de `endpoint`, se o estado atual for outro.
fn apply(endpoint: &str, cloud: bool, force_pkexec: bool) -> anyhow::Result<()> {
    let before = cloud_status(endpoint).map_err(anyhow::Error::msg)?;
    if before.disabled != cloud {
        crate::human_print!("Ollama: já está com {}.", before.describe());
        return Ok(());
    }
    crate::human_print!("Reiniciar o Ollama descarrega os modelos carregados.");
    let restarted = match detect() {
        Deployment::System => apply_system(
            cloud,
            Elevation::choose(force_pkexec),
            Path::new(SYSTEM_UNITS),
        )?,
        Deployment::User => apply_user(cloud)?,
        Deployment::Manual => {
            apply_manual(cloud)?;
            return Ok(());
        }
    };
    let after = if restarted {
        wait_status(endpoint).map_err(anyhow::Error::msg)?
    } else {
        before
    };
    if after.disabled == cloud {
        bail!(
            "o Ollama continua com {}: isso vem de fora do FastWisper (ex.: OLLAMA_NO_CLOUD em outro drop-in ou server.json)",
            after.describe()
        );
    }
    crate::human_print!("Ollama: {}.", after.describe());
    Ok(())
}

fn local_ollama(config: &Config) -> anyhow::Result<String> {
    if config.llm.engine != LlmEngineKind::Ollama {
        bail!("llm.engine não é \"ollama\"; nada a fazer");
    }
    let endpoint = check_endpoint(&config.llm.endpoint, config.llm.remote_consent)
        .map_err(anyhow::Error::msg)?;
    if endpoint.remote {
        bail!("llm.endpoint aponta para outra máquina; o FastWisper só ajusta o Ollama local");
    }
    Ok(config.llm.endpoint.clone())
}

pub fn run(args: OllamaArgs) -> anyhow::Result<()> {
    let mut config = Config::load(args.config.config.as_deref())?;
    let path = args.config.config.map_or_else(Config::default_path, Ok)?;
    let cloud = match args.action {
        Some(Action::Models) => {
            let models = installed_models(&local_ollama(&config)?).map_err(anyhow::Error::msg)?;
            if args.json {
                crate::outln!("{}", serde_json::to_string(&models)?);
            } else if models.is_empty() {
                crate::human_print!(
                    "Nenhum modelo local no Ollama (instale com `ollama pull {}`).",
                    fw_core::models::OLLAMA_MODEL
                );
            } else {
                for model in models {
                    let mark = if model == config.llm.model {
                        "→"
                    } else {
                        " "
                    };
                    crate::outln!("{mark} {model}");
                }
            }
            return Ok(());
        }
        None if args.json => {
            let ollama = local_ollama(&config)
                .and_then(|endpoint| cloud_status(&endpoint).map_err(anyhow::Error::msg));
            let ollama = match ollama {
                Ok(status) => {
                    serde_json::json!({ "disabled": status.disabled, "source": status.source })
                }
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            };
            let row = serde_json::json!({ "cloud": config.llm.ollama_cloud, "ollama": ollama });
            crate::outln!("{row}");
            return Ok(());
        }
        None => {
            crate::human_print!("Config: {}", describe_choice(config.llm.ollama_cloud));
            let endpoint = local_ollama(&config)?;
            match cloud_status(&endpoint) {
                Ok(status) => {
                    crate::human_print!("Ollama: {}", status.describe());
                    if status.disabled == config.llm.ollama_cloud {
                        crate::human_print!("Diferente da config: rode `fastwisper ollama apply`.");
                    }
                }
                Err(e) => crate::human_print!("Ollama: {e}"),
            }
            return Ok(());
        }
        Some(Action::Apply) => {
            // Sem Ollama local respondendo não há o que ajustar (instalação
            // sem LLM, Ollama parado): não é erro.
            let Ok(endpoint) = local_ollama(&config) else {
                crate::human_print!("Nada a aplicar: o LLM configurado não é um Ollama local.");
                return Ok(());
            };
            if let Err(e) = cloud_status(&endpoint) {
                crate::human_print!("Nada aplicado: {e}.");
                return Ok(());
            }
            config.llm.ollama_cloud
        }
        Some(action) => {
            let cloud = action == Action::Online;
            config_edit::update(&path, vec![("llm.ollama_cloud".into(), Value::from(cloud))])
                .context("não foi possível salvar a config")?;
            config.llm.ollama_cloud = cloud;
            crate::human_print!("Config: {}", describe_choice(cloud));
            // O daemon não usa a escolha, mas acompanha a config gravada.
            match fw_core::daemon::DaemonPaths::from_env()
                .map_err(anyhow::Error::from)
                .and_then(|paths| crate::config::reload_for(&paths, &path))
            {
                Ok(crate::config::ReloadFor::Reload(crate::config::Reload::Done(report))) => {
                    if let Some(notice) = crate::config::restart_notice(&report) {
                        crate::human_print!("{notice}");
                    }
                }
                Ok(crate::config::ReloadFor::Elsewhere(daemon)) => {
                    crate::config::print_elsewhere(&path, daemon.as_deref())
                }
                _ => {}
            }
            cloud
        }
    };
    if args.no_apply {
        return Ok(());
    }
    let endpoint = local_ollama(&config)?;
    apply(&endpoint, cloud, args.pkexec).context("config salva, mas o Ollama não foi ajustado")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn server_json_keeps_other_keys() {
        let out = server_json(Some(r#"{"outra": 1}"#), false).expect("json");
        let json: serde_json::Value = serde_json::from_str(&out).expect("válido");
        assert_eq!(json["outra"], 1);
        assert_eq!(json["disable_ollama_cloud"], true);
        let out = server_json(None, true).expect("json");
        assert!(out.contains("\"disable_ollama_cloud\": false"));
        assert!(server_json(Some("[1]"), true).is_err());
    }

    #[test]
    fn drop_in_disables_cloud() {
        assert!(drop_in_text().contains("\nEnvironment=OLLAMA_NO_CLOUD=1\n"));
        assert!(drop_in_text().contains("[Service]"));
    }

    #[test]
    fn tags_skip_cloud_models() {
        let body = r#"{"models":[{"name":"qwen3:8b"},{"name":"gpt-oss:120b-cloud"},{"model":"qwen3:4b-instruct"},{"name":"qwen3:8b"}]}"#;
        assert_eq!(
            parse_tags(body),
            Some(vec!["qwen3:4b-instruct".to_owned(), "qwen3:8b".to_owned()])
        );
        assert_eq!(parse_tags(r#"{"models":[]}"#), Some(vec![]));
        assert_eq!(parse_tags("{}"), None);
    }

    /// `pkexec` falso: roda o comando como o usuário, com um `systemctl`
    /// falso no PATH que só anota os argumentos. Nada toca o sistema.
    struct FakePkexec {
        dir: tempfile::TempDir,
    }

    impl FakePkexec {
        fn new(exit: Option<i32>) -> FakePkexec {
            let dir = tempfile::tempdir().expect("temporário");
            let bin = dir.path().join("bin");
            std::fs::create_dir(&bin).expect("bin");
            let log = dir.path().join("log");
            let script = |path: &Path, body: String| {
                std::fs::write(path, body).expect("script");
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            };
            script(
                &bin.join("systemctl"),
                format!("#!/bin/sh\necho \"systemctl $*\" >> '{}'\n", log.display()),
            );
            let run = match exit {
                Some(code) => format!("exit {code}"),
                None => format!("PATH='{}':\"$PATH\" exec \"$@\"", bin.display()),
            };
            script(
                &dir.path().join("pkexec"),
                format!(
                    "#!/bin/sh\necho \"pkexec $1 $2\" >> '{}'\nprintf '%s' \"$5\" > '{}'\n{run}\n",
                    log.display(),
                    dir.path().join("content").display()
                ),
            );
            std::fs::create_dir(dir.path().join("units")).expect("units");
            FakePkexec { dir }
        }

        fn elevation(&self) -> Option<Elevation> {
            Some(Elevation::Pkexec(self.dir.path().join("pkexec")))
        }

        fn units(&self) -> PathBuf {
            self.dir.path().join("units")
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
        }
    }

    #[test]
    fn pkexec_writes_the_drop_in_and_restarts_in_one_call() {
        let fake = FakePkexec::new(None);
        assert!(apply_system(false, fake.elevation(), &fake.units()).expect("offline"));
        let drop_in = fake.units().join(DROP_IN);
        assert_eq!(
            std::fs::read_to_string(&drop_in).expect("drop-in"),
            drop_in_text()
        );
        assert_eq!(
            fake.log(),
            "pkexec /bin/sh -c\nsystemctl daemon-reload\nsystemctl restart ollama.service\n",
            "uma senha só, para todos os passos"
        );
        // O conteúdo vai como argumento: o root não lê arquivo do usuário
        // (A-30), e nada sobra ao lado do destino.
        assert_eq!(
            std::fs::read_to_string(fake.dir.path().join("content")).expect("argumento"),
            drop_in_text()
        );
        let names: Vec<_> = std::fs::read_dir(drop_in.parent().expect("pasta"))
            .expect("pasta")
            .map(|e| e.expect("entrada").file_name())
            .collect();
        assert_eq!(names, ["fastwisper-cloud.conf"]);
        assert_eq!(
            std::fs::metadata(&drop_in)
                .expect("drop-in")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );

        let fake_online = FakePkexec::new(None);
        std::fs::create_dir_all(drop_in.parent().expect("pasta")).expect("pasta");
        assert!(apply_system(true, fake_online.elevation(), &fake.units()).expect("online"));
        assert!(!drop_in.exists(), "online remove o drop-in");
        assert!(
            fake_online
                .log()
                .ends_with("systemctl restart ollama.service\n")
        );
    }

    #[test]
    fn pkexec_is_not_called_without_a_change_and_refusal_is_an_error() {
        let fake = FakePkexec::new(None);
        assert!(!apply_system(true, fake.elevation(), &fake.units()).expect("nada a remover"));
        assert_eq!(fake.log(), "", "sem drop-in, online não pede senha");

        let denied = FakePkexec::new(Some(126));
        let error = apply_system(false, denied.elevation(), &denied.units())
            .expect_err("senha recusada")
            .to_string();
        assert!(error.contains("polkit"), "{error}");
        assert!(!denied.units().join(DROP_IN).exists());

        let none = FakePkexec::new(None);
        let error = apply_system(false, None, &none.units())
            .expect_err("sem terminal nem pkexec")
            .to_string();
        assert!(error.contains("sudo install"), "{error}");
        assert!(error.contains("/usr/bin/pkexec"), "{error}");
        assert_eq!(none.log(), "");
    }

    /// Só o `pkexec` do root com setuid serve (A-28): um programa com o
    /// mesmo nome noutra pasta do `PATH`, ou sem o bit, é ignorado.
    #[test]
    fn only_a_root_setuid_pkexec_is_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("pkexec");
        std::fs::write(&fake, "#!/bin/sh\n").expect("pkexec falso");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o4755)).expect("chmod");
        assert_eq!(trusted_pkexec(&fake), None, "dono não é o root");
        assert_eq!(trusted_pkexec(&dir.path().join("ausente")), None);
        assert_eq!(trusted_pkexec(dir.path()), None);
        // O do sistema, quando existe com setuid (como no Omarchy), é aceito.
        let system = Path::new(PKEXEC);
        let setuid = std::fs::metadata(system).is_ok_and(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid() == 0 && m.mode() & 0o4000 != 0
        });
        assert_eq!(trusted_pkexec(system).is_some(), setuid);
    }

    #[test]
    fn choice_is_saved_preserving_the_file() {
        let before = "# minha config\n[llm]\nmodel = \"qwen3:8b\" # meu\n";
        let after = config_edit::set(
            before,
            "llm.ollama_cloud",
            Value::from(true),
            Path::new("c.toml"),
        )
        .expect("válido");
        assert!(after.starts_with(before));
        let config = Config::parse(&after, Path::new("c.toml")).expect("config");
        assert!(config.llm.ollama_cloud);
        assert_eq!(config.llm.model, "qwen3:8b");
    }
}
