//! Contrato do `reload_config`: aplica na hora o
//! que é seguro, informa o que exige reinício, recusa config inválida
//! mantendo a anterior e nunca recarrega no meio de um ditado.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{FakeDesktop, GatedPipeline, TestDaemon};
use fw_core::config::{Config, Style};
use fw_core::ipc::{ErrorCode, Request, Response};
use fw_core::pipeline::{Failure, Outcome, Pipeline};
use fw_core::session::{Command, StateName, Ticket};
use fw_core::vram::{ModelOp, VramMode};

/// Pipeline com processamento controlado que registra cada recarga.
#[derive(Default)]
struct ReloadPipeline {
    inner: GatedPipeline,
    reconfigured: Mutex<Vec<Config>>,
}

impl ReloadPipeline {
    fn reconfigured(&self) -> Vec<Config> {
        self.reconfigured.lock().expect("lock").clone()
    }
}

impl Pipeline for ReloadPipeline {
    fn start(&self, ticket: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        self.inner.start(ticket)
    }
    fn finish(
        &self,
        ticket: Ticket,
        literal: bool,
    ) -> impl Future<Output = Outcome> + Send + 'static {
        self.inner.finish(ticket, literal)
    }
    fn abort(&self, ticket: Ticket) {
        self.inner.abort(ticket);
    }
    fn reconfigure(
        &self,
        _op: ModelOp,
        config: &Config,
    ) -> impl Future<Output = ()> + Send + 'static {
        self.reconfigured.lock().expect("lock").push(config.clone());
        std::future::ready(())
    }
}

const BOOT: &str =
    "# config de teste\n[audio]\ndevice = \"mic-de-teste\" # meu microfone\nsounds = false\n";

struct Setup {
    daemon: TestDaemon,
    pipeline: Arc<ReloadPipeline>,
    desktop: Arc<FakeDesktop>,
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
}

async fn setup() -> Setup {
    let dir = common::runtime_dir();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, BOOT).expect("config");
    let boot = Config::load(Some(&path)).expect("config válida");
    let pipeline = Arc::new(ReloadPipeline::default());
    let desktop = Arc::new(FakeDesktop::default());
    let config_path = path.clone();
    let daemon = TestDaemon::start_configured(Arc::clone(&pipeline), Arc::clone(&desktop), |d| {
        d.with_config(config_path, true, boot)
    })
    .await;
    Setup {
        daemon,
        pipeline,
        desktop,
        _dir: dir,
        path,
    }
}

async fn reload(daemon: &TestDaemon) -> Response {
    daemon.ctl(Command::ReloadConfig).await
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, format!("{BOOT}{text}")).expect("config");
}

async fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "tempo esgotado esperando {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn safe_keys_apply_now_and_changes_are_reported_by_name() {
    let s = setup().await;
    write(
        &s.path,
        "[llm]\nstyle = \"formal\"\n[performance]\nvram = \"desempenho\"\n[inject]\nnotify = false\n",
    );
    let response = reload(&s.daemon).await;
    assert!(response.ok, "{response:?}");
    let report = response.reload.clone().expect("relatório");
    assert_eq!(
        report.changed,
        ["inject.notify", "llm.style", "performance.vram"]
    );
    assert!(report.restart_required.is_empty());
    assert!(!report.pending);
    assert_eq!(response.vram.expect("vram").mode, VramMode::Desempenho);

    let applied = s.pipeline.reconfigured();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].llm.style, Style::Formal);
    assert!(!applied[0].inject.notify);
    assert!(
        !s.desktop.reconfigured.lock().expect("lock")[0].notify,
        "[inject] chega ao desktop"
    );
    let status = s.daemon.ctl(Command::Status).await;
    assert_eq!(status.vram.expect("vram").mode, VramMode::Desempenho);
    // Só nomes de chave: o valor (que pode ser pessoal) não vai no IPC.
    let line = String::from_utf8(response.to_line()).expect("utf-8");
    assert!(!line.contains("formal"), "{line}");

    // Sem mudança: recarga vazia.
    let again = reload(&s.daemon).await.reload.expect("relatório");
    assert!(again.changed.is_empty());
    s.daemon.stop().await;
}

