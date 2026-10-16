//! Contrato dos perfis de VRAM: trocar de perfil
//! nunca perde um ditado, cancelar segue a ociosidade do perfil, a descarga
//! espera a transcrição/formatação, o LLM só descarrega o modelo que o
//! FastWisper carregou e, sem carga a tempo, o ditado sai com as regras.
//!
//! Nível do actor: pipeline falso com cargas controladas pelo teste. Nível
//! do LLM: Ollama falso em loopback.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{FakeDesktop, MARKER_TEXT, TestDaemon, window};
use fw_core::asr::Language;
use fw_core::config::Config;
use fw_core::daemon::GAME_MODE_NOTICE;
use fw_core::ipc::{ErrorCode, Request, Response};
use fw_core::llm::LlmOutcome;
use fw_core::pipeline::{Failure, Metrics, Outcome, OutcomeKind, Pipeline};
use fw_core::polish::{LlmUse, Polisher};
use fw_core::session::{Command, Session, StateName, Ticket};
use fw_core::vram::{
    GameLlm, ManualGameDetector, ModelOp, ModelState, Models, Profile, ProfileSettings, Residency,
    VramMode, VramPolicy,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

// ---------------------------------------------------------------------------
// Nível do actor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Load {
        seq: u64,
        generation: Option<u64>,
        llm: bool,
    },
    /// A carga terminou; `kept = false`: uma descarga mais nova a invalidou.
    Loaded {
        seq: u64,
        kept: bool,
    },
    Unload {
        seq: u64,
    },
    FinishStarted,
    FinishDone,
    Profile(Profile),
}

#[derive(Default)]
struct FakeModels {
    last_seq: u64,
    loading: bool,
    loaded: bool,
}

/// Pipeline cujas cargas e processamento só terminam quando o teste libera.
/// Imita o slot real: pedido mais velho que o último aplicado não vale.
struct ModelPipeline {
    load_gate: Arc<Semaphore>,
    finish_gate: Arc<Semaphore>,
    models: Arc<Mutex<FakeModels>>,
    log: Arc<Mutex<Vec<Event>>>,
    finishing: AtomicUsize,
    refuse_start: AtomicBool,
}

impl ModelPipeline {
    /// `gated_loads`: as cargas esperam `release_load`.
    fn new(gated_loads: bool) -> Arc<ModelPipeline> {
        let log = Arc::new(Mutex::new(Vec::new()));
        Arc::new(ModelPipeline {
            load_gate: Arc::new(Semaphore::new(if gated_loads { 0 } else { 1 << 20 })),
            finish_gate: Arc::new(Semaphore::new(0)),
            models: Arc::new(Mutex::new(FakeModels::default())),
            log,
            finishing: AtomicUsize::new(0),
            refuse_start: AtomicBool::new(false),
        })
    }

    fn push(&self, event: Event) {
        self.log.lock().expect("log").push(event);
    }

    fn events(&self) -> Vec<Event> {
        self.log.lock().expect("log").clone()
    }

    fn unloads(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| matches!(e, Event::Unload { .. }))
            .count()
    }

    fn position(&self, wanted: impl Fn(&Event) -> bool) -> Option<usize> {
        self.events().iter().position(wanted)
    }

    fn release_load(&self) {
        self.load_gate.add_permits(1);
    }

    fn clear(&self) {
        self.log.lock().expect("log").clear();
    }

    fn release_finish(&self) {
        self.finish_gate.add_permits(1);
    }
}

impl Pipeline for ModelPipeline {
    fn start(&self, _ticket: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        let refused = self.refuse_start.load(Ordering::SeqCst);
        std::future::ready(if refused {
            Err(Failure::DeviceUnavailable)
        } else {
            Ok(())
        })
    }

    fn finish(
        &self,
        _ticket: Ticket,
        _literal: bool,
    ) -> impl Future<Output = Outcome> + Send + 'static {
        self.push(Event::FinishStarted);
        self.finishing.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&self.finish_gate);
        let log = Arc::clone(&self.log);
        async move {
            if let Ok(permit) = gate.acquire().await {
                permit.forget();
            }
            log.lock().expect("log").push(Event::FinishDone);
            Outcome::Transcribed {
                text: MARKER_TEXT.into(),
                language: Language::Pt,
                submit: false,
                metrics: Metrics::default(),
            }
        }
    }

    /// O worker real continua até o fim depois de um cancel; aqui também.
    fn abort(&self, _ticket: Ticket) {}

    fn set_profile(&self, settings: ProfileSettings) {
        self.push(Event::Profile(settings.profile));
    }

    fn load(&self, op: ModelOp, models: Models) -> impl Future<Output = ()> + Send + 'static {
        self.push(Event::Load {
            seq: op.seq,
            generation: op.ticket.map(|t| t.generation),
            llm: models.llm,
        });
        {
            let mut m = self.models.lock().expect("models");
            if op.seq >= m.last_seq {
                m.last_seq = op.seq;
                if !m.loaded {
                    m.loading = true;
                }
            }
        }
        let gate = Arc::clone(&self.load_gate);
        let state = Arc::clone(&self.models);
        let log = Arc::clone(&self.log);
        async move {
            if let Ok(permit) = gate.acquire().await {
                permit.forget();
            }
            let mut m = state.lock().expect("models");
            let kept = m.last_seq == op.seq;
            if kept {
                m.loaded = true;
                m.loading = false;
            } else if m.last_seq > op.seq && !m.loaded {
                m.loading = false;
            }
            log.lock()
                .expect("log")
                .push(Event::Loaded { seq: op.seq, kept });
        }
    }

    fn unload(&self, op: ModelOp, _models: Models) -> impl Future<Output = ()> + Send + 'static {
        self.push(Event::Unload { seq: op.seq });
        let mut m = self.models.lock().expect("models");
        if op.seq >= m.last_seq {
            m.last_seq = op.seq;
            m.loaded = false;
        }
        std::future::ready(())
    }

    fn residency(&self) -> Residency {
        let m = self.models.lock().expect("models");
        let state = if m.loaded {
            ModelState::Loaded
        } else if m.loading {
            ModelState::Loading
        } else {
            ModelState::Unloaded
        };
        Residency {
            asr: state,
            llm: state,
            vram_bytes: Some(if m.loaded { 4 << 30 } else { 0 }),
        }
    }
}

