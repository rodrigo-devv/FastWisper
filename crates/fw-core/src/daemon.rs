//! Runtime do `fastwisperd`: single-instance, socket `0600` e o actor que
//! serializa a sessão.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, Permissions};
use std::future::Future;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::fs::OFlags;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{self, JoinSet};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::history::History;
use crate::inject::{Delivery, Desktop, Target};
use crate::ipc::{self, ErrorCode, MAX_LINE_BYTES, ReloadReport, Response};
use crate::pipeline::{Failure, LastResult, LastSummary, Metrics, Outcome, Pipeline, Readiness};
use crate::sandbox::NetworkSandbox;
use crate::session::{Command, Effect, Rejection, Session, Snapshot, State, StateName, Ticket};
use crate::vram::{GameDetector, ModelOp, Models, NoGameDetector, Profile, VramPolicy, VramStatus};

pub const SOCKET_NAME: &str = "fastwisper.sock";
pub const LOCK_NAME: &str = "fastwisper.lock";
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5);

const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 32;
const ACTOR_QUEUE: usize = 64;
const STATUS_REFRESH: Duration = Duration::from_millis(500);
/// Intervalo do stream `meter` (~25 Hz).
pub const METER_INTERVAL: Duration = Duration::from_millis(40);
/// Cargas/descargas em andamento no encerramento, e a descarga final.
const MODEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// Aviso ao entrar no modo jogo pelo detector (sem conteúdo).
pub const GAME_MODE_NOTICE: &str = "Modo jogo: o FastWisper liberou a VRAM";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPaths {
    pub runtime_dir: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
}

impl DaemonPaths {
    pub fn in_dir(runtime_dir: impl Into<PathBuf>) -> Self {
        let runtime_dir = runtime_dir.into();
        Self {
            socket: runtime_dir.join(SOCKET_NAME),
            lock: runtime_dir.join(LOCK_NAME),
            runtime_dir,
        }
    }

    /// Sem `XDG_RUNTIME_DIR` não há fallback para `/tmp`.
    pub fn from_env() -> Result<Self, DaemonError> {
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|dir| !dir.is_empty())
            .map(DaemonPaths::in_dir)
            .ok_or(DaemonError::NoRuntimeDir)
    }
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(
        "XDG_RUNTIME_DIR não está definido; o socket só é criado no diretório de runtime do usuário"
    )]
    NoRuntimeDir,
    #[error("diretório de runtime inseguro {path}: {reason}")]
    InsecureRuntimeDir { path: PathBuf, reason: &'static str },
    #[error("o fastwisperd já está rodando (lock em {0})")]
    AlreadyRunning(PathBuf),
    #[error("{0} existe e não é um socket deste usuário; não vou removê-lo")]
    UnsafeSocketPath(PathBuf),
    #[error("erro de E/S em {path}")]
    Io { path: PathBuf, source: io::Error },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> DaemonError + '_ {
    move |source| DaemonError::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub struct Daemon {
    listener: UnixListener,
    paths: DaemonPaths,
    read_timeout: Duration,
    history: Option<History>,
    notes_path: Option<PathBuf>,
    network: Option<NetworkSandbox>,
    vram: VramPolicy,
    detector: Arc<dyn GameDetector>,
    config: Option<ConfigState>,
    _lock: File,
}

impl Daemon {
    /// Garante instância única e cria o socket `0600`. Precisa rodar dentro
    /// de um runtime tokio.
    pub fn bind(paths: DaemonPaths) -> Result<Self, DaemonError> {
        let euid = rustix::process::geteuid().as_raw();
        check_runtime_dir(&paths.runtime_dir, euid)?;

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(OFlags::NOFOLLOW.bits() as i32)
            .open(&paths.lock)
            .map_err(io_err(&paths.lock))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(DaemonError::AlreadyRunning(paths.lock.clone()));
            }
            Err(fs::TryLockError::Error(source)) => return Err(io_err(&paths.lock)(source)),
        }

        // Com o lock na mão, um socket que sobrou não tem dono vivo.
        remove_stale_socket(&paths.socket, euid)?;
        let listener =
            std::os::unix::net::UnixListener::bind(&paths.socket).map_err(io_err(&paths.socket))?;
        fs::set_permissions(&paths.socket, Permissions::from_mode(0o600))
            .map_err(io_err(&paths.socket))?;
        listener
            .set_nonblocking(true)
            .map_err(io_err(&paths.socket))?;
        let listener = UnixListener::from_std(listener).map_err(io_err(&paths.socket))?;

        Ok(Self {
            listener,
            paths,
            read_timeout: DEFAULT_READ_TIMEOUT,
            history: None,
            notes_path: crate::notes::default_path(),
            network: None,
            vram: VramPolicy::default(),
            detector: Arc::new(NoGameDetector),
            config: None,
            _lock: lock,
        })
    }

    /// Caminho alternativo do banco de notas (principalmente para testes isolados).
    pub fn with_notes_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.notes_path = Some(path.into());
        self
    }

    /// Arquivo que `reload_config` relê, e a config lida dele no início.
    /// `required` (`--config` explícito): o arquivo precisa existir; sem
    /// isso, arquivo ausente vale como os padrões. Sem config, a recarga é
    /// recusada.
    pub fn with_config(mut self, path: impl Into<PathBuf>, required: bool, config: Config) -> Self {
        // Absoluto: o `status` informa este caminho e a CLI, noutra pasta,
        // compara com o arquivo que gravou.
        let path = path.into();
        let path = std::path::absolute(&path).unwrap_or(path);
        self.config = Some(ConfigState {
            path,
            required,
            running: config.clone(),
            boot: config,
            pending: None,
        });
        self
    }

    /// Perfil de VRAM inicial (`[performance]`).
    pub fn with_vram(mut self, vram: VramPolicy) -> Self {
        self.vram = vram;
        self
    }

    /// Detector de jogo do perfil `automatico` (padrão: nunca há jogo).
    pub fn with_game_detector(mut self, detector: Arc<dyn GameDetector>) -> Self {
        self.detector = detector;
        self
    }

    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    /// Grava métricas de cada ditado (sem texto) neste histórico.
    pub fn with_history(mut self, history: History) -> Self {
        self.history = Some(history);
        self
    }

    /// Estado do bloqueio de rede aplicado ao processo, mostrado em `status`.
    pub fn with_network_sandbox(mut self, network: NetworkSandbox) -> Self {
        self.network = Some(network);
        self
    }

    pub fn paths(&self) -> &DaemonPaths {
        &self.paths
    }

    /// Atende até `shutdown` resolver. Depois para de aceitar conexões,
    /// cancela a sessão ativa, espera os workers e remove o socket.
    pub async fn run<P: Pipeline, D: Desktop>(
        self,
        pipeline: Arc<P>,
        desktop: Arc<D>,
        shutdown: impl Future<Output = ()>,
    ) {
        let Daemon {
            listener,
            paths,
            read_timeout,
            history,
            notes_path,
            network,
            vram,
            detector,
            config,
            _lock,
        } = self;
        let (tx, rx) = mpsc::channel(ACTOR_QUEUE);
        let (status_tx, status_rx) = watch::channel(StatusEvent::default());
        let (game, game_watch) = watch_game(detector, status_rx.clone());
        let meter = Arc::clone(&pipeline);
        let actor = tokio::spawn(run_actor(
            rx,
            pipeline,
            desktop,
            status_tx,
            Extras {
                history,
                notes_path,
                network,
                vram,
                game,
                config,
            },
        ));
        let euid = rustix::process::geteuid().as_raw();
        let mut connections = JoinSet::new();
        tokio::pin!(shutdown);

        info!(socket = %paths.socket.display(), "fastwisperd pronto");
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) if !same_user(&stream, euid) => {
                        warn!("conexão de outro usuário recusada");
                    }
                    Ok((stream, _)) if connections.len() < MAX_CONNECTIONS => {
                        connections.spawn(serve_connection(
                            stream,
                            tx.clone(),
                            status_rx.clone(),
                            Arc::clone(&meter),
                            read_timeout,
                        ));
                    }
                    Ok(_) => warn!("conexões demais abertas; recusando"),
                    Err(e) => {
                        warn!(error = %e, "falha ao aceitar conexão");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
            }
        }

        info!("encerrando");
        drop(listener);
        if let Err(e) = fs::remove_file(&paths.socket)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(error = %e, "não foi possível remover o socket");
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}

        let (done_tx, done_rx) = oneshot::channel();
        if tx.send(ActorMsg::Shutdown(done_tx)).await.is_ok() {
            let _ = done_rx.await;
        }
        drop(tx);
        if let Err(e) = actor.await {
            error!(error = %e, "actor da sessão terminou com erro");
        }
        if let Some(handle) = game_watch {
            handle.abort();
        }
    }
}

