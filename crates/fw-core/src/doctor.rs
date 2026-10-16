//! Diagnóstico do `fastwisper doctor`, também resumido no hub: cada peça
//! vira um [`Check`] com o que fazer. Só lê: não muda config, não baixa nada
//! e só fala com o Ollama local (endpoint remoto é avisado, nunca chamado).
//!
//! Microfone (PipeWire) e GPU (whisper.cpp) ficam na CLI, que tem essas
//! features; aqui só a parte que não depende delas.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::client;
use crate::config::{Config, LlmEngineKind};
use crate::daemon::DaemonPaths;
use crate::gpu_probe::Probe;
use crate::ipc::{Request, Response};
use crate::llm::check_endpoint;
use crate::models::{
    MANIFEST, OLLAMA_DIGEST, OLLAMA_MODEL, OllamaModelCheck, check_ollama_tags,
    is_ollama_cloud_name,
};
use crate::pipeline::Readiness;
use crate::sandbox::{NetworkSandbox, allowed_connect_ports, probe_kernel};
use crate::session::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// Resultado de uma peça. Sem texto ditado nem título de janela.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Peça conferida (`daemon`, `llm`, `nuvem`...).
    pub item: String,
    pub level: Level,
    pub detail: String,
    /// Como consertar (só em aviso e falha).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Checks em ordem. `on_check` recebe cada um assim que sai (a CLI imprime
/// na hora, sem esperar as consultas lentas).
#[derive(Default)]
pub struct Report {
    pub checks: Vec<Check>,
    on_check: Option<fn(&Check)>,
}

impl Report {
    pub fn streaming(on_check: fn(&Check)) -> Report {
        Report {
            checks: Vec::new(),
            on_check: Some(on_check),
        }
    }

    fn push(&mut self, item: &str, level: Level, detail: String, hint: Option<&str>) {
        let check = Check {
            item: item.to_owned(),
            level,
            detail,
            hint: hint.map(str::to_owned),
        };
        if let Some(on_check) = self.on_check {
            on_check(&check);
        }
        self.checks.push(check);
    }

    pub fn ok(&mut self, item: &str, detail: impl AsRef<str>) {
        self.push(item, Level::Ok, detail.as_ref().to_owned(), None);
    }

    pub fn warn(&mut self, item: &str, detail: impl AsRef<str>, hint: &str) {
        self.push(item, Level::Warn, detail.as_ref().to_owned(), Some(hint));
    }

    pub fn fail(&mut self, item: &str, detail: impl AsRef<str>, hint: &str) {
        self.push(item, Level::Fail, detail.as_ref().to_owned(), Some(hint));
    }

    pub fn failures(&self) -> usize {
        self.count(Level::Fail)
    }

    pub fn warnings(&self) -> usize {
        self.count(Level::Warn)
    }

    fn count(&self, level: Level) -> usize {
        self.checks.iter().filter(|c| c.level == level).count()
    }

    /// Frase final ("Tudo certo.", "Funciona, com 2 aviso(s)."...).
    pub fn verdict(&self) -> String {
        match (self.failures(), self.warnings()) {
            (0, 0) => "Tudo certo.".to_owned(),
            (0, w) => format!("Funciona, com {w} aviso(s)."),
            (f, _) => format!("{f} problema(s) impedem o ditado."),
        }
    }
}

/// Carrega a config (padrão ou `path`); inválida vira falha e os padrões.
pub fn check_config(r: &mut Report, path: Option<&Path>) -> Config {
    match Config::load(path) {
        Ok(config) => {
            let path = path
                .map(Path::to_path_buf)
                .or_else(|| Config::default_path().ok());
            r.ok(
                "config",
                path.map_or("padrões".into(), |p| p.display().to_string()),
            );
            config
        }
        Err(e) => {
            r.fail(
                "config",
                e.to_string(),
                "corrija o arquivo ou rode `fastwisper setup`",
            );
            Config::default()
        }
    }
}