#[tokio::test]
async fn restart_keys_are_reported_and_keep_the_boot_value() {
    let s = setup().await;
    write(
        &s.path,
        "[asr]\nmodel = \"/outro/modelo.bin\"\n[llm]\nendpoint = \"http://127.0.0.1:8080\"\ntimeout_ms = 2000\n",
    );
    let report = reload(&s.daemon).await.reload.expect("relatório");
    assert_eq!(report.restart_required, ["asr.model", "llm.endpoint"]);
    assert_eq!(
        report.changed,
        ["asr.model", "llm.endpoint", "llm.timeout_ms"]
    );
    let applied = s.pipeline.reconfigured().pop().expect("recarga");
    let boot = Config::default();
    assert_eq!(applied.asr.model, boot.asr.model, "fica o valor do início");
    assert_eq!(applied.llm.endpoint, boot.llm.endpoint);
    assert_eq!(applied.llm.timeout_ms, 2000, "o resto vale na hora");

    // Continua sendo informado até reiniciar.
    let again = reload(&s.daemon).await.reload.expect("relatório");
    assert_eq!(again.restart_required, ["asr.model", "llm.endpoint"]);
    s.daemon.stop().await;
}

#[tokio::test]
async fn invalid_config_is_refused_and_the_previous_one_stays() {
    let s = setup().await;
    write(&s.path, "[llm]\nstyle = \"formal\"\n");
    assert!(reload(&s.daemon).await.ok);
    for broken in [
        "[hotkey]\nmax_recording_secs = 0\n",
        "[privacy]\nstore_txt = true\n",
        "[overlay]\nposition = \"top\"\n",
        "[llm\n",
    ] {
        write(&s.path, broken);
        let response = reload(&s.daemon).await;
        assert!(!response.ok, "{broken}");
        assert_eq!(response.error_code(), Some(ErrorCode::InvalidConfig));
        assert!(response.reload.is_none());
        let message = response.error.expect("erro").message;
        assert!(message.contains("config inválida"), "{message}");
    }
    assert_eq!(s.pipeline.reconfigured().len(), 1, "nada novo aplicado");

    // A anterior segue valendo: voltar a ela não muda nada.
    write(&s.path, "[llm]\nstyle = \"formal\"\n");
    let report = reload(&s.daemon).await.reload.expect("relatório");
    assert!(report.changed.is_empty(), "{report:?}");

    // `--config` explícito que sumiu também é recusado.
    std::fs::remove_file(&s.path).expect("rm");
    assert_eq!(
        reload(&s.daemon).await.error_code(),
        Some(ErrorCode::InvalidConfig)
    );
    s.daemon.stop().await;
}

#[tokio::test]
async fn reload_waits_for_the_dictation_in_progress() {
    let s = setup().await;
    assert_eq!(
        s.daemon.ctl(Command::Press).await.state,
        Some(StateName::Recording)
    );
    write(&s.path, "[llm]\nstyle = \"formal\"\n");
    let report = reload(&s.daemon).await.reload.expect("relatório");
    assert!(report.pending, "gravando: fica para depois");
    assert_eq!(report.changed, ["llm.style"]);
    // Uma recarga nova durante o ditado substitui a pendente.
    write(&s.path, "[llm]\nstyle = \"excited\"\n");
    assert!(reload(&s.daemon).await.reload.expect("relatório").pending);

    assert_eq!(
        s.daemon.ctl(Command::Release).await.state,
        Some(StateName::Transcribing)
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        s.pipeline.reconfigured().is_empty(),
        "nunca no meio de um ditado"
    );

    s.pipeline.inner.release_one();
    wait_until("a recarga depois do ditado", || {
        !s.pipeline.reconfigured().is_empty()
    })
    .await;
    let applied = s.pipeline.reconfigured();
    assert_eq!(applied.len(), 1, "só a última recarga");
    assert_eq!(applied[0].llm.style, Style::Excited);
    assert_eq!(
        s.daemon.ctl(Command::Status).await.state,
        Some(StateName::Idle)
    );
    s.daemon.stop().await;
}

#[tokio::test]
async fn daemon_without_a_config_file_refuses_reload() {
    let daemon = TestDaemon::start(Arc::new(GatedPipeline::default())).await;
    let response = reload(&daemon).await;
    assert_eq!(response.error_code(), Some(ErrorCode::Unavailable));
    daemon.stop().await;
}