/// Consulta o detector de jogo fora do actor, no intervalo dele.
fn watch_game(
    detector: Arc<dyn GameDetector>,
    status: watch::Receiver<StatusEvent>,
) -> (Option<watch::Receiver<bool>>, Option<task::JoinHandle<()>>) {
    let Some(interval) = detector.poll_interval() else {
        return (None, None);
    };
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        loop {
            let automatic = status
                .borrow()
                .vram
                .as_ref()
                .is_some_and(|v| v.mode == crate::vram::VramMode::Automatico);
            if !automatic {
                tx.send_if_modified(|current| std::mem::replace(current, false));
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(interval).await;
                continue;
            }
            let probe = Arc::clone(&detector);
            let running = task::spawn_blocking(move || probe.is_game_running())
                .await
                .unwrap_or(false);
            tx.send_if_modified(|current| std::mem::replace(current, running) != running);
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(interval).await;
        }
    });
    (Some(rx), Some(handle))
}

fn check_runtime_dir(dir: &Path, euid: u32) -> Result<(), DaemonError> {
    let insecure = |reason| DaemonError::InsecureRuntimeDir {
        path: dir.to_path_buf(),
        reason,
    };
    let meta = fs::metadata(dir).map_err(io_err(dir))?;
    if !meta.is_dir() {
        return Err(insecure("não é um diretório"));
    }
    if meta.uid() != euid {
        return Err(insecure("pertence a outro usuário"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(insecure("acessível por grupo/outros (esperado 0700)"));
    }
    Ok(())
}

fn remove_stale_socket(path: &Path, euid: u32) -> Result<(), DaemonError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_err(path)(source)),
        Ok(meta) if meta.file_type().is_socket() && meta.uid() == euid => {
            debug!("removendo socket antigo");
            match fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_err(path)(e)),
                _ => Ok(()),
            }
        }
        Ok(_) => Err(DaemonError::UnsafeSocketPath(path.to_path_buf())),
    }
}

/// Resposta do actor a um comando, antes de virar JSON.
struct Reply {
    snapshot: Snapshot,
    error: Option<ErrorCode>,
    asr: Option<Readiness>,
    last: Option<LastSummary>,
    text: Option<String>,
    delivery: Option<Delivery>,
    network: Option<NetworkSandbox>,
    vram: Option<VramStatus>,
    /// Mensagem específica do erro (config inválida), no lugar da padrão.
    message: Option<String>,
    reload: Option<ReloadReport>,
    config_path: Option<PathBuf>,
    notes: Option<Vec<crate::notes::Note>>,
    deleted: Option<usize>,
    note_session: bool,
}

impl Reply {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            error: None,
            asr: None,
            last: None,
            text: None,
            delivery: None,
            network: None,
            vram: None,
            message: None,
            reload: None,
            config_path: None,
            notes: None,
            deleted: None,
            note_session: false,
        }
    }

    fn rejected(code: ErrorCode, snapshot: Snapshot) -> Self {
        Self {
            error: Some(code),
            ..Reply::new(snapshot)
        }
    }

    fn into_response(self, request_id: String) -> Response {
        let mut response = match self.error {
            Some(code) => Response::rejected(request_id, code, self.snapshot),
            None => Response::ok(request_id, self.snapshot),
        };
        response.asr = self.asr;
        response.last = self.last;
        response.text = self.text;
        response.delivery = self.delivery;
        response.network = self.network;
        response.vram = self.vram;
        response.reload = self.reload;
        response.config_path = self.config_path;
        response.notes = self.notes;
        response.deleted = self.deleted;
        response.note_session = self.note_session;
        if let (Some(error), Some(message)) = (&mut response.error, self.message) {
            error.message = message;
        }
        response
    }
}

