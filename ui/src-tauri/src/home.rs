//! Início: estado do daemon ao vivo (`subscribe`), VRAM, último ditado e
//! avisos do doctor. Tudo projetado em DTOs fechados: estado e métricas,
//! nunca texto ditado, título de janela ou a `Response` inteira.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fw_core::client;
use fw_core::config::Config;
use fw_core::daemon::DaemonPaths;
use fw_core::doctor::{self, Level};
use fw_core::ipc::{Request, Response};
use fw_core::pipeline::{LastSummary, OutcomeKind, Readiness};
use fw_core::session::{Command, Mode, StateName};
use fw_core::vram::{ModelState, Profile, VramMode, VramStatus};
use fw_core::vram_switch::{ModeSwitch, switch_mode};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// Evento com o [`DaemonView`] novo a cada mudança.
pub const DAEMON_EVENT: &str = "daemon-status";
const RECONNECT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DaemonView {
    /// O daemon respondeu (`false` = parado ou socket ausente).
    available: bool,
    state: Option<StateName>,
    /// Gravação em modo mãos livres (`toggle`).
    hands_free: bool,
    note_session: bool,
    asr: Option<Readiness>,
    vram: Option<VramView>,
    last: Option<LastView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct VramView {
    mode: VramMode,
    profile: Profile,
    game: bool,
    asr: ModelState,
    llm: ModelState,
    vram_mb: Option<u64>,
    unload_pending: bool,
}

/// Último ditado: desfecho e métricas. O daemon nem manda o texto no
/// `subscribe`; mesmo assim nada aqui teria onde guardá-lo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LastView {
    /// `transcribed`, `no_speech`, `too_short` ou o código da falha.
    outcome: String,
    failed: bool,
    /// Aviso pt-BR da falha (o mesmo da notificação).
    message: Option<&'static str>,
    language: Option<&'static str>,
    words: Option<u64>,
    audio_ms: u64,
    speech_ms: Option<u64>,
    asr_ms: Option<u64>,
    llm: Option<String>,
    llm_ms: Option<u64>,
    e2e_ms: Option<u64>,
    load_ms: Option<u64>,
    llm_load_ms: Option<u64>,
    profile: Option<Profile>,
    /// `pasted`, `copied` ou `failed`.
    delivery: Option<String>,
    /// Motivo da cópia (`focus_changed`, `app_not_approved`...).
    delivery_reason: Option<String>,
}

fn enum_name<T: Serialize>(value: T) -> Option<String> {
    serde_json::to_value(value)
        .ok()?
        .as_str()
        .map(str::to_owned)
}

impl VramView {
    fn from_status(v: &VramStatus) -> VramView {
        VramView {
            mode: v.mode,
            profile: v.profile,
            game: v.game,
            asr: v.asr,
            llm: v.llm,
            vram_mb: v.vram_mb,
            unload_pending: v.unload_pending,
        }
    }
}

impl LastView {
    fn from_summary(last: &LastSummary) -> LastView {
        let m = &last.metrics;
        let failure = match last.outcome {
            OutcomeKind::Failed(f) => Some(f),
            OutcomeKind::Done(_) => None,
        };
        let delivery = last
            .delivery
            .and_then(|d| serde_json::to_value(d).ok())
            .unwrap_or_default();
        LastView {
            outcome: last.outcome.as_str(),
            failed: failure.is_some(),
            message: failure.map(|f| f.user_message()),
            language: last.language.map(|l| l.code()),
            words: m.words,
            audio_ms: m.audio_ms,
            speech_ms: m.speech_ms,
            asr_ms: m.asr_ms,
            llm: m.llm.and_then(enum_name),
            llm_ms: m.llm_ms,
            e2e_ms: m.e2e_ms,
            load_ms: m.load_ms,
            llm_load_ms: m.llm_load_ms,
            profile: m.profile,
            delivery: delivery["delivery"].as_str().map(str::to_owned),
            delivery_reason: delivery["reason"].as_str().map(str::to_owned),
        }
    }
}

impl DaemonView {
    fn offline() -> DaemonView {
        DaemonView::default()
    }