/// Devolve a resposta de `status` quando o daemon respondeu.
pub fn check_daemon(r: &mut Report) -> Option<Response> {
    let service = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "fastwisperd"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let Ok(paths) = DaemonPaths::from_env() else {
        r.fail(
            "daemon",
            "XDG_RUNTIME_DIR ausente",
            "rode dentro da sessão gráfica",
        );
        return None;
    };
    match client::send(&paths.socket, &Request::new("doctor", Command::Status)) {
        Ok(status) => {
            match status.asr {
                Some(Readiness::Ready) => r.ok(
                    "daemon",
                    format!("respondendo; serviço {service}; ASR pronto"),
                ),
                Some(Readiness::Loading) => {
                    r.warn("daemon", "carregando o modelo", "aguarde alguns segundos")
                }
                _ => r.fail(
                    "daemon",
                    "ASR indisponível",
                    "veja `journalctl --user -u fastwisperd`",
                ),
            }
            Some(status)
        }
        Err(e) => {
            r.fail(
                "daemon",
                format!("{e} (serviço: {service})"),
                "systemctl --user start fastwisperd  (ou packaging/omarchy/install.sh)",
            );
            None
        }
    }
}

const LANDLOCK_HINT: &str = "regras de rede do Landlock exigem Linux 6.7+ com landlock na lista de LSM (/sys/kernel/security/lsm); o IPAddressDeny da unidade não vale em serviço de usuário";

/// Bloqueio de rede do daemon (Landlock), como o daemon em execução informou.
pub fn check_network(r: &mut Report, config: &Config, status: Option<&Response>) {
    let Some(status) = status else {
        // Daemon fora do ar: só dá para dizer se o kernel aceita.
        match probe_kernel() {
            Ok(()) => r.ok(
                "rede",
                "kernel aceita Landlock de rede; o daemon bloqueia TCP ao iniciar",
            ),
            Err(e) => r.warn("rede", e, LANDLOCK_HINT),
        }
        return;
    };
    match &status.network {
        Some(sandbox @ NetworkSandbox::Enforced { connect_ports, .. }) => {
            if *connect_ports == allowed_connect_ports(&config.llm) {
                r.ok("rede", sandbox.to_string());
            } else {
                r.warn(
                    "rede",
                    format!("{sandbox}, mas a config atual pede outra porta"),
                    "o bloqueio é fixado ao iniciar: systemctl --user restart fastwisperd",
                );
            }
        }
        Some(sandbox) => r.warn("rede", sandbox.to_string(), LANDLOCK_HINT),
        None => r.warn(
            "rede",
            "o daemon em execução não informou o bloqueio de rede (versão antiga)",
            "systemctl --user restart fastwisperd",
        ),
    }
}

/// `audio.device` preenchido? A conferência no PipeWire fica com quem tem a
/// captura (a CLI).
pub fn check_microphone_configured(r: &mut Report, config: &Config) -> bool {
    if config.audio.device.trim().is_empty() {
        r.fail("microfone", "não configurado", "fastwisper setup");
        return false;
    }
    true
}

pub fn check_model(r: &mut Report, config: &Config) {
    let path = config.asr.model_path();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let spec = MANIFEST.iter().find(|m| m.file_name == name);
    match (std::fs::metadata(&path), spec) {
        (Err(_), Some(spec)) => r.fail(
            "modelo",
            format!("{} ausente", path.display()),
            &format!("fastwisper models pull {}", spec.id),
        ),
        (Err(_), None) => r.fail(
            "modelo",
            format!("{} ausente", path.display()),
            "ajuste asr.model",
        ),
        (Ok(meta), Some(spec)) if meta.len() != spec.size => r.fail(
            "modelo",
            format!("{} com tamanho errado", path.display()),
            &format!(
                "fastwisper models pull {} (ou `fastwisper models verify`)",
                spec.id
            ),
        ),
        (Ok(meta), _) => r.ok(
            "modelo",
            format!("{} ({} MB)", path.display(), meta.len() / 1_000_000),
        ),
    }
}