/// Config do daemon para `reload_config`.
struct ConfigState {
    path: PathBuf,
    required: bool,
    /// Config do início: as chaves que exigem reinício ficam com ela.
    boot: Config,
    /// O que está em vigor.
    running: Config,
    /// Recarga validada esperando o ditado (ou a troca anterior) terminar.
    pending: Option<Config>,
}

enum ActorMsg {
    Command {
        cmd: Command,
        args: ipc::Args,
        reply: oneshot::Sender<Reply>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// Carga ou descarga terminada.
enum OpDone {
    Load,
    /// Aquecimento do ASR no início do daemon (perfis sob demanda).
    Warmup,
    Unload,
}

/// Estado do actor: só ele mexe aqui, então nada de locks.
struct Ctx {
    session: Session,
    workers: Workers,
    last: LastResult,
    /// Prazo da gravação atual: passou, o microfone fecha sem esperar o release.
    deadline: Option<(Ticket, Instant)>,
    /// Janela com foco no `press` desta sessão (destino da colagem).
    target: Option<(Ticket, Option<Target>)>,
    /// Instante do release, para o `e2e_ms`.
    released: Option<(Ticket, Instant)>,
    history: Option<History>,
    notes_path: Option<PathBuf>,
    network: Option<NetworkSandbox>,
    /// Perfil de VRAM (M7). Cargas e descargas rodam em `ops`, fora do actor.
    vram: VramPolicy,
    ops: JoinSet<OpDone>,
    /// Descargas em execução (contam como pendentes no status).
    unloading: usize,
    /// Ordem dos pedidos de carga/descarga (`ModelOp::seq`).
    op_seq: u64,
    /// Descarga agendada por ociosidade (`unload_after_secs`).
    unload_at: Option<Instant>,
    /// Descarga pedida; sai quando não houver ditado nem worker em andamento.
    unload_pending: bool,
    /// Perfil com que a sessão começou (vai nas métricas).
    session_profile: Option<(Ticket, Profile)>,
    config: Option<ConfigState>,
    /// Troca de config em andamento no pipeline (LLM); a próxima espera.
    reconfiguring: JoinSet<()>,
    note_ticket: Option<Ticket>,
}

impl Ctx {
    fn next_op(&mut self, ticket: Option<Ticket>) -> ModelOp {
        self.op_seq += 1;
        ModelOp {
            seq: self.op_seq,
            ticket,
        }
    }

    fn idle(&self) -> bool {
        self.session.state() == State::Idle && self.workers.is_empty()
    }

    /// Sessão cancelada, recusada ou abortada. Para a VRAM é o fim de um
    /// ditado: vale a ociosidade do perfil, mesmo com a carga do press ainda
    /// em andamento (o modelo fica quando chegar). O cancel nunca descarrega
    /// além do fim normal: `jogo` descarrega logo depois, `desempenho` mantém.
    fn abandon(&mut self, ticket: Ticket) {
        self.session_profile = None;
        if self.note_ticket == Some(ticket) {
            self.note_ticket = None;
        }
        self.schedule_idle();
    }

    /// Fim de um ditado (ou de uma tentativa): agenda a descarga do perfil.
    fn schedule_idle(&mut self) {
        match self.vram.settings().unload_after {
            None => self.unload_at = None,
            Some(after) if after.is_zero() => self.unload_pending = true,
            Some(after) => self.unload_at = Some(Instant::now() + after),
        }
    }

    fn vram_status<P: Pipeline>(&self, pipeline: &P, countdown: bool) -> VramStatus {
        let residency = pipeline.residency();
        let now = Instant::now();
        VramStatus {
            mode: self.vram.mode(),
            profile: self.vram.profile(),
            game: self.vram.game(),
            asr: residency.asr,
            llm: residency.llm,
            vram_mb: residency.vram_bytes.map(|b| b.div_ceil(1 << 20)),
            unload_in_secs: self
                .unload_at
                .filter(|_| countdown)
                .map(|at| at.saturating_duration_since(now).as_secs()),
            unload_pending: self.unload_pending || self.unloading > 0,
        }
    }
}

/// Pede a carga do que o perfil usa. Nunca espera: roda em `ops`.
fn request_load<P: Pipeline>(ctx: &mut Ctx, pipeline: &Arc<P>, ticket: Option<Ticket>) {
    let llm = ctx.vram.settings().llm;
    let op = ctx.next_op(ticket);
    let load = pipeline.load(op, Models { asr: true, llm });
    ctx.ops.spawn(async move {
        load.await;
        OpDone::Load
    });
}

/// Executa a descarga pendente se não houver ditado em andamento. A
/// descarga de um perfil novo, de um `unload` ou do timer espera aqui o
/// ditado atual terminar.
fn try_unload<P: Pipeline>(ctx: &mut Ctx, pipeline: &Arc<P>) {
    if !ctx.unload_pending || !ctx.idle() {
        return;
    }
    ctx.unload_pending = false;
    ctx.unload_at = None;
    let op = ctx.next_op(None);
    debug!(seq = op.seq, "descarregando modelos");
    let unload = pipeline.unload(op, Models::ALL);
    ctx.unloading += 1;
    ctx.ops.spawn(async move {
        unload.await;
        OpDone::Unload
    });
}

/// O perfil em vigor mudou (comando ou detector).
fn apply_profile<P: Pipeline, D: Desktop>(
    ctx: &mut Ctx,
    pipeline: &Arc<P>,
    desktop: &Arc<D>,
    (from, to): (Profile, Profile),
    automatic: bool,
) {
    pipeline.set_profile(ctx.vram.settings());
    info!(from = %from, to = %to, automatic, "perfil de VRAM");
    // Pendência e timer do perfil anterior não valem mais: o perfil novo
    // decide (ex.: jogo → sob_demanda no meio do ditado volta a esperar a
    // ociosidade, sem descarga logo após o ditado).
    ctx.unload_at = None;
    ctx.unload_pending = false;
    match to {
        Profile::Desempenho => request_load(ctx, pipeline, None),
        _ if ctx.session.state() == State::Idle => ctx.schedule_idle(),
        // Ditado em andamento: ele termina com o perfil com que começou; a
        // descarga do jogo fica pendente até lá.
        Profile::Jogo => ctx.unload_pending = true,
        Profile::SobDemanda => {}
    }
    if automatic && to == Profile::Jogo {
        desktop.notify(GAME_MODE_NOTICE);
    }
    try_unload(ctx, pipeline);
}

/// Relê a config (fora do actor) e guarda a validada como pendente. Config
/// inválida é recusada e a pendente anterior, se houver, continua.
async fn read_config(ctx: &mut Ctx) -> Result<ReloadReport, (ErrorCode, String)> {
    let Some(state) = ctx.config.as_ref() else {
        return Err((
            ErrorCode::Unavailable,
            "este daemon não tem arquivo de config para reler".into(),
        ));
    };
    let (path, required) = (state.path.clone(), state.required);
    let new = match task::spawn_blocking(move || Config::load_path(&path, required)).await {
        Ok(Ok(config)) => config,
        Ok(Err(e)) => {
            warn!("recarga da config recusada: {e}");
            return Err((ErrorCode::InvalidConfig, e.to_string()));
        }
        Err(e) => return Err((ErrorCode::InvalidConfig, e.to_string())),
    };
    let Some(state) = ctx.config.as_mut() else {
        unreachable!("a config do daemon não some");
    };
    let restart_required = Config::restart_required(&state.boot, &new);
    let changed = state.running.changed_keys(&new);
    state.pending = Some(new.with_boot_values(&state.boot, &restart_required));
    Ok(ReloadReport {
        changed,
        restart_required,
        pending: true,
    })
}

/// Aplica a config pendente se não houver ditado, worker nem troca anterior
/// em andamento (como a descarga de VRAM, espera o ditado terminar).
fn try_reload<P: Pipeline, D: Desktop>(ctx: &mut Ctx, pipeline: &Arc<P>, desktop: &Arc<D>) {
    let ready = ctx.idle() && ctx.reconfiguring.is_empty();
    let Some(state) = ctx.config.as_mut() else {
        return;
    };
    if !ready {
        return;
    }
    let Some(config) = state.pending.take() else {
        return;
    };
    let changed = state.running.changed_keys(&config);
    state.running = config.clone();
    desktop.reconfigure(&config.inject);
    desktop.reconfigure_language(config.ui.language);
    let op = ctx.next_op(None);
    ctx.reconfiguring.spawn(pipeline.reconfigure(op, &config));
    let before = ctx.vram.settings();
    match ctx.vram.reconfigure(&config.performance) {
        Some(change) => apply_profile(ctx, pipeline, desktop, change, false),
        None if ctx.vram.settings() != before => {
            let profile = ctx.vram.profile();
            apply_profile(ctx, pipeline, desktop, (profile, profile), false);
        }
        // A gravação máxima entra no `keep_alive` do LLM.
        None => pipeline.set_profile(ctx.vram.settings()),
    }
    info!(changed = ?changed, "config recarregada");
}

/// O que o actor recebe além dos canais.
struct Extras {
    history: Option<History>,
    notes_path: Option<PathBuf>,
    network: Option<NetworkSandbox>,
    vram: VramPolicy,
    game: Option<watch::Receiver<bool>>,
    config: Option<ConfigState>,
}

/// Próxima mudança do detector de jogo; sem detector, nunca.
async fn game_changed(game: &mut Option<watch::Receiver<bool>>) -> bool {
    let Some(rx) = game else {
        return std::future::pending().await;
    };
    if rx.changed().await.is_err() {
        *game = None;
        return std::future::pending().await;
    }
    *rx.borrow_and_update()
}

/// Único dono da [`Session`] e do último resultado. Comandos, completions e
/// entregas passam por aqui em ordem.
async fn run_actor<P: Pipeline, D: Desktop>(
    mut rx: mpsc::Receiver<ActorMsg>,
    pipeline: Arc<P>,
    desktop: Arc<D>,
    status: watch::Sender<StatusEvent>,
    extras: Extras,
) {
    let Extras {
        history,
        notes_path,
        network,
        vram,
        mut game,
        config,
    } = extras;
    desktop.reconfigure_language(
        config
            .as_ref()
            .map(|state| state.running.ui.language)
            .unwrap_or_default(),
    );
    // Prontidão do ASR muda fora do actor: revisão periódica do status.
    let mut refresh = tokio::time::interval(STATUS_REFRESH);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ctx = Ctx {
        session: Session::new(),
        workers: Workers::default(),
        last: LastResult::default(),
        deadline: None,
        target: None,
        released: None,
        history,
        notes_path,
        network,
        vram,
        ops: JoinSet::new(),
        unloading: 0,
        op_seq: 0,
        unload_at: None,
        unload_pending: false,
        session_profile: None,
        config,
        reconfiguring: JoinSet::new(),
        note_ticket: None,
    };
    // Desempenho carrega tudo já. Nos outros perfis o ASR aquece uma vez
    // agora (a 1ª carga depois de instalar/atualizar compila os pipelines
    // Vulkan, ~7 s, e o 1º ditado não deve pagar isso) e só então vale a
    // descarga do perfil; daí em diante, carga no press.
    let settings = ctx.vram.settings();
    pipeline.set_profile(settings);
    if settings.unload_after.is_none() {
        request_load(&mut ctx, &pipeline, None);
    } else {
        let op = ctx.next_op(None);
        let warmup = pipeline.load(
            op,
            Models {
                asr: true,
                llm: false,
            },
        );
        ctx.ops.spawn(async move {
            warmup.await;
            OpDone::Warmup
        });
    }

    let done = loop {
        try_reload(&mut ctx, &pipeline, &desktop);
        try_unload(&mut ctx, &pipeline);
        publish(&status, &ctx, &*pipeline);
        let expiry = ctx.last.expires_at();
        let deadline = ctx.deadline.map(|(_, at)| at);
        let unload_at = ctx.unload_at;
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(ActorMsg::Command { cmd, args, reply }) => {
                    let r = handle_command(cmd, args, &mut ctx, &pipeline, &desktop).await;
                    let _ = reply.send(r);
                }
                Some(ActorMsg::Shutdown(done)) => break Some(done),
                None => break None,
            },
            Some((ticket, outcome)) = ctx.workers.join_next(), if !ctx.workers.is_empty() => {
                complete(ticket, outcome, &mut ctx, &desktop).await;
            }
            Some(joined) = ctx.ops.join_next(), if !ctx.ops.is_empty() => match joined {
                Ok(OpDone::Load) => {}
                Ok(OpDone::Warmup) => {
                    if ctx.session.state() == State::Idle {
                        ctx.schedule_idle();
                    }
                }
                Ok(OpDone::Unload) => ctx.unloading = ctx.unloading.saturating_sub(1),
                Err(e) => error!(error = %e, "carga/descarga de modelo falhou"),
            },
            Some(joined) = ctx.reconfiguring.join_next(), if !ctx.reconfiguring.is_empty() => {
                if let Err(e) = joined {
                    error!(error = %e, "troca de config no pipeline falhou");
                }
                // Desempenho: um LLM trocado carrega já, como no início.
                if ctx.vram.settings().unload_after.is_none() {
                    request_load(&mut ctx, &pipeline, None);
                }
            }
            running = game_changed(&mut game) => {
                if let Some(change) = ctx.vram.set_game(running) {
                    apply_profile(&mut ctx, &pipeline, &desktop, change, true);
                }
            }
            () = sleep_until(unload_at), if unload_at.is_some() => {
                ctx.unload_at = None;
                ctx.unload_pending = true;
            }
            _ = refresh.tick() => {}
            () = sleep_until(deadline), if deadline.is_some() => {
                if let Some((ticket, _)) = ctx.deadline.take()
                    && ctx.session.abort_recording(ticket)
                {
                    pipeline.abort(ticket);
                    ctx.target = None;
                    warn!(session = %ticket.session_id, "gravação passou do limite; microfone fechado");
                    desktop.notify(Failure::RecordingTooLong.user_message());
                    // Nota que estoura o prazo também não substitui o último ditado.
                    if ctx.note_ticket != Some(ticket) {
                        ctx.last.record(Outcome::Failed(Failure::RecordingTooLong, Metrics::default()), Instant::now());
                    }
                    ctx.abandon(ticket);
                }
            }
            () = sleep_until(expiry), if expiry.is_some() => {
                ctx.last.expire(Instant::now());
                debug!("último texto expirou");
            }
        }
    };