    /// Projeção de uma resposta de `status`/`subscribe`.
    fn from_response(r: &Response) -> DaemonView {
        DaemonView {
            available: r.ok,
            state: r.state,
            hands_free: r.mode == Some(Mode::Toggle),
            note_session: r.note_session,
            asr: r.asr,
            vram: r.vram.as_ref().map(VramView::from_status),
            last: r.last.as_ref().map(LastView::from_summary),
        }
    }
}

fn request_id(what: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("hub-{what}-{}-{nanos}", std::process::id())
}

/// Uma assinatura: repassa cada evento até o daemon fechar a conexão ou
/// `on_view` pedir para parar (`false`).
fn follow_once(socket: &Path, on_view: &mut impl FnMut(DaemonView) -> bool) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return true;
    };
    if stream
        .write_all(&Request::new(request_id("subscribe"), Command::Subscribe).to_line())
        .is_err()
    {
        return true;
    }
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(response) = serde_json::from_str::<Response>(&line) else {
            continue;
        };
        if !on_view(DaemonView::from_response(&response)) {
            return false;
        }
    }
    true
}

/// Segue o daemon para sempre, como `fastwisper status --follow`: parado
/// vira um único `offline` e uma nova tentativa a cada 2 s.
fn follow(socket: &Path, mut on_view: impl FnMut(DaemonView) -> bool) {
    let mut offline_sent = false;
    loop {
        let mut connected = false;
        let keep = follow_once(socket, &mut |view| {
            connected = true;
            offline_sent = false;
            on_view(view)
        });
        if !keep {
            return;
        }
        if (connected || !offline_sent) && !on_view(DaemonView::offline()) {
            return;
        }
        offline_sent = true;
        std::thread::sleep(RECONNECT);
    }
}

/// Último estado visto, para a página pedir ao montar.
#[derive(Default)]
pub struct Live(Arc<Mutex<DaemonView>>);

/// Uma assinatura por processo do hub, aberta ao iniciar; as páginas leem
/// o último valor e escutam [`DAEMON_EVENT`].
pub fn start_watcher(app: AppHandle, live: &Live) {
    let latest = Arc::clone(&live.0);
    let Ok(paths) = DaemonPaths::from_env() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("fastwisper-subscribe".into())
        .spawn(move || {
            follow(&paths.socket, |view| {
                if let Ok(mut slot) = latest.lock() {
                    *slot = view.clone();
                }
                let _ = app.emit(DAEMON_EVENT, view);
                true
            });
        });
}

#[tauri::command]
pub fn daemon_view(live: State<'_, Live>) -> DaemonView {
    live.0.lock().map(|v| v.clone()).unwrap_or_default()
}

fn send(command: Command) -> Result<Response, String> {
    let paths = DaemonPaths::from_env().map_err(|_| "Runtime do daemon indisponível")?;
    let response = client::send(&paths.socket, &Request::new(request_id("cmd"), command)).map_err(
        |error| match error {
            client::ClientError::NotRunning(_) => "O FastWisper está parado".to_owned(),
            _ => "Falha de comunicação com o daemon".to_owned(),
        },
    )?;
    if response.ok {
        Ok(response)
    } else {
        Err(response
            .error
            .map_or_else(|| "O daemon recusou o pedido".to_owned(), |e| e.message))
    }
}

/// "Liberar VRAM": descarrega os modelos agora (o daemon adia até o fim do
/// ditado em andamento). O estado novo chega pelo `subscribe`.
#[tauri::command]
pub async fn release_vram() -> Result<Option<VramView>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        send(Command::Unload).map(|r| r.vram.as_ref().map(VramView::from_status))
    })
    .await
    .map_err(|_| "Falha ao pedir a descarga".to_owned())?
}

/// Troca o modo de VRAM como `fastwisper vram <modo>`: aplica no daemon e
/// salva `[performance] vram` (mesma função da CLI). Devolve o aviso pt-BR.
#[tauri::command]
pub async fn set_vram_mode(mode: String) -> Result<String, String> {
    let mode = VramMode::from_name(&mode).ok_or("Modo de VRAM inválido")?;
    tauri::async_runtime::spawn_blocking(move || {
        let paths = DaemonPaths::from_env().map_err(|_| "Runtime do daemon indisponível")?;
        let config = Config::default_path().map_err(|e| e.to_string())?;
        switch(&paths.socket, &config, mode)
    })
    .await
    .map_err(|_| "Falha ao trocar o modo".to_owned())?
}