/// GPU do ASR pela `asr.gpu` sobre a sondagem das GPUs Vulkan (`probe`, num
/// processo filho; os testes passam o resultado). Sem GPU, a CPU é um caminho
/// suportado: o aviso diz como ganhar velocidade, sem supor fabricante. Um
/// driver que travou ou derrubou a sondagem é aviso: o ditado segue na CPU.
pub fn check_gpu(r: &mut Report, config: &Config, probe: &Probe) {
    if let Some(failure) = probe.failure() {
        r.warn(
            "gpu",
            crate::gpu_probe::broken_detail(failure),
            BROKEN_DRIVER_HINT,
        );
        return;
    }
    let names = probe.gpus();
    // `asr.gpu = "cpu"` não sonda: nada a dizer sobre GPUs que não vimos.
    let no_gpu = names.is_empty() && *probe != Probe::Skipped;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    // O perfil GPU na CPU é lento (~13 s por frase numa CPU desktop de 8 núcleos).
    let gpu_model = MANIFEST
        .iter()
        .find(|m| m.id == crate::models::DEFAULT_MODEL_ID)
        .is_some_and(|m| {
            config.asr.model_path().file_name() == Some(std::ffi::OsStr::new(m.file_name))
        });
    match crate::asr::select_gpu(&config.asr.gpu, &refs) {
        Ok(Some(i)) => r.ok(
            "gpu",
            format!("{} (asr.gpu = \"{}\")", names[i], config.asr.gpu),
        ),
        Ok(None) if gpu_model => r.warn(
            "gpu",
            if no_gpu {
                "nenhuma GPU Vulkan: o modelo grande na CPU é lento"
            } else {
                "CPU (asr.gpu = \"cpu\"): o modelo grande na CPU é lento"
            },
            if no_gpu {
                CPU_MODEL_HINT_NO_GPU
            } else {
                CPU_MODEL_HINT
            },
        ),
        Ok(None) if no_gpu => r.warn(
            "gpu",
            "nenhuma GPU Vulkan: transcrição na CPU",
            VULKAN_DRIVER_HINT,
        ),
        Ok(None) => r.ok("gpu", "CPU (asr.gpu = \"cpu\")"),
        Err(e) => r.fail(
            "gpu",
            e.to_string(),
            "ajuste asr.gpu (\"auto\", \"cpu\" ou parte do nome)",
        ),
    }
}

const BROKEN_DRIVER_HINT: &str = "atualize ou reinstale o driver Vulkan da GPU (vulkan-radeon, vulkan-intel ou nvidia-utils) e reinicie o daemon (`fastwisper restart`); `asr.gpu = \"cpu\"` pula a sondagem";
const VULKAN_DRIVER_HINT: &str = "normal numa máquina sem GPU. Com GPU, instale o driver Vulkan dela (vulkan-radeon, vulkan-intel ou nvidia-utils) e o vulkan-icd-loader";
const CPU_MODEL_HINT: &str =
    "use o modelo small: `fastwisper models use small-q5_1` e `fastwisper models pull small-q5_1`";
const CPU_MODEL_HINT_NO_GPU: &str = "use o modelo small (`fastwisper models use small-q5_1` e `fastwisper models pull small-q5_1`) ou instale o driver Vulkan da GPU (vulkan-radeon, vulkan-intel ou nvidia-utils)";

/// Prazo total da consulta ao Ollama no `doctor` (conexão + resposta): um
/// Ollama que aceita a conexão e não responde vira aviso, nunca espera sem
/// fim (o hub mostra "Conferindo…" enquanto isso).
pub const LLM_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

pub fn check_llm(r: &mut Report, config: &Config) {
    check_llm_within(r, config, LLM_CHECK_TIMEOUT);
}