fn policy(mode: VramMode, unload_after: Duration) -> VramPolicy {
    VramPolicy::new(mode, unload_after, GameLlm::Off)
}

async fn start(pipeline: &Arc<ModelPipeline>, vram: VramPolicy) -> (TestDaemon, Arc<FakeDesktop>) {
    let desktop = Arc::new(FakeDesktop::default());
    desktop.focus(Some(window("0x1")));
    let daemon = TestDaemon::start_configured(Arc::clone(pipeline), Arc::clone(&desktop), |d| {
        d.with_vram(vram)
    })
    .await;
    (daemon, desktop)
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "esperando: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_state(daemon: &TestDaemon, state: StateName) -> Response {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = daemon.ctl(Command::Status).await;
        if status.state == Some(state) {
            return status;
        }
        assert!(Instant::now() < deadline, "esperando o estado {state}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn set_vram(daemon: &TestDaemon, mode: VramMode) -> Response {
    common::send(
        &daemon.paths.socket,
        Request::set_vram("test-set-vram", mode),
    )
    .await
}

/// Perfis sob demanda aquecem o ASR no início. Libera esse aquecimento,
/// descarrega e zera o registro: o teste começa a frio (`seq` 1 e 2 usados).
async fn after_warmup(daemon: &TestDaemon, pipeline: &ModelPipeline) {
    pipeline.release_load();
    wait_until("aquecimento do início", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;
    settle().await;
    daemon.ctl(Command::Unload).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon
        .ctl(Command::Status)
        .await
        .vram
        .is_some_and(|v| v.unload_pending || v.asr != ModelState::Unloaded)
    {
        assert!(Instant::now() < deadline, "descarga do aquecimento");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    pipeline.clear();
}

/// Espera um pouco para provar que algo **não** acontece.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

#[tokio::test]
async fn profile_switch_during_recording_never_loses_the_dictation() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_secs(300)),
    )
    .await;

    let press = daemon.ctl(Command::Press).await;
    assert!(press.ok);
    let generation = press.generation;

    // Troca para o jogo no meio da gravação: a descarga fica pendente.
    let switched = set_vram(&daemon, VramMode::Jogo).await;
    assert!(switched.ok, "{switched:?}");
    let vram = switched.vram.expect("vram na resposta");
    assert_eq!((vram.mode, vram.profile), (VramMode::Jogo, Profile::Jogo));
    assert!(vram.unload_pending, "descarga adiada até o fim do ditado");
    assert_eq!(switched.state, Some(StateName::Recording), "gravação segue");
    assert_eq!(
        switched.generation, generation,
        "mesma sessão, mesma generation"
    );
    settle().await;
    assert_eq!(pipeline.unloads(), 0, "nada descarrega durante a gravação");

    daemon.ctl(Command::Release).await;
    wait_until("worker começar", || {
        pipeline.finishing.load(Ordering::SeqCst) == 1
    })
    .await;
    // Troca de novo durante a transcrição: ainda nada sai.
    set_vram(&daemon, VramMode::Desempenho).await;
    set_vram(&daemon, VramMode::Jogo).await;
    settle().await;
    assert_eq!(
        pipeline.unloads(),
        0,
        "nada descarrega durante a transcrição"
    );

    pipeline.release_finish();
    let done = wait_state(&daemon, StateName::Idle).await;
    let last = done.last.expect("resumo");
    assert_eq!(last.outcome, OutcomeKind::TRANSCRIBED);
    assert_eq!(
        last.metrics.profile,
        Some(Profile::SobDemanda),
        "o ditado termina com o perfil com que começou"
    );
    assert_eq!(
        desktop.delivered.lock().expect("lock")[0].0,
        MARKER_TEXT,
        "texto entregue"
    );

    wait_until("descarga depois do ditado", || pipeline.unloads() >= 1).await;
    let finished = pipeline
        .position(|e| *e == Event::FinishDone)
        .expect("fim do worker");
    let unloaded = pipeline
        .position(|e| matches!(e, Event::Unload { .. }))
        .expect("descarga");
    assert!(unloaded > finished, "descarga só depois da transcrição");
    daemon.stop().await;
}

