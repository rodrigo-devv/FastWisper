//! Apoio dos testes de contrato: daemon em diretório temporário e um
//! pipeline falso cujo processamento só termina quando o teste libera.

#![allow(dead_code)]

#[cfg(feature = "pipewire")]
pub mod virtual_mic;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fw_core::asr::Language;
use fw_core::client;
use fw_core::config::InjectConfig;
use fw_core::daemon::{Daemon, DaemonPaths};
use fw_core::inject::{CopyReason, Delivery, Desktop, Target};
use fw_core::ipc::{Request, Response};
use fw_core::pipeline::{Failure, Metrics, Outcome, Pipeline};
use fw_core::session::{Command, Ticket};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinHandle;

/// Texto sintético devolvido pelo pipeline falso; serve de marcador para
/// provar que conteúdo não vaza para status/logs.
pub const MARKER_TEXT: &str = "marcador-sintetico-7f3a";

/// Pipeline cujo `finish` fica preso até `release_one` ou `abort`.
pub struct GatedPipeline {
    pub started: AtomicUsize,
    pub finishing: AtomicUsize,
    pub aborted: AtomicUsize,
    gate: Arc<Semaphore>,
    refuse_start: Mutex<Option<Failure>>,
    outcome: Mutex<Option<Outcome>>,
    pub max_recording: Mutex<Duration>,
    /// `literal` recebido em cada `finish`.
    pub literal: Mutex<Vec<bool>>,
    /// Nível do microfone devolvido em `level` (bits de f32).
    level: AtomicU32,
}

impl Default for GatedPipeline {
    fn default() -> Self {
        Self {
            started: AtomicUsize::new(0),
            finishing: AtomicUsize::new(0),
            aborted: AtomicUsize::new(0),
            gate: Arc::new(Semaphore::new(0)),
            refuse_start: Mutex::new(None),
            outcome: Mutex::new(None),
            max_recording: Mutex::new(Duration::from_secs(300)),
            literal: Mutex::new(Vec::new()),
            level: AtomicU32::new(0),
        }
    }
}

impl GatedPipeline {
    pub fn release_one(&self) {
        self.gate.add_permits(1);
    }

    /// Próximos `start` falham com `failure` (microfone ausente etc.).
    pub fn refuse_start(&self, failure: Option<Failure>) {
        *self.refuse_start.lock().expect("lock") = failure;
    }

    /// Nível que o "microfone" reporta (mesmo fora da gravação: quem
    /// filtra é o daemon).
    pub fn set_level(&self, level: f32) {
        self.level.store(level.to_bits(), Ordering::SeqCst);
    }

    /// Resultado dos próximos `finish` (padrão: transcreve [`MARKER_TEXT`]).
    pub fn set_outcome(&self, outcome: Outcome) {
        *self.outcome.lock().expect("lock") = Some(outcome);
    }
}

impl Pipeline for GatedPipeline {
    fn start(&self, _ticket: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        let refused = *self.refuse_start.lock().expect("lock");
        if refused.is_none() {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
        std::future::ready(refused.map_or(Ok(()), Err))
    }

    fn finish(
        &self,
        _ticket: Ticket,
        literal: bool,
    ) -> impl Future<Output = Outcome> + Send + 'static {
        self.literal.lock().expect("lock").push(literal);
        self.finishing.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&self.gate);
        let outcome = self
            .outcome
            .lock()
            .expect("lock")
            .clone()
            .unwrap_or(Outcome::Transcribed {
                text: MARKER_TEXT.into(),
                language: Language::Pt,
                submit: false,
                metrics: Metrics::default(),
            });
        async move {
            if let Ok(permit) = gate.acquire().await {
                permit.forget();
            }
            outcome
        }
    }

    fn max_recording(&self) -> Duration {
        *self.max_recording.lock().expect("lock")
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::SeqCst))
    }

    fn abort(&self, _ticket: Ticket) {
        self.aborted.fetch_add(1, Ordering::SeqCst);
        // O cancelamento alcança o worker em andamento.
        self.gate.add_permits(1);
    }
}

/// Diretório temporário com as permissões de um `XDG_RUNTIME_DIR` (0700).
pub fn runtime_dir() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    dir
}

/// Desktop falso: janela com foco configurável e registro do que seria
/// copiado/colado e onde.
#[derive(Default)]
pub struct FakeDesktop {
    pub focused: Mutex<Option<Target>>,
    /// (texto, destino capturado no press) de cada entrega automática.
    pub delivered: Mutex<Vec<(String, Option<Target>)>>,
    pub copied: Mutex<Vec<String>>,
    pub pasted_last: Mutex<Vec<String>>,
    pub notices: Mutex<Vec<String>>,
    pub languages: Mutex<Vec<fw_core::config::UiLanguage>>,
    /// Pedido de "enviar agora" de cada `deliver`.
    pub submits: Mutex<Vec<bool>>,
    /// `[inject]` de cada recarga da config.
    pub reconfigured: Mutex<Vec<InjectConfig>>,
}

pub fn window(address: &str) -> Target {
    Target {
        address: address.into(),
        stable_id: Some("1".into()),
        pid: 42,
        class: "ghostty".into(),
        initial_class: "ghostty".into(),
        process: Some("ghostty".into()),
        tags: vec![],
        terminal: false,
        harness: false,
    }
}