/// [`check_llm`] com outro prazo total (testes).
pub fn check_llm_within(r: &mut Report, config: &Config, timeout: Duration) {
    if !config.llm.enabled {
        r.ok("llm", "desligado (só regras)");
        return;
    }
    let endpoint = match check_endpoint(&config.llm.endpoint, config.llm.remote_consent) {
        Ok(e) => e,
        Err(e) => {
            r.fail(
                "llm",
                e,
                "ajuste llm.endpoint (o ditado segue só com regras)",
            );
            return;
        }
    };
    if config.llm.engine == LlmEngineKind::Ollama && endpoint.remote {
        r.warn(
            "llm",
            "engine ollama é só local",
            "ditado segue só com regras; provider remoto usa engine = \"openai_compatible\" com remote_consent",
        );
        return;
    }
    // Mesmo nome que o cliente usa (sem espaços nas pontas).
    let model = config.llm.model.trim();
    if config.llm.engine == LlmEngineKind::Ollama && is_ollama_cloud_name(model) {
        r.warn(
            "llm",
            format!("{model} é um modelo de nuvem do Ollama"),
            &format!(
                "o texto sairia da máquina: não é usado. Escolha um modelo local (padrão recomendado: {OLLAMA_MODEL})"
            ),
        );
        return;
    }
    if endpoint.remote {
        r.warn(
            "llm",
            format!(
                "provider remoto {} (o texto ditado sai da máquina)",
                endpoint.url.host_str().unwrap_or("?")
            ),
            "desligue com llm.remote_consent = false se não for intencional",
        );
        return;
    }
    if config.llm.engine == LlmEngineKind::Ollama {
        let tags = format!("{}/api/tags", config.llm.endpoint.trim_end_matches('/'));
        // Prazo global: conexão, cabeçalho e corpo somados.
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .proxy(None)
            .max_redirects(0)
            .build()
            .new_agent();
        let mut timed_out = false;
        // Nada escutando (Ollama não instalado ou parado), em vez de uma
        // resposta inválida ou redirect de algo que escuta na porta.
        let mut absent = false;
        let installed = agent
            .get(&tags)
            .call()
            .map_err(|e| {
                timed_out = matches!(e, ureq::Error::Timeout(_));
                absent = matches!(e, ureq::Error::Io(_) | ureq::Error::ConnectionFailed);
            })
            .ok()
            .filter(|res| res.status().is_success())
            .and_then(|mut res| {
                res.body_mut()
                    .read_to_string()
                    .map_err(|e| timed_out = matches!(e, ureq::Error::Timeout(_)))
                    .ok()
            })
            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok());
        match installed {
            None if timed_out => {
                r.warn(
                    "llm",
                    format!("sem resposta do LLM em {} s", timeout.as_secs().max(1)),
                    "o Ollama aceitou a conexão e não respondeu; ditado segue só com regras até ele voltar",
                );
                // A nuvem é consultada no mesmo Ollama travado: só repetiria a espera.
                return;
            }
            None if absent => {
                r.warn(
                    "llm",
                    "Ollama não respondeu",
                    "ditado segue só com regras. Para formatar com IA, instale e inicie o Ollama com um modelo local; ou desligue com `fastwisper config set llm.enabled false`",
                );
                // Sem Ollama (não instalado ou parado) não há nuvem a conferir.
                return;
            }
            None => r.warn(
                "llm",
                "Ollama não respondeu",
                "ditado segue só com regras; nenhum download automático",
            ),
            Some(tags) => report_ollama_model(r, model, check_ollama_tags(&tags, model)),
        }
        if ollama_listens_everywhere() {
            r.warn(
                "ollama",
                "escuta em todas as interfaces (0.0.0.0:11434)",
                "outras máquinas da rede podem usar o seu Ollama; restrinja OLLAMA_HOST a 127.0.0.1 se não for intencional",
            );
        }
        check_cloud(r, config);
    } else {
        r.ok(
            "llm",
            format!("OpenAI-compatível local em {}", config.llm.endpoint),
        );
    }
}