fn switch(socket: &Path, config: &Path, mode: VramMode) -> Result<String, String> {
    let result = switch_mode(socket, config, mode, &request_id("vram"))
        .map_err(|error| error_chain(&error))?;
    Ok(match result {
        ModeSwitch::Saved => {
            "Modo salvo. O FastWisper está parado e usa esse modo no próximo início.".to_owned()
        }
        ModeSwitch::Applied { reload, .. } => {
            let restart = reload.map(|r| r.restart_required).unwrap_or_default();
            if restart.is_empty() {
                "Modo aplicado e salvo na configuração.".to_owned()
            } else {
                format!(
                    "Modo aplicado e salvo. Outras mudanças do arquivo só valem depois de reiniciar o FastWisper: {}.",
                    restart.join(", ")
                )
            }
        }
    })
}

/// "perfil aplicado no daemon, mas não foi possível salvar …: motivo".
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Aviso ou falha do doctor, com a ação sugerida.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notice {
    item: String,
    level: Level,
    detail: String,
    hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoctorSummary {
    verdict: String,
    ok: usize,
    /// Só avisos e falhas; o que está certo vira contagem.
    notices: Vec<Notice>,
}

fn summarize(report: &doctor::Report) -> DoctorSummary {
    let notices = report
        .checks
        .iter()
        .filter(|c| c.level != Level::Ok)
        .map(|c| Notice {
            item: c.item.clone(),
            level: c.level,
            detail: c.detail.clone(),
            hint: c.hint.clone(),
        })
        .collect();
    DoctorSummary {
        verdict: report.verdict(),
        ok: report
            .checks
            .iter()
            .filter(|c| c.level == Level::Ok)
            .count(),
        notices,
    }
}

/// As checagens do `fastwisper doctor` que não dependem de PipeWire nem de
/// GPU. Só lê; o único acesso de rede é o Ollama local.
#[tauri::command]
pub async fn doctor_summary() -> Result<DoctorSummary, String> {
    tauri::async_runtime::spawn_blocking(|| summarize(&doctor::run_portable(None)))
        .await
        .map_err(|_| "Falha ao conferir o FastWisper".to_owned())
}

/// Maior VRAM informada pelo sistema, em MiB: `mem_info_vram_total` das
/// placas no sysfs (AMD, Intel) ou a saída de `nvidia-smi`.
fn total_vram_mb(drm: &Path, nvidia_smi: &str) -> Option<u64> {
    let sysfs = std::fs::read_dir(drm)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let text =
                std::fs::read_to_string(entry.ok()?.path().join("device/mem_info_vram_total"))
                    .ok()?;
            Some(text.trim().parse::<u64>().ok()? >> 20)
        });
    let nvidia = std::process::Command::new(nvidia_smi)
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    sysfs
        .chain(
            nvidia
                .lines()
                .filter_map(|line| line.trim().parse::<u64>().ok()),
        )
        .filter(|&mb| mb > 0)
        .max()
}