#[tokio::test]
async fn cancel_during_a_cold_load_keeps_it_for_the_profile_idle() {
    let pipeline = ModelPipeline::new(true);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_secs(300)),
    )
    .await;
    after_warmup(&daemon, &pipeline).await;

    let press = daemon.ctl(Command::Press).await;
    let generation = press.generation.expect("generation");
    assert_eq!(
        pipeline.events(),
        [Event::Load {
            seq: 3,
            generation: Some(generation),
            llm: true
        }],
        "carga no press, ASR e LLM juntos"
    );
    let status = daemon.ctl(Command::Status).await;
    assert_eq!(status.vram.map(|v| v.asr), Some(ModelState::Loading));

    let cancel = daemon.ctl(Command::Cancel).await;
    assert_eq!(cancel.state, Some(StateName::Idle));
    assert!(
        cancel.generation > Some(generation),
        "generation invalidada"
    );
    settle().await;
    assert_eq!(
        pipeline.unloads(),
        0,
        "o cancel não descarrega a carga a frio"
    );
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert!(!vram.unload_pending, "{vram:?}");
    assert!(vram.unload_in_secs.is_some_and(|s| s > 290), "{vram:?}");

    pipeline.release_load();
    wait_until("carga do ditado cancelado mantida", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 3, kept: true })
    })
    .await;
    let status = daemon.ctl(Command::Status).await;
    assert_eq!(status.vram.map(|v| v.asr), Some(ModelState::Loaded));
    assert_eq!(status.last, None, "sessão cancelada não deixa resultado");

    // A próxima sessão tem identidade nova e acha o modelo já carregado.
    let again = daemon.ctl(Command::Press).await;
    assert!(again.generation > cancel.generation);
    assert!(pipeline.events().contains(&Event::Load {
        seq: 4,
        generation: again.generation,
        llm: true
    }));
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!(vram.asr, ModelState::Loaded, "sem recarga");
    daemon.ctl(Command::Cancel).await;
    pipeline.release_load();
    settle().await;
    assert_eq!(pipeline.unloads(), 0);
    daemon.stop().await;
}

/// `press` → `cancel` imediato com o ASR já residente e a carga do press
/// (ASR + LLM) ainda sem resposta: o cancel vale como fim de ditado e o
/// modelo fica pela ociosidade do perfil. Nunca `release`.
async fn cancel_right_after_press_keeps_the_resident_asr(mode: VramMode) {
    let idle = Duration::from_millis(1200);
    let pipeline = ModelPipeline::new(true);
    let (daemon, _desktop) = start(&pipeline, policy(mode, idle)).await;
    pipeline.release_load();
    wait_until("aquecimento do início", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;
    pipeline.clear();

    for round in 0..2 {
        let press = daemon.ctl(Command::Press).await;
        assert!(press.ok, "{press:?}");
        let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
        assert_eq!(
            (vram.profile, vram.asr),
            (Profile::SobDemanda, ModelState::Loaded),
            "press {round} acha o ASR carregado, sem recarga"
        );
        let cancel = daemon.ctl(Command::Cancel).await;
        assert_eq!(cancel.state, Some(StateName::Idle));
        settle().await;
        assert_eq!(
            pipeline.unloads(),
            0,
            "cancel {round} não descarrega: {:?}",
            pipeline.events()
        );
        let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
        assert_eq!(vram.asr, ModelState::Loaded, "{mode}: {vram:?}");
        assert!(!vram.unload_pending, "{mode}: {vram:?}");
        assert!(vram.unload_in_secs.is_some(), "ociosidade agendada");
    }
    assert!(
        !pipeline
            .events()
            .iter()
            .any(|e| matches!(e, Event::Loaded { .. })),
        "as cargas dos presses ainda estão sem resposta"
    );

    // Passada a ociosidade do perfil, descarrega.
    wait_until("descarga por ociosidade", || pipeline.unloads() == 1).await;
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!(vram.asr, ModelState::Unloaded);
    pipeline.release_load();
    pipeline.release_load();
    daemon.stop().await;
}

#[tokio::test]
async fn cancel_right_after_press_keeps_the_asr_on_demand() {
    cancel_right_after_press_keeps_the_resident_asr(VramMode::SobDemanda).await;
}

#[tokio::test]
async fn cancel_right_after_press_keeps_the_asr_in_automatic() {
    // Sem jogo, o automático vale o perfil base (sob demanda).
    cancel_right_after_press_keeps_the_resident_asr(VramMode::Automatico).await;
}

#[tokio::test]
async fn cancel_in_game_unloads_like_the_end_of_a_dictation() {
    // Jogo: descarga logo depois do ditado, por definição. O cancel segue a
    // mesma regra do fim normal: uma descarga, depois do cancel, nada além.
    let pipeline = ModelPipeline::new(true);
    let (daemon, _desktop) =
        start(&pipeline, policy(VramMode::Jogo, Duration::from_secs(300))).await;
    pipeline.release_load();
    wait_until("jogo descarrega o aquecimento", || pipeline.unloads() == 1).await;
    settle().await;
    pipeline.clear();

    daemon.ctl(Command::Press).await;
    settle().await;
    assert_eq!(pipeline.unloads(), 0, "gravando: nada sai");
    daemon.ctl(Command::Cancel).await;
    wait_until("descarga do jogo", || pipeline.unloads() == 1).await;
    pipeline.release_load();
    wait_until("carga do press terminar", || {
        pipeline
            .events()
            .iter()
            .any(|e| matches!(e, Event::Loaded { .. }))
    })
    .await;
    settle().await;
    assert_eq!(pipeline.unloads(), 1, "só a descarga do fim do ditado");
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!(vram.asr, ModelState::Unloaded, "{vram:?}");
    assert!(!vram.unload_pending, "{vram:?}");
    daemon.stop().await;
}

#[tokio::test]
async fn cancel_during_a_reload_keeps_performance_resident() {
    let pipeline = ModelPipeline::new(true);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::Desempenho, Duration::from_secs(300)),
    )
    .await;
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Cancel).await;
    settle().await;
    assert_eq!(pipeline.unloads(), 0, "desempenho não descarrega no cancel");
    pipeline.release_load();
    pipeline.release_load();
    daemon.stop().await;
}