/// O pin é verificação, não proibição: modelo fora dele ou com outro digest
/// é usado e vira aviso. Só o padrão recomendado é sugerido.
fn report_ollama_model(r: &mut Report, model: &str, check: OllamaModelCheck) {
    match check {
        OllamaModelCheck::Pinned => r.ok(
            "llm",
            format!("{model} digest {OLLAMA_DIGEST} conferido"),
        ),
        OllamaModelCheck::DigestMismatch { digest } => r.warn(
            "llm",
            format!(
                "{model} com digest {} diferente do fixado",
                digest.as_deref().unwrap_or("ausente")
            ),
            &format!(
                "o ditado usa este modelo mesmo assim; o digest verificado é {OLLAMA_DIGEST} (`fastwisper models llm-manifest`)"
            ),
        ),
        OllamaModelCheck::Unpinned { .. } => r.warn(
            "llm",
            format!("{model} fora do manifesto (sem digest fixado)"),
            &format!(
                "permitido: o ditado usa este modelo. Padrão recomendado e verificado: {OLLAMA_MODEL}"
            ),
        ),
        OllamaModelCheck::Missing => r.warn(
            "llm",
            format!("{model} não está instalado no Ollama"),
            &format!(
                "ditado segue só com regras; instale você mesmo com `ollama pull {model}` (o FastWisper não baixa modelos sozinho)"
            ),
        ),
        OllamaModelCheck::Cloud => r.warn(
            "llm",
            format!("{model} é um alias de nuvem no Ollama"),
            &format!(
                "o texto sairia da máquina: não é usado. Escolha um modelo local (padrão recomendado: {OLLAMA_MODEL})"
            ),
        ),
        OllamaModelCheck::Malformed => r.warn(
            "llm",
            "inventário do Ollama inválido",
            "ditado segue só com regras",
        ),
    }
}

/// `/proc/net/tcp*`: porta 11434 em LISTEN no endereço "todos".
fn ollama_listens_everywhere() -> bool {
    let port = format!(":{:04X}", 11434);
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|file| {
        std::fs::read_to_string(file).is_ok_and(|table| {
            table.lines().skip(1).any(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                cols.len() > 3
                    && cols[3] == "0A"
                    && cols[1].ends_with(&port)
                    && cols[1].trim_end_matches(&port).bytes().all(|b| b == b'0')
            })
        })
    })
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|dir| dir.join(program).is_file()))
}