    if let Some(Effect::Abort(ticket)) = ctx.session.shutdown() {
        pipeline.abort(ticket);
    }
    ctx.last.clear_text();
    ctx.workers.drain(WORKER_SHUTDOWN_TIMEOUT).await;
    // Cargas (e trocas de LLM) em andamento terminam antes da descarga
    // final: um modelo que chegasse depois dela ficaria na VRAM sem dono.
    if tokio::time::timeout(MODEL_SHUTDOWN_TIMEOUT, async {
        while ctx.reconfiguring.join_next().await.is_some() {}
        while ctx.ops.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        warn!("cargas de modelo não terminaram a tempo; abortando");
        ctx.reconfiguring.abort_all();
        ctx.ops.abort_all();
    }
    let op = ctx.next_op(None);
    if tokio::time::timeout(MODEL_SHUTDOWN_TIMEOUT, pipeline.unload(op, Models::ALL))
        .await
        .is_err()
    {
        warn!("descarga dos modelos não terminou a tempo");
    }
    if let Some(done) = done {
        let _ = done.send(());
    }
}

/// Resultado de um worker. Só vale (e só é entregue) se o ticket ainda for
/// o da sessão corrente; o texto fica em RAM **antes** da colagem.
async fn complete<D: Desktop>(ticket: Ticket, outcome: Outcome, ctx: &mut Ctx, desktop: &Arc<D>) {
    if !ctx.session.complete(ticket) {
        debug!(session = %ticket.session_id, generation = ticket.generation, "resultado de sessão antiga descartado");
        return;
    }
    if ctx.note_ticket == Some(ticket) {
        ctx.note_ticket = None;
        ctx.target = None;
        ctx.released = None;
        ctx.deadline = None;
        ctx.session_profile = None;
        ctx.schedule_idle();
        // Não passa por LastResult, métricas nem entrega: show-last/paste-last
        // nunca recuperam uma nota, e contexto da janela nem foi consultado.
        match outcome {
            Outcome::Transcribed { text, language, .. } => {
                let enabled = ctx
                    .config
                    .as_ref()
                    .is_some_and(|c| c.running.experimental.notes);
                let saved = ctx
                    .notes_path
                    .as_ref()
                    .ok_or(crate::notes::NotesError::Unsafe)
                    .and_then(|path| {
                        crate::notes::Notes::new(path).save(enabled, &text, language.code())
                    });
                if saved.is_err() {
                    desktop.notify("Não foi possível salvar a nota nesta máquina.");
                }
            }
            Outcome::Failed(failure, _) => desktop.notify(failure.user_message()),
            _ => {}
        }
        return;
    }
    let target = match ctx.target.take() {
        Some((t, target)) if t == ticket => target,
        _ => None,
    };
    let app = target.as_ref().map(|t| t.class.clone());
    let released = match ctx.released.take() {
        Some((t, at)) if t == ticket => Some(at),
        _ => None,
    };
    let summary = outcome.summary();
    let submit = matches!(outcome, Outcome::Transcribed { submit: true, .. });
    if let Outcome::Failed(failure, _) = &outcome {
        desktop.notify(failure.user_message());
    }
    ctx.last.record(outcome, Instant::now());
    if let Some((t, profile)) = ctx.session_profile.take()
        && t == ticket
    {
        ctx.last.set_profile(profile);
    }
    ctx.schedule_idle();

    let delivery = match ctx.last.text(Instant::now()) {
        Some(text) => {
            let delivery = desktop.deliver(text.to_owned(), target, submit).await;
            ctx.last.set_delivery(delivery);
            Some(delivery)
        }
        None => None,
    };
    if let Some(at) = released {
        ctx.last.set_e2e(at.elapsed());
    }
    if let (Some(history), Some(summary)) = (&ctx.history, ctx.last.summary())
        && let Err(e) = history.record(&summary, app.as_deref())
    {
        warn!("{e}");
    }
    info!(
        session = %ticket.session_id,
        outcome = %summary.outcome.as_str(),
        audio_ms = summary.metrics.audio_ms,
        asr_ms = summary.metrics.asr_ms,
        load_ms = summary.metrics.load_ms,
        words = summary.metrics.words,
        delivery = ?delivery,
        "ditado concluído"
    );
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

async fn handle_command<P: Pipeline, D: Desktop>(
    cmd: Command,
    args: ipc::Args,
    ctx: &mut Ctx,
    pipeline: &Arc<P>,
    desktop: &Arc<D>,
) -> Reply {
    if matches!(
        cmd,
        Command::NoteStart
            | Command::NoteSave
            | Command::NotesList
            | Command::NotesCopy
            | Command::NotesDelete
            | Command::NotesClear
    ) && !ctx
        .config
        .as_ref()
        .is_some_and(|c| c.running.experimental.notes)
    {
        return Reply::rejected(ErrorCode::NotesDisabled, ctx.session.snapshot());
    }
    // Um `cancel` (Esc) com nota ativa cancela a nota, não o último ditado.
    let cancels_note = cmd == Command::Cancel && ctx.note_ticket.is_some();
    let session_cmd = if cmd == Command::NoteCancel && ctx.note_ticket.is_some() {
        Command::Cancel
    } else {
        cmd
    };
    let transition = match ctx.session.handle(session_cmd) {
        Ok(transition) => transition,
        Err(Rejection::Busy) => return Reply::rejected(ErrorCode::Busy, ctx.session.snapshot()),
    };

    match transition.effect {
        Some(Effect::StartCapture(ticket)) => {
            ctx.unload_at = None;
            ctx.unload_pending = false;
            ctx.session_profile = Some((ticket, ctx.vram.profile()));
            // Destino = janela com foco no momento do press. Uma nota não tem
            // destino e não mexe no último ditado: `paste-last`/`copy-last`
            // continuam com ele.
            let note = cmd == Command::NoteStart;
            let target = if note {
                ctx.note_ticket = Some(ticket);
                None
            } else {
                desktop.focused().await
            };
            if let Err(failure) = pipeline.start(ticket).await {
                ctx.session.abort_recording(ticket);
                ctx.abandon(ticket);
                warn!(code = failure.error_code().as_str(), "gravação recusada");
                desktop.notify(failure.user_message());
                if !note {
                    ctx.last
                        .record(Outcome::Failed(failure, Metrics::default()), Instant::now());
                }
                return Reply::rejected(failure.error_code(), ctx.session.snapshot());
            }
            // Microfone aberto: os modelos carregam enquanto a pessoa fala
            // (sem esperar aqui). Press recusado não carrega nada.
            request_load(ctx, pipeline, Some(ticket));
            ctx.target = Some((ticket, target));
            ctx.deadline = Some((ticket, Instant::now() + pipeline.max_recording()));
        }
        Some(Effect::FinishCapture(ticket)) => {
            let literal = matches!(&ctx.target, Some((t, Some(target))) if *t == ticket && target.is_terminal_or_harness());
            ctx.released = Some((ticket, Instant::now()));
            ctx.workers.spawn(ticket, pipeline.finish(ticket, literal));
        }
        Some(Effect::Abort(ticket)) => {
            pipeline.abort(ticket);
            ctx.target = None;
            ctx.abandon(ticket);
        }
        None => {}
    }
    // `cancel` apaga o texto recuperável, com ou sem ditado ativo; com uma
    // nota ativa, só a nota é cancelada.
    if cmd == Command::Cancel && !cancels_note {
        ctx.last.clear_text();
    }

    let snapshot = ctx.session.snapshot();
    debug!(cmd = cmd.as_str(), state = %snapshot.state, generation = snapshot.generation, "comando");
    let mut reply = Reply::new(snapshot);
    reply.note_session = ctx.note_ticket.is_some();
    match cmd {
        Command::NotesList | Command::NotesCopy | Command::NotesDelete | Command::NotesClear => {
            let Some(path) = &ctx.notes_path else {
                return Reply::rejected(ErrorCode::NotesStorage, snapshot);
            };
            let store = crate::notes::Notes::new(path);
            match cmd {
                Command::NotesList => match store.list_recent(50) {
                    Ok(notes) => {
                        // Resposta sempre cabe na linha IPC (inclui escaping JSON).
                        let mut bytes = 0;
                        reply.notes = Some(
                            notes
                                .into_iter()
                                .take(50)
                                .take_while(|note| {
                                    bytes += serde_json::to_vec(note)
                                        .map_or(usize::MAX / 2, |j| j.len())
                                        + 1;
                                    bytes < 48 * 1024
                                })
                                .collect(),
                        );
                    }
                    Err(_) => reply.error = Some(ErrorCode::NotesStorage),
                },
                Command::NotesCopy => match store.get(args.id.unwrap_or(0)) {
                    Ok(Some(note)) => {
                        reply.delivery = Some(desktop.copy(note.text).await);
                        if reply.delivery == Some(Delivery::Failed) {
                            reply.error = Some(ErrorCode::DeliveryFailed);
                        }
                    }
                    Ok(None) => reply.error = Some(ErrorCode::NoteNotFound),
                    Err(_) => reply.error = Some(ErrorCode::NotesStorage),
                },
                _ => match if cmd == Command::NotesClear {
                    store.clear()
                } else {
                    store.delete(args.id.unwrap_or(0))
                } {
                    Ok(n) => reply.deleted = Some(n),
                    Err(_) => reply.error = Some(ErrorCode::NotesStorage),
                },
            }
        }
        Command::Status => {
            reply.asr = Some(pipeline.readiness());
            reply.last = ctx.last.summary();
            reply.network = ctx.network.clone();
            reply.vram = Some(ctx.vram_status(&**pipeline, true));
            reply.config_path = ctx.config.as_ref().map(|c| c.path.clone());
        }
        Command::Unload => {
            ctx.unload_at = None;
            ctx.unload_pending = true;
            try_unload(ctx, pipeline);
            reply.vram = Some(ctx.vram_status(&**pipeline, true));
        }
        Command::SetVram => {
            if let Some(mode) = args.mode {
                // A config em vigor acompanha (a CLI grava a mesma escolha).
                if let Some(state) = &mut ctx.config {
                    state.running.performance.vram = mode;
                    if let Some(pending) = &mut state.pending {
                        pending.performance.vram = mode;
                    }
                }
                if let Some(change) = ctx.vram.set_mode(mode) {
                    apply_profile(ctx, pipeline, desktop, change, false);
                }
            }
            reply.vram = Some(ctx.vram_status(&**pipeline, true));
        }
        Command::ReloadConfig => {
            match read_config(ctx).await {
                Ok(mut report) => {
                    if ctx
                        .config
                        .as_ref()
                        .and_then(|c| c.pending.as_ref())
                        .is_some_and(|c| !c.experimental.notes)
                        && ctx.note_ticket.is_some()
                        && let Ok(transition) = ctx.session.handle(Command::Cancel)
                        && let Some(Effect::Abort(ticket)) = transition.effect
                    {
                        pipeline.abort(ticket);
                        ctx.target = None;
                        ctx.abandon(ticket);
                    }
                    try_reload(ctx, pipeline, desktop);
                    report.pending = ctx.config.as_ref().is_some_and(|c| c.pending.is_some());
                    reply.reload = Some(report);
                }
                Err((code, message)) => {
                    reply.error = Some(code);
                    reply.message = Some(message);
                }
            }
            reply.vram = Some(ctx.vram_status(&**pipeline, true));
        }
        Command::ShowLast | Command::CopyLast | Command::PasteLast => {
            reply.last = ctx.last.summary();
            let Some(text) = ctx.last.text(Instant::now()).map(str::to_owned) else {
                reply.error = Some(ErrorCode::NoResult);
                return reply;
            };
            match cmd {
                Command::ShowLast => reply.text = Some(text),
                Command::CopyLast => reply.delivery = Some(desktop.copy(text).await),
                _ => reply.delivery = Some(desktop.paste_focused(text).await),
            }
            if reply.delivery == Some(Delivery::Failed) {
                reply.error = Some(ErrorCode::DeliveryFailed);
            }
        }
        _ => {}
    }
    reply
}

/// Workers de processamento, com o ticket de cada um para contabilidade
/// mesmo se o future entrar em pânico.
#[derive(Default)]
struct Workers {
    set: JoinSet<(Ticket, Outcome)>,
    tickets: HashMap<task::Id, Ticket>,
}

impl Workers {
    fn spawn(&mut self, ticket: Ticket, work: impl Future<Output = Outcome> + Send + 'static) {
        let handle = self.set.spawn(async move { (ticket, work.await) });
        self.tickets.insert(handle.id(), ticket);
    }

    fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    async fn join_next(&mut self) -> Option<(Ticket, Outcome)> {
        match self.set.join_next_with_id().await? {
            Ok((id, result)) => {
                self.tickets.remove(&id);
                Some(result)
            }
            Err(e) => {
                error!(error = %e, "worker de processamento falhou");
                let ticket = self.tickets.remove(&e.id())?;
                Some((
                    ticket,
                    Outcome::Failed(Failure::AsrFailed, Metrics::default()),
                ))
            }
        }
    }

    async fn drain(&mut self, timeout: Duration) {
        let drained = tokio::time::timeout(timeout, async {
            while !self.is_empty() {
                self.join_next().await;
            }
        })
        .await;
        if drained.is_err() {
            warn!(
                pending = self.set.len(),
                "workers não terminaram a tempo; abortando"
            );
            self.set.abort_all();
            while self.set.join_next().await.is_some() {}
        }
    }
}

/// Credenciais do par: só o próprio usuário fala com o daemon.
fn same_user(stream: &UnixStream, euid: u32) -> bool {
    stream.peer_cred().is_ok_and(|cred| cred.uid() == euid)
}

/// Status publicado para `subscribe`: estado, prontidão e resumo do último
/// ditado. Nunca texto.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusEvent {
    seq: u64,
    snapshot: Option<Snapshot>,
    asr: Option<Readiness>,
    last: Option<LastSummary>,
    failures: u64,
    vram: Option<VramStatus>,
    note_session: bool,
}

impl StatusEvent {
    fn to_response(&self) -> Response {
        let mut response = match self.snapshot {
            Some(snapshot) => Response::ok(String::from("subscribe"), snapshot),
            None => Response::error(None, ErrorCode::Unavailable),
        };
        response.request_id = None;
        response.asr = self.asr;
        response.last = self.last;
        response.failures = Some(self.failures);
        response.vram = self.vram;
        response.seq = Some(self.seq);
        response.note_session = self.note_session;
        response
    }
}

/// Publica só quando algo mudou; o `watch` guarda apenas o último valor,
/// então cliente lento nunca acumula fila (pula para o estado atual).
fn publish<P: Pipeline>(status: &watch::Sender<StatusEvent>, ctx: &Ctx, pipeline: &P) {
    let snapshot = Some(ctx.session.snapshot());
    let asr = Some(pipeline.readiness());
    let last = ctx.last.summary();
    let failures = ctx.last.failures();
    // Sem contagem regressiva: ela mudaria a cada tique.
    let vram = Some(ctx.vram_status(pipeline, false));
    status.send_if_modified(|event| {
        if event.snapshot == snapshot
            && event.asr == asr
            && event.last == last
            && event.failures == failures
            && event.vram == vram
            && event.note_session == ctx.note_ticket.is_some()
        {
            return false;
        }
        event.seq += 1;
        event.snapshot = snapshot;
        event.asr = asr;
        event.last = last;
        event.failures = failures;
        event.vram = vram;
        event.note_session = ctx.note_ticket.is_some();
        true
    });
}

/// Conexão de `subscribe`: manda o status atual e depois cada mudança, até
/// o cliente fechar.
async fn stream_status(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
    mut status: watch::Receiver<StatusEvent>,
) {
    let mut sink = [0u8; 256];
    loop {
        let event = status.borrow_and_update().clone();
        if event.snapshot.is_some()
            && write_response(&mut writer, &event.to_response())
                .await
                .is_err()
        {
            return;
        }
        tokio::select! {
            changed = status.changed() => if changed.is_err() { return },
            read = reader.read(&mut sink) => if matches!(read, Ok(0) | Err(_)) { return },
        }
    }
}

/// Gravação em andamento segundo o último status publicado.
fn recording(status: &watch::Receiver<StatusEvent>) -> Option<Snapshot> {
    status
        .borrow()
        .snapshot
        .filter(|s| s.state == StateName::Recording)
}

/// Conexão de `meter`: enquanto grava, o nível do microfone a cada
/// [`METER_INTERVAL`]; no fim da gravação, um evento com o estado novo e
/// nível 0; fora da gravação, nada. Lê só um atômico do pipeline: a captura
/// e o actor nunca esperam este cliente, e tique perdido por cliente lento
/// é pulado, não enfileirado.
async fn stream_meter<P: Pipeline>(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
    mut status: watch::Receiver<StatusEvent>,
    pipeline: Arc<P>,
) {
    let mut sink = [0u8; 256];
    loop {
        status.mark_unchanged();
        let Some(mut current) = recording(&status) else {
            tokio::select! {
                changed = status.changed() => if changed.is_err() { return },
                read = reader.read(&mut sink) => if matches!(read, Ok(0) | Err(_)) { return },
            }
            continue;
        };
        let mut tick = tokio::time::interval(METER_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let event = ipc::MeterEvent::new(current, pipeline.level());
                    if write_line(&mut writer, &event.to_line()).await.is_err() {
                        return;
                    }
                }
                changed = status.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let now = status.borrow_and_update().snapshot;
                    match now {
                        // Mesma gravação (outro campo do status mudou).
                        Some(s) if s.state == StateName::Recording
                            && s.session_id == current.session_id => current = s,
                        _ => {
                            let end = now.unwrap_or(Snapshot {
                                state: StateName::Idle,
                                mode: None,
                                session_id: None,
                                generation: current.generation,
                            });
                            let event = ipc::MeterEvent::new(end, 0.0);
                            if write_line(&mut writer, &event.to_line()).await.is_err() {
                                return;
                            }
                            break;
                        }
                    }
                }
                read = reader.read(&mut sink) => if matches!(read, Ok(0) | Err(_)) { return },
            }
        }
    }
}