#[tokio::test]
async fn refused_press_loads_nothing() {
    let pipeline = ModelPipeline::new(false);
    pipeline.refuse_start.store(true, Ordering::SeqCst);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_secs(300)),
    )
    .await;
    after_warmup(&daemon, &pipeline).await;
    let press = daemon.ctl(Command::Press).await;
    assert_eq!(press.error_code(), Some(ErrorCode::DeviceUnavailable));
    settle().await;
    assert!(
        !pipeline
            .events()
            .iter()
            .any(|e| matches!(e, Event::Load { .. })),
        "sem microfone, nada vai para a VRAM: {:?}",
        pipeline.events()
    );
    daemon.stop().await;
}

#[tokio::test]
async fn unload_waits_for_the_transcription_in_progress() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::Desempenho, Duration::from_secs(300)),
    )
    .await;
    wait_until("carga inicial do desempenho", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    wait_until("worker começar", || {
        pipeline.finishing.load(Ordering::SeqCst) == 1
    })
    .await;

    let unload = daemon.ctl(Command::Unload).await;
    assert!(unload.ok);
    assert_eq!(unload.state, Some(StateName::Transcribing));
    assert!(unload.vram.expect("vram").unload_pending);
    settle().await;
    assert_eq!(
        pipeline.unloads(),
        0,
        "descarga não acontece no meio da transcrição"
    );

    // Cancelar não libera a descarga enquanto o worker ainda roda.
    daemon.ctl(Command::Cancel).await;
    settle().await;
    assert_eq!(
        pipeline.unloads(),
        0,
        "worker cancelado ainda ocupa o modelo"
    );

    pipeline.release_finish();
    wait_until("descarga depois do worker", || pipeline.unloads() == 1).await;
    let finished = pipeline.position(|e| *e == Event::FinishDone).expect("fim");
    let unloaded = pipeline
        .position(|e| matches!(e, Event::Unload { .. }))
        .expect("descarga");
    assert!(unloaded > finished);

    // Ocioso: `unload` sai na hora (o status mostra a descarga em execução
    // até ela terminar).
    daemon.ctl(Command::Unload).await;
    assert_eq!(pipeline.unloads(), 2);
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon
        .ctl(Command::Status)
        .await
        .vram
        .is_some_and(|v| v.unload_pending)
    {
        assert!(Instant::now() < deadline, "descarga não terminou");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    daemon.stop().await;
}

#[tokio::test]
async fn idle_timer_unloads_on_demand_but_never_in_performance() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_millis(400)),
    )
    .await;
    // Aquecimento só do ASR no início; depois vale a ociosidade.
    wait_until("aquecimento do início", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;
    assert!(pipeline.events().contains(&Event::Load {
        seq: 1,
        generation: None,
        llm: false
    }));
    assert_eq!(pipeline.unloads(), 0, "aquecido, dentro de unload_after");
    wait_until("descarga do aquecimento", || pipeline.unloads() == 1).await;

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_finish();
    wait_state(&daemon, StateName::Idle).await;
    assert_eq!(pipeline.unloads(), 1, "ainda dentro de unload_after");
    wait_until("descarga por ociosidade", || pipeline.unloads() == 2).await;
    daemon.stop().await;

    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::Desempenho, Duration::from_millis(100)),
    )
    .await;
    wait_until("desempenho carrega no início", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_finish();
    wait_state(&daemon, StateName::Idle).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pipeline.unloads(), 0, "desempenho mantém tudo carregado");
    daemon.stop().await;
}

#[tokio::test]
async fn zero_unload_after_releases_right_after_the_dictation() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(&pipeline, policy(VramMode::SobDemanda, Duration::ZERO)).await;
    // `0`: o aquecimento do início sai logo que termina.
    wait_until("descarga do aquecimento", || pipeline.unloads() == 1).await;
    pipeline.clear();
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    settle().await;
    assert_eq!(pipeline.unloads(), 0);
    pipeline.release_finish();
    wait_until("descarga imediata", || pipeline.unloads() == 1).await;
    daemon.stop().await;
}

#[tokio::test]
async fn startup_warmup_hides_the_first_compile_then_follows_the_profile() {
    // O aquecimento do início fica carregado só pela ociosidade do perfil.
    let pipeline = ModelPipeline::new(true);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_secs(300)),
    )
    .await;
    wait_until("aquecimento pedido", || {
        pipeline.events().contains(&Event::Load {
            seq: 1,
            generation: None,
            llm: false,
        })
    })
    .await;
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!(vram.asr, ModelState::Loading);
    assert_eq!(vram.unload_in_secs, None, "timer só depois do aquecimento");

    // Press durante o aquecimento usa a mesma carga.
    let press = daemon.ctl(Command::Press).await;
    pipeline.release_load();
    pipeline.release_load();
    wait_until("carga do press", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 2, kept: true })
    })
    .await;
    settle().await;
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!(vram.unload_in_secs, None, "gravando: sem descarga agendada");
    assert!(press.ok);
    daemon.ctl(Command::Cancel).await;
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert!(vram.unload_in_secs.is_some_and(|s| s > 290), "{vram:?}");
    assert_eq!(pipeline.unloads(), 0);
    daemon.stop().await;

    // Jogo: aquece e descarrega logo em seguida.
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) =
        start(&pipeline, policy(VramMode::Jogo, Duration::from_secs(300))).await;
    wait_until("jogo descarrega o aquecimento", || pipeline.unloads() == 1).await;
    let loaded = pipeline
        .position(|e| matches!(e, Event::Loaded { seq: 1, .. }))
        .expect("aquecido");
    let unloaded = pipeline
        .position(|e| matches!(e, Event::Unload { .. }))
        .expect("descarga");
    assert!(unloaded > loaded, "descarga só depois do aquecimento");
    daemon.stop().await;
}