/// VRAM total para a barra do cartão de estado (lida uma vez). `None` quando
/// o sistema não informa: a barra fica vazia.
#[tauri::command]
pub async fn vram_total() -> Option<u64> {
    static TOTAL: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    tauri::async_runtime::spawn_blocking(|| {
        *TOTAL.get_or_init(|| total_vram_mb(Path::new("/sys/class/drm"), "nvidia-smi"))
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    use fw_core::asr::Language;
    use fw_core::daemon::Daemon;
    use fw_core::inject::{CopyReason, Delivery, Desktop, Target};
    use fw_core::pipeline::{Failure, Metrics, Outcome, Pipeline};
    use fw_core::session::Ticket;

    use super::*;

    const MARKER: &str = "marcador-sintetico-hub-7f3a";
    const TITLE: &str = "titulo-privado-da-janela";

    /// Transcreve o marcador; o resto do pipeline é o padrão (pronto, sem VRAM).
    struct FakePipeline;

    impl Pipeline for FakePipeline {
        fn start(
            &self,
            _ticket: Ticket,
        ) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
            std::future::ready(Ok(()))
        }

        fn finish(
            &self,
            _ticket: Ticket,
            _literal: bool,
        ) -> impl Future<Output = Outcome> + Send + 'static {
            std::future::ready(Outcome::Transcribed {
                text: MARKER.into(),
                language: Language::Pt,
                submit: false,
                metrics: Metrics {
                    audio_ms: 2400,
                    asr_ms: Some(180),
                    words: Some(4),
                    ..Metrics::default()
                },
            })
        }

        fn abort(&self, _ticket: Ticket) {}
    }

    /// Janela com título privado: não pode chegar ao hub.
    struct FakeDesktop;

    impl Desktop for FakeDesktop {
        async fn focused(&self) -> Option<Target> {
            Some(Target {
                address: "0x1".into(),
                stable_id: Some("1".into()),
                pid: 42,
                class: TITLE.into(),
                initial_class: TITLE.into(),
                process: Some("editor".into()),
                tags: vec![],
                terminal: false,
                harness: false,
            })
        }
        async fn deliver(&self, _text: String, _target: Option<Target>, _submit: bool) -> Delivery {
            Delivery::Copied(CopyReason::FocusChanged)
        }
        async fn copy(&self, _text: String) -> Delivery {
            Delivery::Failed
        }
        async fn paste_focused(&self, _text: String) -> Delivery {
            Delivery::Failed
        }
        fn notify(&self, _message: &str) {}
    }

    fn command(socket: &Path, cmd: Command) -> Response {
        client::send(socket, &Request::new(request_id("test"), cmd)).expect("daemon fake")
    }

    fn next(
        views: &mpsc::Receiver<DaemonView>,
        wanted: impl Fn(&DaemonView) -> bool,
    ) -> DaemonView {
        loop {
            let view = views
                .recv_timeout(Duration::from_secs(5))
                .expect("evento do subscribe");
            if wanted(&view) {
                return view;
            }
        }
    }

    #[test]
    fn follows_the_fake_daemon_with_only_state_and_metrics() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        let paths = DaemonPaths::in_dir(dir.path());
        let _runtime_context = runtime.enter();
        let daemon = Daemon::bind(paths.clone()).expect("bind");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server =
            runtime.spawn(
                daemon.run(Arc::new(FakePipeline), Arc::new(FakeDesktop), async {
                    let _ = stopped.await;
                }),
            );

        let (tx, views) = mpsc::channel();
        let socket = paths.socket.clone();
        std::thread::spawn(move || follow(&socket, move |view| tx.send(view).is_ok()));

        let idle = next(&views, |v| v.available);
        assert_eq!(idle.state, Some(StateName::Idle));
        assert_eq!(idle.asr, Some(Readiness::Ready));
        assert!(idle.vram.is_some(), "perfil e VRAM vêm no subscribe");

        command(&paths.socket, Command::Press);
        next(&views, |v| v.state == Some(StateName::Recording));
        command(&paths.socket, Command::Cancel);
        next(&views, |v| v.state == Some(StateName::Idle));

        // Fake de ponta a ponta (desktop falso): o texto existe no daemon.
        command(&paths.socket, Command::Press);
        command(&paths.socket, Command::Release);
        let done = next(&views, |v| v.last.is_some());
        let last = done.last.clone().expect("último ditado");
        assert_eq!(last.outcome, "transcribed");
        assert_eq!(
            (last.words, last.asr_ms, last.language),
            (Some(4), Some(180), Some("pt"))
        );
        assert!(last.e2e_ms.is_some(), "o daemon mede release → entrega");
        assert_eq!(last.delivery.as_deref(), Some("copied"));
        assert_eq!(last.delivery_reason.as_deref(), Some("focus_changed"));
        let json = serde_json::to_string(&done).expect("json");
        assert!(!json.contains(MARKER), "texto ditado vazou: {json}");
        assert!(!json.contains(TITLE), "janela vazou: {json}");

        // `unload` responde com a VRAM; daemon parado vira `offline`.
        let unloaded = command(&paths.socket, Command::Unload);
        assert!(unloaded.ok && unloaded.vram.is_some());

        // Troca de modo: aplica no daemon, salva preservando comentários e
        // chega ao Início pelo `subscribe`.
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "# minha config\n[performance]\nvram = \"automatico\" # meu\n",
        )
        .expect("config");
        let message = switch(&paths.socket, &config, VramMode::Desempenho).expect("troca");
        assert!(message.starts_with("Modo aplicado e salvo"), "{message}");
        let switched = next(&views, |v| {
            v.vram.is_some_and(|vram| vram.mode == VramMode::Desempenho)
        });
        assert_eq!(switched.vram.map(|v| v.profile), Some(Profile::Desempenho));
        let saved = std::fs::read_to_string(&config).expect("config");
        assert_eq!(
            saved,
            "# minha config\n[performance]\nvram = \"desempenho\" # meu\n"
        );
        stop.send(()).expect("parar");
        runtime.block_on(server).expect("daemon");
        assert_eq!(next(&views, |v| !v.available), DaemonView::offline());
    }

    #[test]
    fn note_status_projects_only_the_session_marker() {
        let mut session = fw_core::session::Session::new();
        session.handle(Command::NoteStart).expect("nota");
        let mut response = Response::ok("note-status".into(), session.snapshot());
        response.note_session = true;
        response.notes = Some(vec![fw_core::notes::Note {
            id: 1,
            ts: 1,
            text: MARKER.into(),
            language: "pt".into(),
            words: 1,
        }]);
        let projected = DaemonView::from_response(&response);
        assert!(projected.note_session);
        assert!(!projected.hands_free);
        assert!(
            !serde_json::to_string(&projected)
                .expect("json")
                .contains(MARKER)
        );
    }

    #[test]
    fn failure_carries_the_user_message_and_no_delivery() {
        let summary = Outcome::Failed(Failure::MicSilent, Metrics::default()).summary();
        let view = LastView::from_summary(&summary);
        assert!(view.failed);
        assert_eq!(view.outcome, "mic_silent");
        assert_eq!(view.message, Some(Failure::MicSilent.user_message()));
        assert_eq!(view.delivery, None);
    }

    #[test]
    fn doctor_summary_keeps_only_notices_with_actions() {
        let mut report = doctor::Report::default();
        report.ok("config", "padrões");
        report.warn("llm", "Ollama não respondeu", "ditado segue só com regras");
        let summary = summarize(&report);
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.verdict, "Funciona, com 1 aviso(s).");
        assert_eq!(summary.notices.len(), 1);
        assert_eq!(
            summary.notices[0].hint.as_deref(),
            Some("ditado segue só com regras")
        );
    }

    #[test]
    fn total_vram_takes_the_largest_card_from_sysfs_or_nvidia_smi() {
        let dir = tempfile::tempdir().expect("temporário");
        let missing = dir.path().join("sem-nvidia-smi");
        let missing = missing.to_str().expect("caminho");
        assert_eq!(total_vram_mb(dir.path(), missing), None);
        for (card, bytes) in [
            ("card0", "536870912\n"),
            ("card1", "17179869184\n"),
            ("card1-DP-1", "lixo"),
        ] {
            std::fs::create_dir_all(dir.path().join(card).join("device")).expect("card");
            std::fs::write(
                dir.path().join(card).join("device/mem_info_vram_total"),
                bytes,
            )
            .expect("vram");
        }
        assert_eq!(total_vram_mb(dir.path(), missing), Some(16384));
        let smi = dir.path().join("nvidia-smi");
        crate::test_support::write_executable(&smi, "#!/bin/sh\nprintf '16303\\n8188\\n'\n");
        let empty = dir.path().join("vazio");
        std::fs::create_dir(&empty).expect("vazio");
        assert_eq!(
            total_vram_mb(&empty, smi.to_str().expect("caminho")),
            Some(16303)
        );
    }
}