#[tokio::test]
async fn reload_config_takes_no_arguments() {
    let daemon = TestDaemon::start(Arc::new(GatedPipeline::default())).await;
    let lines = common::exchange(
        &daemon.paths.socket,
        &[&common::line(
            r#"{"v":1,"request_id":"r1","cmd":"reload_config","args":{"mode":"jogo"}}"#,
        )],
    )
    .await;
    assert_eq!(lines[0]["error"]["code"], "invalid_args");
    let ok = common::send(
        &daemon.paths.socket,
        Request::new("r2", Command::ReloadConfig),
    )
    .await;
    assert_eq!(ok.request_id.as_deref(), Some("r2"));
    daemon.stop().await;
}

#[tokio::test]
async fn polisher_applies_rules_style_and_llm_switch_without_restart() {
    use fw_core::llm::LlmOutcome;
    use fw_core::polish::Polisher;
    use fw_core::vram::ModelState;

    // LLM desligado no início: só regras.
    let boot = Config::parse("[llm]\nenabled = false\n", Path::new("c.toml")).expect("config");
    let polisher = Polisher::new(&boot);
    let out = polisher
        .polish("vou usar o fast whisper", None, false)
        .await;
    assert_eq!(out.llm, LlmOutcome::Disabled);
    assert_eq!(polisher.llm_state(), ModelState::Disabled);

    // Dicionário novo vale no próximo texto; ligar o LLM cria o cliente.
    let new = Config::parse(
        "[dictionary]\nreplace = { \"fast whisper\" = \"FastWisper\", \"omar\" = \"Omarchy\" }\n",
        Path::new("c.toml"),
    )
    .expect("config");
    polisher
        .reconfigure(
            ModelOp {
                seq: 1,
                ticket: None,
            },
            &new,
        )
        .await;
    assert!(polisher.llm_enabled());
    assert_eq!(polisher.llm_state(), ModelState::Unloaded);
    let out = polisher.polish("omar", None, false).await;
    assert_eq!(out.text, "Omarchy");
    assert_eq!(out.llm, LlmOutcome::SkippedShort);

    // E desligar volta a só regras.
    polisher
        .reconfigure(
            ModelOp {
                seq: 2,
                ticket: None,
            },
            &boot,
        )
        .await;
    assert!(!polisher.llm_enabled());
    assert_eq!(polisher.llm_state(), ModelState::Disabled);
}

#[tokio::test]
async fn ui_language_reloads_notifications_without_changing_ipc_codes_or_transcription() {
    use fw_core::config::UiLanguage;
    let s = setup().await;
    wait_until("idioma inicial", || {
        !s.desktop
            .languages
            .lock()
            .expect("fixture de idioma válida")
            .is_empty()
    })
    .await;
    assert_eq!(
        *s.desktop
            .languages
            .lock()
            .expect("fixture de idioma válida"),
        [UiLanguage::En]
    );
    let wire_before = s.daemon.ctl(Command::CopyLast).await;
    write(&s.path, "[ui]\nlanguage = \"pt-BR\"\n");
    let response = reload(&s.daemon).await;
    let report = response.reload.expect("fixture de idioma válida");
    assert_eq!(report.changed, ["ui.language"]);
    assert!(report.restart_required.is_empty());
    wait_until("idioma recarregado", || {
        s.desktop
            .languages
            .lock()
            .expect("fixture de idioma válida")
            .last()
            == Some(&UiLanguage::PtBr)
    })
    .await;
    let wire_after = s.daemon.ctl(Command::CopyLast).await;
    assert_eq!(wire_before.error, wire_after.error);
    assert_eq!(
        wire_after.error.expect("fixture de idioma válida").code,
        ErrorCode::NoResult
    );
    assert_eq!(
        Config::load(Some(&s.path))
            .expect("fixture de idioma válida")
            .asr
            .languages,
        Config::default().asr.languages
    );
    assert_eq!(
        Config::load(Some(&s.path))
            .expect("fixture de idioma válida")
            .inject
            .send_now_phrases,
        Config::default().inject.send_now_phrases
    );
    s.daemon.stop().await;
}