#[tokio::test]
async fn pending_unload_does_not_survive_a_profile_change() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(
        &pipeline,
        policy(VramMode::SobDemanda, Duration::from_secs(300)),
    )
    .await;
    after_warmup(&daemon, &pipeline).await;

    daemon.ctl(Command::Press).await;
    let to_game = set_vram(&daemon, VramMode::Jogo).await;
    assert!(to_game.vram.expect("vram").unload_pending);
    let back = set_vram(&daemon, VramMode::SobDemanda).await;
    assert!(
        !back.vram.expect("vram").unload_pending,
        "a pendência do jogo sai com a troca"
    );
    daemon.ctl(Command::Release).await;
    pipeline.release_finish();
    wait_state(&daemon, StateName::Idle).await;
    settle().await;
    assert_eq!(pipeline.unloads(), 0, "sob demanda espera a ociosidade");
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert!(vram.unload_in_secs.is_some_and(|s| s > 290), "{vram:?}");

    // Mesmo com a sessão cancelada e o worker ainda vivo.
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    daemon.ctl(Command::Cancel).await;
    set_vram(&daemon, VramMode::Jogo).await;
    let back = set_vram(&daemon, VramMode::SobDemanda).await;
    assert!(!back.vram.expect("vram").unload_pending);
    pipeline.release_finish();
    settle().await;
    assert_eq!(pipeline.unloads(), 0);
    daemon.stop().await;
}

#[tokio::test]
async fn automatic_enters_game_mode_and_returns_to_the_base() {
    let pipeline = ModelPipeline::new(false);
    let detector = Arc::new(ManualGameDetector::new(Duration::from_millis(20)));
    let desktop = Arc::new(FakeDesktop::default());
    let vram = VramPolicy::default().with_base(Profile::Desempenho);
    let probe = Arc::clone(&detector);
    let daemon = TestDaemon::start_configured(Arc::clone(&pipeline), Arc::clone(&desktop), |d| {
        d.with_vram(vram).with_game_detector(probe)
    })
    .await;
    wait_until("desempenho carrega no início", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;

    detector.set(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
        if vram.profile == Profile::Jogo {
            assert!(vram.game);
            assert_eq!(vram.mode, VramMode::Automatico);
            break;
        }
        assert!(Instant::now() < deadline, "não entrou no modo jogo");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_until("descarga ao entrar no jogo", || pipeline.unloads() == 1).await;
    assert_eq!(
        desktop.notices.lock().expect("lock").as_slice(),
        [GAME_MODE_NOTICE]
    );

    // No jogo, o press carrega só o ASR (game_llm = off).
    let press = daemon.ctl(Command::Press).await;
    assert!(pipeline.events().contains(&Event::Load {
        seq: 3,
        generation: press.generation,
        llm: false
    }));
    daemon.ctl(Command::Cancel).await;

    detector.set(false);
    wait_until("volta ao perfil base e recarrega", || {
        pipeline
            .events()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Load {
                        generation: None,
                        llm: true,
                        ..
                    }
                )
            })
            .count()
            == 2
    })
    .await;
    let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
    assert_eq!((vram.profile, vram.game), (Profile::Desempenho, false));
    assert_eq!(
        desktop.notices.lock().expect("lock").len(),
        1,
        "só a entrada no jogo avisa"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn status_and_events_carry_the_profile_but_never_text() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(&pipeline, policy(VramMode::Desempenho, Duration::ZERO)).await;
    wait_until("carga", || {
        pipeline
            .events()
            .contains(&Event::Loaded { seq: 1, kept: true })
    })
    .await;
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_finish();
    let status = wait_state(&daemon, StateName::Idle).await;
    let json = String::from_utf8(status.to_line()).expect("utf8");
    assert!(!json.contains(MARKER_TEXT), "{json}");
    let vram = status.vram.expect("vram");
    assert_eq!(vram.mode, VramMode::Desempenho);
    assert_eq!(
        (vram.asr, vram.llm),
        (ModelState::Loaded, ModelState::Loaded)
    );
    assert_eq!(vram.vram_mb, Some(4096));

    let mut stream = tokio::net::UnixStream::connect(&daemon.paths.socket)
        .await
        .expect("connect");
    stream
        .write_all(&Request::new("sub", Command::Subscribe).to_line())
        .await
        .expect("write");
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
        .await
        .expect("evento")
        .expect("read");
    let event: serde_json::Value =
        serde_json::from_slice(buf[..n].split(|b| *b == b'\n').next().expect("linha"))
            .expect("json");
    assert_eq!(event["vram"]["profile"], "desempenho");
    assert!(
        event["vram"].get("unload_in_secs").is_none(),
        "eventos sem contagem regressiva"
    );
    assert!(!String::from_utf8_lossy(&buf[..n]).contains(MARKER_TEXT));
    daemon.stop().await;
}