async fn serve_connection<P: Pipeline>(
    stream: UnixStream,
    tx: mpsc::Sender<ActorMsg>,
    status: watch::Receiver<StatusEvent>,
    pipeline: Arc<P>,
    read_timeout: Duration,
) {
    let (read_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = Vec::with_capacity(256);
    let limit = MAX_LINE_BYTES as u64 + 1;

    loop {
        line.clear();
        let read = tokio::time::timeout(
            read_timeout,
            (&mut reader).take(limit).read_until(b'\n', &mut line),
        )
        .await;
        match read {
            Err(_) => {
                // Conexão ociosa fecha calada; linha pela metade recebe erro.
                if !line.is_empty() {
                    let _ = write_response(&mut writer, &Response::error(None, ErrorCode::Timeout))
                        .await;
                }
                return;
            }
            Ok(Err(_) | Ok(0)) => return,
            Ok(Ok(_)) => {}
        }

        let terminated = line.last() == Some(&b'\n');
        if terminated {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        if line.len() > MAX_LINE_BYTES {
            // Sem `\n` dentro do limite não há como ressincronizar o stream.
            let _ =
                write_response(&mut writer, &Response::error(None, ErrorCode::LineTooLong)).await;
            return;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            if terminated {
                continue;
            }
            return;
        }

        let response = match ipc::parse_request(&line) {
            Ok(request) if request.cmd == Command::Subscribe => {
                return stream_status(reader, writer, status).await;
            }
            Ok(request) if request.cmd == Command::Meter => {
                return stream_meter(reader, writer, status, pipeline).await;
            }
            Ok(request) => dispatch(&tx, request).await,
            Err(e) => e.into_response(),
        };
        if write_response(&mut writer, &response).await.is_err() || !terminated {
            return;
        }
    }
}

async fn dispatch(tx: &mpsc::Sender<ActorMsg>, request: ipc::Request) -> Response {
    let ipc::Request {
        request_id,
        cmd,
        args,
        ..
    } = request;
    let (reply, rx) = oneshot::channel();
    if tx
        .send(ActorMsg::Command { cmd, args, reply })
        .await
        .is_err()
    {
        return Response::error(Some(request_id), ErrorCode::Unavailable);
    }
    match rx.await {
        Ok(reply) => reply.into_response(request_id),
        Err(_) => Response::error(Some(request_id), ErrorCode::Unavailable),
    }
}

async fn write_response(writer: &mut OwnedWriteHalf, response: &Response) -> io::Result<()> {
    write_line(writer, &response.to_line()).await
}

async fn write_line(writer: &mut OwnedWriteHalf, line: &[u8]) -> io::Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, writer.write_all(line))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}