pub fn check_desktop(r: &mut Report) {
    for (program, hint) in [
        ("wl-copy", "instale wl-clipboard"),
        ("notify-send", "instale libnotify (avisos de erro)"),
    ] {
        if in_path(program) {
            r.ok(program, "presente");
        } else {
            r.fail(program, "não encontrado", hint);
        }
    }
    match crate::hypr::Hyprland::from_env() {
        Some(_) => r.ok("hyprland", "socket da sessão encontrado"),
        None => r.fail(
            "hyprland",
            "HYPRLAND_INSTANCE_SIGNATURE ausente",
            "o FastWisper cola só no Hyprland",
        ),
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    let bindings = Path::new(&home).join(".config/hypr/bindings.lua");
    let installed =
        std::fs::read_to_string(&bindings).is_ok_and(|s| s.contains("-- BEGIN fastwisper"));
    if installed {
        r.ok("atalhos", "binds do FastWisper em bindings.lua");
    } else {
        r.fail("atalhos", "não instalados", "packaging/omarchy/install.sh");
    }
}

/// Nuvem do Ollama (`/api/status`) contra `llm.ollama_cloud`.
fn check_cloud(r: &mut Report, config: &Config) {
    let wanted = config.llm.ollama_cloud;
    match crate::ollama::cloud_status(&config.llm.endpoint) {
        Ok(status) if status.disabled != wanted => {
            let choice = if wanted { "online" } else { "offline" };
            r.ok(
                "nuvem",
                format!("{} (llm.ollama_cloud: {choice})", status.describe()),
            );
        }
        Ok(status) if wanted => r.warn(
            "nuvem",
            format!("{}, mas llm.ollama_cloud = true", status.describe()),
            "fastwisper ollama apply (ou ajuste a config)",
        ),
        Ok(status) => r.warn(
            "nuvem",
            format!("{}: o Ollama pode acessar a internet", status.describe()),
            "fastwisper ollama offline (ou `fastwisper ollama online` se for intencional)",
        ),
        Err(e) => r.warn("nuvem", e, "atualize o Ollama para conferir a nuvem"),
    }
}

/// Tudo o que não depende de PipeWire nem de GPU, na ordem da CLI. O
/// microfone só é conferido na config; a GPU, não.
pub fn run_portable(config_path: Option<&Path>) -> Report {
    let mut r = Report::default();
    let config = check_config(&mut r, config_path);
    let status = check_daemon(&mut r);
    check_network(&mut r, &config, status.as_ref());
    check_microphone_configured(&mut r, &config);
    check_model(&mut r, &config);
    check_llm(&mut r, &config);
    check_desktop(&mut r);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_probe::Failure;

    #[test]
    fn report_counts_and_verdict() {
        let mut r = Report::default();
        r.ok("config", "padrões");
        assert_eq!(r.verdict(), "Tudo certo.");
        r.warn("llm", "Ollama não respondeu", "ditado segue só com regras");
        assert_eq!(r.verdict(), "Funciona, com 1 aviso(s).");
        r.fail("modelo", "ausente", "fastwisper models pull small");
        assert_eq!((r.failures(), r.warnings()), (1, 1));
        assert_eq!(r.verdict(), "1 problema(s) impedem o ditado.");
        let json = serde_json::to_value(&r.checks).expect("json");
        assert_eq!(
            json[0],
            serde_json::json!({"item":"config","level":"ok","detail":"padrões"})
        );
        assert_eq!(json[2]["hint"], "fastwisper models pull small");
    }

    #[test]
    fn streaming_sees_each_check_in_order() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut r = Report::streaming(|c| SEEN.lock().expect("lock").push(c.item.clone()));
        r.ok("a", "1");
        r.fail("b", "2", "3");
        assert_eq!(*SEEN.lock().expect("lock"), ["a", "b"]);
    }

    /// Ollama que aceita a conexão e nunca responde (A-20): aviso no prazo,
    /// sem consultar a nuvem no mesmo servidor travado.
    #[test]
    fn llm_that_accepts_and_never_answers_is_a_timeout_warning() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = server.local_addr().expect("endereço");
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = std::sync::Arc::clone(&done);
        let held = std::thread::spawn(move || {
            // Segura as conexões abertas, sem responder.
            let mut open = Vec::new();
            server.set_nonblocking(true).expect("nonblocking");
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < until
                && !stop.load(std::sync::atomic::Ordering::SeqCst)
            {
                if let Ok((stream, _)) = server.accept() {
                    open.push(stream);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            open.len()
        });
        let mut config = Config::default();
        config.llm.endpoint = format!("http://{address}");
        let mut r = Report::default();
        let started = std::time::Instant::now();
        check_llm_within(&mut r, &config, Duration::from_millis(300));
        let took = started.elapsed();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(took < Duration::from_secs(3), "{took:?}");
        let llm: Vec<_> = r.checks.iter().filter(|c| c.item == "llm").collect();
        assert_eq!(llm.len(), 1, "{:?}", r.checks);
        assert_eq!(llm[0].level, Level::Warn);
        assert_eq!(llm[0].detail, "sem resposta do LLM em 1 s");
        assert!(r.checks.iter().all(|c| c.item != "nuvem"), "{:?}", r.checks);
        assert_eq!(held.join().expect("servidor"), 1, "uma conexão só");
    }

    /// Ollama ausente (nada escutando): um aviso só, sem a dica de
    /// "atualizar o Ollama" da nuvem.
    #[test]
    fn missing_ollama_is_one_warning() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("porta livre")
            .port();
        let mut config = Config::default();
        config.llm.endpoint = format!("http://127.0.0.1:{port}");
        let mut r = Report::default();
        check_llm_within(&mut r, &config, Duration::from_millis(500));
        let llm: Vec<_> = r
            .checks
            .iter()
            .filter(|c| c.item == "llm" || c.item == "nuvem")
            .collect();
        assert_eq!(llm.len(), 1, "{:?}", r.checks);
        assert_eq!((llm[0].item.as_str(), llm[0].level), ("llm", Level::Warn));
        assert!(
            llm[0]
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("llm.enabled false")
        );
        assert_eq!(r.failures(), 0);
    }

    fn gpu_check(gpu: &str, model: &str, names: &[&str]) -> Check {
        let probe = if gpu == "cpu" {
            Probe::Skipped
        } else {
            Probe::Usable(names.iter().map(|n| (*n).to_owned()).collect())
        };
        gpu_check_with(gpu, model, &probe)
    }

    fn gpu_check_with(gpu: &str, model: &str, probe: &Probe) -> Check {
        let mut config = Config::default();
        config.asr.gpu = gpu.into();
        config.asr.model = format!("/modelos/{model}");
        let mut r = Report::default();
        check_gpu(&mut r, &config, probe);
        assert_eq!(r.checks.len(), 1);
        r.checks.remove(0)
    }

    /// AMD, Intel e só CPU: nada pressupõe NVIDIA, e a dica do modelo troca
    /// a config (`models use`), não só baixa.
    #[test]
    fn gpu_check_without_nvidia() {
        const SMALL: &str = "ggml-small-q5_1.bin";
        const TURBO: &str = "ggml-large-v3-turbo-q5_0.bin";
        let amd = gpu_check("auto", TURBO, &["AMD Radeon RX 7800 XT (RADV NAVI32)"]);
        assert_eq!(amd.level, Level::Ok);
        assert!(amd.detail.starts_with("AMD Radeon"), "{amd:?}");
        let intel = gpu_check("auto", TURBO, &["Intel(R) Arc(tm) Graphics (MTL)"]);
        assert_eq!(intel.level, Level::Ok);

        let cpu = gpu_check("auto", SMALL, &[]);
        assert_eq!(
            (cpu.level, cpu.detail.as_str()),
            (Level::Warn, "nenhuma GPU Vulkan: transcrição na CPU")
        );
        let slow = gpu_check("auto", TURBO, &[]);
        assert_eq!(slow.level, Level::Warn);
        assert!(
            slow.hint
                .as_deref()
                .unwrap_or_default()
                .contains("models use small-q5_1")
        );
        let forced = gpu_check("cpu", TURBO, &["AMD Radeon RX 7800 XT (RADV NAVI32)"]);
        assert_eq!(forced.level, Level::Warn);
        assert_eq!(
            gpu_check("cpu", SMALL, &["Intel(R) Arc(tm) Graphics"]).level,
            Level::Ok
        );
        assert_eq!(gpu_check("RTX", SMALL, &["AMD Radeon"]).level, Level::Fail);

        for check in [amd, intel, cpu, slow, forced] {
            let text = format!("{} {}", check.detail, check.hint.unwrap_or_default());
            assert!(!text.contains("RTX") && !text.contains("GeForce"), "{text}");
        }
    }

    /// Driver Vulkan que travou ou derrubou a sondagem: aviso com o código do
    /// motivo, nunca falha (o ditado segue na CPU), mesmo com GPU nomeada.
    #[test]
    fn gpu_check_with_a_broken_driver() {
        const TURBO: &str = "ggml-large-v3-turbo-q5_0.bin";
        for (gpu, failure) in [
            ("auto", Failure::Timeout),
            ("auto", Failure::Signal(6)),
            ("Radeon", Failure::Exit(1)),
        ] {
            let check = gpu_check_with(gpu, TURBO, &Probe::Broken(failure.clone()));
            assert_eq!(check.level, Level::Warn, "{check:?}");
            assert_eq!(
                check.detail,
                format!("driver Vulkan falhou na sondagem ({failure}): transcrição na CPU")
            );
            let hint = check.hint.unwrap_or_default();
            assert!(hint.contains("fastwisper restart"), "{hint}");
            assert_eq!(
                crate::i18n::translate(&check.detail, crate::config::UiLanguage::En),
                format!("Vulkan driver failed the probe ({failure}): transcription uses the CPU")
            );
            assert_ne!(
                crate::i18n::translate(&hint, crate::config::UiLanguage::En),
                hint
            );
        }
        // Sem driver nenhum continua o aviso de antes; `cpu` não sondou.
        assert_eq!(
            gpu_check_with("auto", "ggml-small-q5_1.bin", &Probe::NoVulkan).detail,
            "nenhuma GPU Vulkan: transcrição na CPU"
        );
        assert_eq!(
            gpu_check_with("cpu", "ggml-small-q5_1.bin", &Probe::Skipped).level,
            Level::Ok
        );
    }

    #[test]
    fn empty_device_is_a_failure() {
        let mut r = Report::default();
        let mut config = Config::default();
        config.audio.device = "  ".into();
        assert!(!check_microphone_configured(&mut r, &config));
        assert_eq!(r.checks[0].level, Level::Fail);
        config.audio.device = "usb-mic".into();
        assert!(check_microphone_configured(&mut Report::default(), &config));
    }
}