#[tokio::test]
async fn set_vram_and_unload_arguments_are_typed() {
    let pipeline = ModelPipeline::new(false);
    let (daemon, _desktop) = start(&pipeline, VramPolicy::default()).await;
    for (request, expected) in [
        (
            r#"{"v":1,"request_id":"a","cmd":"set_vram","args":{"mode":"turbo"}}"#,
            "invalid_args",
        ),
        (
            r#"{"v":1,"request_id":"b","cmd":"set_vram"}"#,
            "invalid_args",
        ),
        (
            r#"{"v":1,"request_id":"c","cmd":"set_vram","args":{"mode":"jogo","x":1}}"#,
            "invalid_args",
        ),
        (
            r#"{"v":1,"request_id":"d","cmd":"unload","args":{"mode":"jogo"}}"#,
            "invalid_args",
        ),
        (
            r#"{"v":1,"request_id":"e","cmd":"status","args":{"mode":"jogo"}}"#,
            "invalid_args",
        ),
    ] {
        let responses = common::exchange(&daemon.paths.socket, &[&common::line(request)]).await;
        assert_eq!(
            responses[0]["error"]["code"].as_str(),
            Some(expected),
            "{request}"
        );
    }
    let ok = common::exchange(
        &daemon.paths.socket,
        &[&common::line(
            r#"{"v":1,"request_id":"f","cmd":"set_vram","args":{"mode":"sob_demanda"}}"#,
        )],
    )
    .await;
    assert_eq!(ok[0]["vram"]["mode"], "sob_demanda");
    let unload = daemon.ctl(Command::Unload).await;
    assert_eq!(unload.error_code(), None::<ErrorCode>);
    daemon.stop().await;
}

#[test]
fn vram_commands_do_not_touch_the_session() {
    let mut session = Session::new();
    session.handle(Command::Press).expect("press");
    let before = session.snapshot();
    for cmd in [Command::Unload, Command::SetVram] {
        let t = session.handle(cmd).expect("aceito");
        assert_eq!(t.effect, None);
        assert_eq!(t.snapshot, before, "mesma sessão e generation");
    }
}

// ---------------------------------------------------------------------------
// Nível do LLM: Ollama falso
// ---------------------------------------------------------------------------

const OUR_MODEL: &str = fw_core::models::OLLAMA_MODEL;
const OTHER_MODEL: &str = "outro-modelo:7b";
const INPUT: &str = "manda o relatório na quinta";

/// Ollama falso: inventário fixo, `/api/ps` configurável, `/api/generate`
/// com atraso configurável e `/api/chat` que formata.
struct FakeOllama {
    url: String,
    /// (instante, primeira linha, corpo) de cada pedido.
    requests: Arc<Mutex<Vec<(Instant, String, String)>>>,
    resident: Arc<Mutex<Vec<String>>>,
    load_delay: Arc<Mutex<Duration>>,
    chat_delay: Arc<Mutex<Duration>>,
    ps: Arc<Mutex<PsMode>>,
}

/// Como o `/api/ps` falso responde.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum PsMode {
    #[default]
    Ok,
    Status500,
    NotJson,
    NoList,
    /// Além do prazo de 2 s do cliente.
    Slow,
}