impl FakeDesktop {
    pub fn focus(&self, target: Option<Target>) {
        *self.focused.lock().expect("lock") = target;
    }
}

impl Desktop for FakeDesktop {
    fn reconfigure_language(&self, language: fw_core::config::UiLanguage) {
        self.languages
            .lock()
            .expect("fixture de idioma válida")
            .push(language);
    }
    async fn focused(&self) -> Option<Target> {
        self.focused.lock().expect("lock").clone()
    }

    fn notify(&self, message: &str) {
        self.notices.lock().expect("lock").push(message.to_owned());
    }

    async fn deliver(&self, text: String, target: Option<Target>, submit: bool) -> Delivery {
        let now = self.focused.lock().expect("lock").clone();
        let same = matches!((&target, &now), (Some(a), Some(b)) if a.same_window(b));
        self.submits.lock().expect("lock").push(submit);
        self.delivered.lock().expect("lock").push((text, target));
        if same {
            Delivery::Pasted
        } else {
            Delivery::Copied(CopyReason::FocusChanged)
        }
    }

    async fn copy(&self, text: String) -> Delivery {
        self.copied.lock().expect("lock").push(text);
        Delivery::Copied(CopyReason::Requested)
    }

    async fn paste_focused(&self, text: String) -> Delivery {
        self.pasted_last.lock().expect("lock").push(text);
        Delivery::Pasted
    }

    fn reconfigure(&self, config: &InjectConfig) {
        self.reconfigured.lock().expect("lock").push(config.clone());
    }
}

pub struct TestDaemon {
    pub dir: TempDir,
    pub paths: DaemonPaths,
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
}

impl TestDaemon {
    pub async fn start<P: Pipeline>(pipeline: Arc<P>) -> TestDaemon {
        TestDaemon::start_full(
            pipeline,
            Arc::new(FakeDesktop::default()),
            Duration::from_secs(5),
        )
        .await
    }

    pub async fn start_with<P: Pipeline>(pipeline: Arc<P>, read_timeout: Duration) -> TestDaemon {
        TestDaemon::start_full(pipeline, Arc::new(FakeDesktop::default()), read_timeout).await
    }

    pub async fn start_desktop<P: Pipeline, D: Desktop>(
        pipeline: Arc<P>,
        desktop: Arc<D>,
    ) -> TestDaemon {
        TestDaemon::start_full(pipeline, desktop, Duration::from_secs(5)).await
    }

    pub async fn start_full<P: Pipeline, D: Desktop>(
        pipeline: Arc<P>,
        desktop: Arc<D>,
        read_timeout: Duration,
    ) -> TestDaemon {
        TestDaemon::start_configured(pipeline, desktop, |d| d.with_read_timeout(read_timeout)).await
    }

    /// Daemon com ajustes (perfil de VRAM, detector de jogo...).
    pub async fn start_configured<P: Pipeline, D: Desktop>(
        pipeline: Arc<P>,
        desktop: Arc<D>,
        configure: impl FnOnce(Daemon) -> Daemon,
    ) -> TestDaemon {
        let dir = runtime_dir();
        let paths = DaemonPaths::in_dir(dir.path());
        let daemon = configure(Daemon::bind(paths.clone()).expect("bind"));
        let (tx, rx) = oneshot::channel();
        let handle = tokio::spawn(daemon.run(pipeline, desktop, async {
            let _ = rx.await;
        }));
        TestDaemon {
            dir,
            paths,
            shutdown: Some(tx),
            handle,
        }
    }

    /// Para o daemon e espera o encerramento completo.
    pub async fn stop(mut self) -> TempDir {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        tokio::time::timeout(Duration::from_secs(10), &mut self.handle)
            .await
            .expect("daemon não encerrou a tempo")
            .expect("daemon entrou em pânico");
        self.dir
    }

    /// Comando via cliente síncrono da CLI.
    pub async fn ctl(&self, cmd: Command) -> Response {
        ctl(&self.paths.socket, cmd).await
    }
}

pub async fn ctl(socket: &Path, cmd: Command) -> Response {
    let id = format!("test-{}", cmd.as_str());
    send(socket, Request::new(id, cmd)).await
}

/// Requisição qualquer (com argumentos) pelo cliente da CLI.
pub async fn send(socket: &Path, request: Request) -> Response {
    let socket = socket.to_path_buf();
    tokio::task::spawn_blocking(move || client::send(&socket, &request).expect("cliente"))
        .await
        .expect("spawn_blocking")
}

/// Escreve `chunks` (com uma pausa entre eles), fecha a escrita e devolve
/// todas as linhas de resposta até o daemon fechar a conexão.
pub async fn exchange(socket: &Path, chunks: &[&[u8]]) -> Vec<serde_json::Value> {
    let mut stream = UnixStream::connect(socket).await.expect("connect");
    for (i, chunk) in chunks.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        // O daemon pode fechar antes de ler tudo (linha grande demais).
        if stream.write_all(chunk).await.is_err() {
            break;
        }
    }
    let _ = stream.shutdown().await;
    read_lines(&mut stream).await
}

pub async fn read_lines(stream: &mut UnixStream) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    // Fechar com dados não lidos gera ECONNRESET depois da resposta; isso
    // conta como fim da conexão.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    .expect("daemon não respondeu a tempo");
    out.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).expect("resposta é JSON"))
        .collect()
}

pub fn line(json: &str) -> Vec<u8> {
    let mut bytes = json.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
}