impl FakeOllama {
    /// Um pedido por conexão: as respostas dizem `connection: close` para o
    /// cliente não reaproveitar do pool uma conexão já fechada (A-24).
    async fn start() -> FakeOllama {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let fake = FakeOllama {
            url,
            requests: Arc::default(),
            resident: Arc::default(),
            load_delay: Arc::default(),
            chat_delay: Arc::default(),
            ps: Arc::default(),
        };
        let (requests, resident, load_delay, chat_delay, ps) = (
            Arc::clone(&fake.requests),
            Arc::clone(&fake.resident),
            Arc::clone(&fake.load_delay),
            Arc::clone(&fake.chat_delay),
            Arc::clone(&fake.ps),
        );
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (requests, resident, load_delay, chat_delay, ps) = (
                    Arc::clone(&requests),
                    Arc::clone(&resident),
                    Arc::clone(&load_delay),
                    Arc::clone(&chat_delay),
                    Arc::clone(&ps),
                );
                tokio::spawn(async move {
                    let raw = read_request(&mut socket).await;
                    let head = raw.lines().next().unwrap_or_default().to_owned();
                    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
                    requests.lock().expect("lock").push((
                        Instant::now(),
                        head.clone(),
                        body.clone(),
                    ));
                    let json: serde_json::Value =
                        serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                    let reply = if head.starts_with("GET /api/tags ") {
                        serde_json::json!({"models": [
                            {"name": OUR_MODEL, "digest": fw_core::models::OLLAMA_DIGEST},
                            {"name": OTHER_MODEL, "digest": "abc"},
                        ]})
                        .to_string()
                    } else if head.starts_with("GET /api/ps ") {
                        let mode = *ps.lock().expect("lock");
                        match mode {
                            PsMode::Ok => {}
                            PsMode::Status500 => {
                                let _ = socket
                                    .write_all(b"HTTP/1.1 500 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                                    .await;
                                return;
                            }
                            PsMode::NotJson => {
                                let _ = socket
                                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\noops!")
                                    .await;
                                return;
                            }
                            PsMode::NoList => {
                                let _ = socket
                                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                                    .await;
                                return;
                            }
                            PsMode::Slow => tokio::time::sleep(Duration::from_secs(3)).await,
                        }
                        let models: Vec<_> = resident
                            .lock()
                            .expect("lock")
                            .iter()
                            .map(|m| serde_json::json!({"name": m, "model": m, "size_vram": 3_u64 << 30}))
                            .collect();
                        serde_json::json!({ "models": models }).to_string()
                    } else if head.starts_with("POST /api/generate ") {
                        let model = json["model"].as_str().unwrap_or_default().to_owned();
                        if json["keep_alive"] == 0 {
                            resident.lock().expect("lock").retain(|m| *m != model);
                        } else {
                            let delay = *load_delay.lock().expect("lock");
                            tokio::time::sleep(delay).await;
                            let mut list = resident.lock().expect("lock");
                            if !list.contains(&model) {
                                list.push(model);
                            }
                        }
                        r#"{"done":true}"#.to_owned()
                    } else {
                        let delay = *chat_delay.lock().expect("lock");
                        tokio::time::sleep(delay).await;
                        let line = serde_json::json!({
                            "message": {"content": "<out>Manda o relatório na quinta.</out>"},
                            "done": true,
                        });
                        format!("{line}\n")
                    };
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                                reply.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        fake
    }

    fn heads(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("lock")
            .iter()
            .map(|(_, head, _)| head.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// Corpos de `POST /api/generate`.
    fn generates(&self) -> Vec<(Instant, serde_json::Value)> {
        self.requests
            .lock()
            .expect("lock")
            .iter()
            .filter(|(_, head, _)| head.starts_with("POST /api/generate "))
            .map(|(at, _, body)| (*at, serde_json::from_str(body).expect("json")))
            .collect()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = socket.read(&mut buf).await {
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some(head_end) = text.find("\r\n\r\n") {
            let length = text[..head_end]
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if data.len() >= head_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn polisher(url: &str) -> Polisher {
    let mut config = Config::default();
    config.llm.endpoint = url.to_owned();
    config.llm.timeout_ms = 400;
    Polisher::new(&config)
}

fn op(seq: u64) -> ModelOp {
    ModelOp { seq, ticket: None }
}

#[tokio::test]
async fn keep_alive_zero_only_for_our_model() {
    let server = FakeOllama::start().await;
    server
        .resident
        .lock()
        .expect("lock")
        .push(OTHER_MODEL.into());
    let polisher = polisher(&server.url);
    polisher.set_keep_alive(Some(Duration::from_secs(420)));

    polisher.load(op(1)).await;
    assert_eq!(polisher.llm_state(), ModelState::Loaded);
    assert_eq!(
        polisher.llm_vram_bytes(),
        Some(3 << 30),
        "VRAM do nosso modelo"
    );
    assert_eq!(
        server.heads(),
        [
            "GET /api/tags",
            "GET /api/ps",
            "POST /api/generate",
            "GET /api/ps"
        ]
    );
    polisher.unload(op(2)).await;
    assert_eq!(polisher.llm_state(), ModelState::Unloaded);

    let generates = server.generates();
    assert_eq!(generates.len(), 2);
    assert_eq!(
        generates[0].1["keep_alive"], 420,
        "carga com a folga do perfil"
    );
    assert_eq!(generates[1].1["keep_alive"], 0);
    for (_, body) in &generates {
        assert_eq!(body["model"], OUR_MODEL, "só o nosso modelo: {body}");
    }
    assert_eq!(
        server.resident.lock().expect("lock").as_slice(),
        [OTHER_MODEL],
        "o modelo de outro cliente continua carregado"
    );

    // Pedido velho não refaz nada.
    polisher.load(op(1)).await;
    assert_eq!(server.generates().len(), 2);
    assert_eq!(polisher.llm_state(), ModelState::Unloaded);
}

#[tokio::test]
async fn model_loaded_by_someone_else_is_never_unloaded() {
    let server = FakeOllama::start().await;
    server.resident.lock().expect("lock").push(OUR_MODEL.into());
    let polisher = polisher(&server.url);
    polisher.load(op(1)).await;
    assert_eq!(polisher.llm_state(), ModelState::Loaded);
    assert_eq!(
        polisher.llm_vram_bytes(),
        None,
        "não conta na VRAM do FastWisper"
    );
    polisher.unload(op(2)).await;
    assert!(
        !server
            .generates()
            .iter()
            .any(|(_, body)| body["keep_alive"] == 0),
        "sem keep_alive 0 num modelo que o FastWisper não carregou"
    );
    assert_eq!(
        server.resident.lock().expect("lock").as_slice(),
        [OUR_MODEL]
    );
}

fn unloads_sent(server: &FakeOllama) -> usize {
    server
        .generates()
        .iter()
        .filter(|(_, body)| body["keep_alive"] == 0)
        .count()
}

#[tokio::test]
async fn unknown_residency_never_makes_the_model_ours() {
    // O modelo é de outro cliente e o `/api/ps` falha de várias formas: na
    // dúvida, o FastWisper não manda `keep_alive: 0`.
    for mode in [
        PsMode::Status500,
        PsMode::NotJson,
        PsMode::NoList,
        PsMode::Slow,
    ] {
        let server = FakeOllama::start().await;
        server.resident.lock().expect("lock").push(OUR_MODEL.into());
        *server.ps.lock().expect("lock") = mode;
        let polisher = polisher(&server.url);
        polisher.load(op(1)).await;
        assert_eq!(polisher.llm_state(), ModelState::Loaded, "{mode:?}");
        assert_eq!(polisher.llm_vram_bytes(), None, "{mode:?}");
        polisher.unload(op(2)).await;
        assert_eq!(unloads_sent(&server), 0, "{mode:?}: keep_alive 0 na dúvida");
        assert_eq!(
            server.resident.lock().expect("lock").as_slice(),
            [OUR_MODEL],
            "{mode:?}"
        );
    }
}

#[tokio::test]
async fn stale_loaded_state_is_reconciled_without_touching_other_clients() {
    // Era de outro cliente; ele descarregou por fora: o próximo press
    // recarrega (agora é nosso) em vez de deixar o chat carregar escondido.
    let server = FakeOllama::start().await;
    server.resident.lock().expect("lock").push(OUR_MODEL.into());
    let polisher = polisher(&server.url);
    polisher.load(op(1)).await;
    let loads = server.generates().len();
    assert_eq!(loads, 1);

    // Ainda residente: o press seguinte não recarrega nem descarrega.
    polisher.load(op(2)).await;
    assert_eq!(server.generates().len(), loads);

    server.resident.lock().expect("lock").clear();
    polisher.load(op(3)).await;
    assert_eq!(polisher.llm_state(), ModelState::Loaded);
    assert_eq!(server.generates().len(), loads + 1, "recarregou");
    assert_eq!(polisher.llm_vram_bytes(), Some(3 << 30), "agora é nosso");
    polisher.unload(op(4)).await;
    assert_eq!(unloads_sent(&server), 1);

    // `/api/ps` falhando com o estado carregado: fica como está.
    let server = FakeOllama::start().await;
    let unsure = self::polisher(&server.url);
    unsure.load(op(1)).await;
    *server.ps.lock().expect("lock") = PsMode::Status500;
    unsure.load(op(2)).await;
    assert_eq!(server.generates().len(), 1, "sem recarga na dúvida");
    assert_eq!(unsure.llm_state(), ModelState::Loaded);
}

#[tokio::test]
async fn llm_not_loaded_in_time_falls_back_to_rules() {
    let server = FakeOllama::start().await;
    *server.load_delay.lock().expect("lock") = Duration::from_millis(1500);
    let polisher = Arc::new(polisher(&server.url));
    polisher.set_keep_alive(Some(Duration::from_secs(420)));

    let loader = Arc::clone(&polisher);
    let load = tokio::spawn(async move { loader.load(op(1)).await });
    let started = Instant::now();
    let out = polisher
        .polish_with(INPUT, Some(Language::Pt), false, LlmUse::OnDemand)
        .await;
    let waited = started.elapsed();
    assert_eq!((out.llm, out.text.as_str()), (LlmOutcome::NotLoaded, INPUT));
    assert!(
        waited < Duration::from_millis(800),
        "o ditado não espera o LLM além do prazo ({waited:?})"
    );
    assert!(out.llm_ms.is_some());
    assert!(
        !server.heads().contains(&"POST /api/chat".to_owned()),
        "nenhuma geração (nem carga implícita) fora da conta"
    );

    load.await.expect("carga");
    let out = polisher
        .polish_with(INPUT, Some(Language::Pt), false, LlmUse::OnDemand)
        .await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Used, "Manda o relatório na quinta.")
    );
    let chat = server
        .requests
        .lock()
        .expect("lock")
        .iter()
        .find(|(_, head, _)| head.starts_with("POST /api/chat "))
        .map(|(_, _, body)| serde_json::from_str::<serde_json::Value>(body).expect("json"))
        .expect("chat");
    assert_eq!(chat["keep_alive"], 420);
}

#[tokio::test]
async fn game_without_llm_uses_only_the_rules() {
    let server = FakeOllama::start().await;
    let polisher = polisher(&server.url);
    let out = polisher
        .polish_with(INPUT, Some(Language::Pt), false, LlmUse::Off)
        .await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::SkippedGame, INPUT)
    );
    assert!(server.heads().is_empty(), "nenhum pedido ao provider");
}

#[tokio::test]
async fn llm_unload_waits_for_the_generation_in_progress() {
    let server = FakeOllama::start().await;
    *server.chat_delay.lock().expect("lock") = Duration::from_millis(250);
    let polisher = Arc::new(polisher(&server.url));
    polisher.load(op(1)).await;

    let formatter = Arc::clone(&polisher);
    let polish = tokio::spawn(async move {
        formatter
            .polish_with(INPUT, Some(Language::Pt), false, LlmUse::OnDemand)
            .await
    });
    wait_until("chat começar", || {
        server.heads().contains(&"POST /api/chat".to_owned())
    })
    .await;
    polisher.unload(op(2)).await;
    let out = polish.await.expect("polish");
    assert_eq!(out.llm, LlmOutcome::Used, "a formatação não foi cortada");
    let requests = server.requests.lock().expect("lock").clone();
    let chat_at = requests
        .iter()
        .find(|(_, head, _)| head.starts_with("POST /api/chat "))
        .map(|(at, _, _)| *at)
        .expect("chat");
    let unload_at = server
        .generates()
        .iter()
        .find(|(_, body)| body["keep_alive"] == 0)
        .map(|(at, _)| *at)
        .expect("descarga");
    assert!(
        unload_at >= chat_at + Duration::from_millis(240),
        "keep_alive 0 só depois da resposta da formatação"
    );
}

#[tokio::test]
async fn resident_profile_keeps_the_model_forever() {
    let server = FakeOllama::start().await;
    let polisher = polisher(&server.url);
    polisher.set_keep_alive(None);
    polisher.load(op(1)).await;
    assert_eq!(server.generates()[0].1["keep_alive"], -1);
}

#[tokio::test]
async fn external_provider_is_not_managed() {
    let mut config = Config::default();
    config.llm.engine = fw_core::config::LlmEngineKind::OpenaiCompatible;
    config.llm.endpoint = "http://127.0.0.1:9".into();
    let polisher = Polisher::new(&config);
    assert_eq!(polisher.llm_state(), ModelState::External);
    polisher.load(op(1)).await;
    polisher.unload(op(2)).await;
    assert_eq!(polisher.llm_state(), ModelState::External);

    config.llm.enabled = false;
    assert_eq!(Polisher::new(&config).llm_state(), ModelState::Disabled);
}

#[test]
fn performance_config_maps_to_the_policy() {
    let config = Config::parse(
        "[performance]\nvram = \"jogo\"\nunload_after_secs = 0\n",
        std::path::Path::new("t.toml"),
    )
    .expect("config");
    let policy = VramPolicy::from_config(&config.performance);
    assert_eq!(policy.mode(), VramMode::Jogo);
    let settings = policy.settings();
    // `game_llm` padrão é `local`: LLM no press, descarregado logo depois.
    assert_eq!(
        (settings.profile, settings.llm, settings.unload_after),
        (Profile::Jogo, true, Some(Duration::ZERO))
    );
}
