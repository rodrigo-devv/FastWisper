//! Contrato de sessão: idempotência, ordens de
//! soltura, cancelamento, generations e completions atrasados.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{GatedPipeline, MARKER_TEXT, TestDaemon};
use fw_core::ipc::ErrorCode;
use fw_core::pipeline::{Failure, Metrics, Outcome, OutcomeKind};
use fw_core::session::{
    Command, Effect, Mode, Rejection, Session, SessionId, State, StateName, Ticket,
};

fn handle(session: &mut Session, cmd: Command) -> Option<Effect> {
    session.handle(cmd).expect("comando aceito").effect
}

fn recording_ticket(session: &Session) -> Ticket {
    match session.state() {
        State::Recording { ticket, .. } => ticket,
        other => panic!("esperava gravação, estado {other:?}"),
    }
}

#[test]
fn starts_idle_with_generation_zero() {
    let snapshot = Session::new().snapshot();
    assert_eq!(snapshot.state, StateName::Idle);
    assert_eq!(snapshot.generation, 0);
    assert_eq!(snapshot.session_id, None);
}

#[test]
fn repeated_press_keeps_the_same_recording() {
    let mut s = Session::new();
    let first = handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);
    assert_eq!(first, Some(Effect::StartCapture(ticket)));

    for _ in 0..3 {
        assert_eq!(handle(&mut s, Command::Press), None);
    }
    assert_eq!(recording_ticket(&s), ticket);
    assert_eq!(s.snapshot().generation, ticket.generation);
}

#[test]
fn release_without_recording_is_noop() {
    let mut s = Session::new();
    let before = s.snapshot();
    assert_eq!(handle(&mut s, Command::Release), None);
    assert_eq!(s.snapshot(), before);
}

#[test]
fn duplicate_release_from_alt_and_space_stops_once() {
    // Soltar Space antes de Alt, ou Alt antes de Space, dispara dois
    // releases (bindr de Space e bindr de ALT_L). Só o primeiro encerra.
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);

    assert_eq!(
        handle(&mut s, Command::Release),
        Some(Effect::FinishCapture(ticket))
    );
    assert_eq!(handle(&mut s, Command::Release), None);
    assert_eq!(s.state(), State::Transcribing { ticket });
}

#[test]
fn toggle_does_not_convert_a_ptt_session() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);

    assert_eq!(handle(&mut s, Command::Toggle), None);
    assert_eq!(
        s.state(),
        State::Recording {
            ticket,
            mode: Mode::Ptt
        }
    );
    assert_eq!(
        handle(&mut s, Command::Release),
        Some(Effect::FinishCapture(ticket))
    );
}

#[test]
fn release_does_not_stop_hands_free() {
    // Soltar o Alt depois de Alt+Shift+Space não encerra o hands-free.
    let mut s = Session::new();
    handle(&mut s, Command::Toggle);
    let ticket = recording_ticket(&s);

    assert_eq!(handle(&mut s, Command::Release), None);
    assert_eq!(handle(&mut s, Command::Press), None);
    assert_eq!(
        s.state(),
        State::Recording {
            ticket,
            mode: Mode::Toggle
        }
    );

    assert_eq!(
        handle(&mut s, Command::Toggle),
        Some(Effect::FinishCapture(ticket))
    );
}

#[test]
fn new_recording_while_processing_is_busy() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    handle(&mut s, Command::Release);
    let before = s.snapshot();

    assert_eq!(s.handle(Command::Press), Err(Rejection::Busy));
    assert_eq!(s.handle(Command::Toggle), Err(Rejection::Busy));
    assert_eq!(s.snapshot(), before, "recusa não cria fila nem muda estado");
}

#[test]
fn cancel_during_recording_invalidates_generation() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);

    assert_eq!(handle(&mut s, Command::Cancel), Some(Effect::Abort(ticket)));
    assert_eq!(s.state(), State::Idle);
    assert!(s.snapshot().generation > ticket.generation);
}

#[test]
fn cancel_during_transcription_discards_the_late_result() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);
    handle(&mut s, Command::Release);

    assert_eq!(handle(&mut s, Command::Cancel), Some(Effect::Abort(ticket)));
    assert!(!s.complete(ticket), "completion após cancel é descartado");
    assert_eq!(s.state(), State::Idle);
    assert_eq!(s.stale_completions(), 1);
}

#[test]
fn late_result_never_lands_in_a_new_session() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let old = recording_ticket(&s);
    handle(&mut s, Command::Release);
    handle(&mut s, Command::Cancel);

    handle(&mut s, Command::Press);
    let new = recording_ticket(&s);
    assert_ne!(new.session_id, old.session_id);

    assert!(!s.complete(old));
    assert_eq!(
        s.state(),
        State::Recording {
            ticket: new,
            mode: Mode::Ptt
        }
    );

    handle(&mut s, Command::Release);
    assert!(
        !s.complete(old),
        "ticket antigo continua inválido no processamento novo"
    );
    assert_eq!(s.state(), State::Transcribing { ticket: new });
    assert!(s.complete(new));
    assert_eq!(s.state(), State::Idle);
}

#[test]
fn completion_of_current_session_returns_to_idle() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);
    handle(&mut s, Command::Release);

    assert!(s.complete(ticket));
    assert_eq!(s.state(), State::Idle);
    assert!(
        !s.complete(ticket),
        "o mesmo resultado não é aceito duas vezes"
    );
}

#[test]
fn completion_while_still_recording_is_rejected() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);
    assert!(!s.complete(ticket));
    assert_eq!(
        s.state(),
        State::Recording {
            ticket,
            mode: Mode::Ptt
        }
    );
}

#[test]
fn session_ids_and_generations_are_monotonic() {
    let mut s = Session::new();
    let mut last: Option<Ticket> = None;
    for round in 0..5 {
        handle(&mut s, Command::Press);
        let ticket = recording_ticket(&s);
        if let Some(prev) = last {
            assert!(ticket.session_id > prev.session_id);
            assert!(ticket.generation > prev.generation);
        }
        last = Some(ticket);
        if round % 2 == 0 {
            handle(&mut s, Command::Cancel);
        } else {
            handle(&mut s, Command::Release);
            assert!(s.complete(ticket));
        }
    }
    assert_eq!(last.map(|t| t.session_id), Some(SessionId(5)));
}

#[test]
fn cancel_when_idle_is_noop() {
    let mut s = Session::new();
    let before = s.snapshot();
    assert_eq!(handle(&mut s, Command::Cancel), None);
    assert_eq!(s.snapshot(), before);
}

#[test]
fn shutdown_invalidates_the_active_session() {
    let mut s = Session::new();
    assert_eq!(s.shutdown(), None);

    handle(&mut s, Command::Press);
    let ticket = recording_ticket(&s);
    handle(&mut s, Command::Release);
    assert_eq!(s.shutdown(), Some(Effect::Abort(ticket)));
    assert!(!s.complete(ticket));
}

#[test]
fn status_is_free_of_side_effects() {
    let mut s = Session::new();
    handle(&mut s, Command::Press);
    let before = s.snapshot();
    assert_eq!(handle(&mut s, Command::Status), None);
    assert_eq!(s.snapshot(), before);
}

// --- Pelo daemon: o actor serializa comandos concorrentes ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_presses_start_exactly_one_session() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let socket = daemon.paths.socket.clone();
        tasks.push(tokio::spawn(async move {
            common::ctl(&socket, Command::Press).await
        }));
    }
    let mut sessions = Vec::new();
    for task in tasks {
        let response = task.await.expect("task");
        assert!(response.ok, "{response:?}");
        assert_eq!(response.state, Some(StateName::Recording));
        sessions.push(response.session_id);
    }
    sessions.dedup();
    assert_eq!(sessions.len(), 1, "todos os press caem na mesma sessão");
    assert_eq!(pipeline.started.load(Ordering::SeqCst), 1);

    daemon.stop().await;
}

#[tokio::test]
async fn press_while_processing_answers_busy_then_recovers() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    daemon.ctl(Command::Press).await;
    let released = daemon.ctl(Command::Release).await;
    assert_eq!(released.state, Some(StateName::Transcribing));

    let busy = daemon.ctl(Command::Press).await;
    assert!(!busy.ok);
    assert_eq!(busy.error_code(), Some(ErrorCode::Busy));
    assert_eq!(busy.state, Some(StateName::Transcribing));

    pipeline.release_one();
    wait_for_state(&daemon, StateName::Idle).await;

    let again = daemon.ctl(Command::Press).await;
    assert!(again.ok);
    assert_ne!(again.session_id, released.session_id);

    daemon.stop().await;
}

#[tokio::test]
async fn shutdown_with_busy_worker_aborts_and_cleans_up() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
    let socket = daemon.paths.socket.clone();

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    assert_eq!(pipeline.finishing.load(Ordering::SeqCst), 1);

    let _dir = daemon.stop().await;
    assert_eq!(pipeline.aborted.load(Ordering::SeqCst), 1);
    assert!(!socket.exists(), "socket removido no encerramento");
}

async fn wait_for_state(daemon: &TestDaemon, state: StateName) {
    for _ in 0..100 {
        if daemon.ctl(Command::Status).await.state == Some(state) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon não chegou ao estado {state}");
}

// --- M2: microfone recusado, resultado recuperável e privacidade ---

async fn transcribe_once(daemon: &TestDaemon, pipeline: &GatedPipeline) {
    assert!(daemon.ctl(Command::Press).await.ok);
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_for_state(daemon, StateName::Idle).await;
}

#[tokio::test]
async fn missing_microphone_refuses_press_and_stays_idle() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    pipeline.refuse_start(Some(Failure::DeviceUnavailable));
    let refused = daemon.ctl(Command::Press).await;
    assert!(!refused.ok);
    assert_eq!(refused.error_code(), Some(ErrorCode::DeviceUnavailable));
    assert_eq!(refused.state, Some(StateName::Idle));
    assert_eq!(pipeline.started.load(Ordering::SeqCst), 0);

    let status = daemon.ctl(Command::Status).await;
    assert_eq!(
        status.last.map(|l| l.outcome.as_str()),
        Some("device_unavailable".into())
    );

    // Reconectar o microfone é explícito: o próximo press funciona.
    pipeline.refuse_start(None);
    assert!(daemon.ctl(Command::Press).await.ok);
    daemon.stop().await;
}

#[tokio::test]
async fn text_only_comes_back_through_show_last() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
    transcribe_once(&daemon, &pipeline).await;

    let status = daemon.ctl(Command::Status).await;
    let line = String::from_utf8(status.to_line()).expect("utf8");
    assert!(!line.contains(MARKER_TEXT), "status vazou o texto: {line}");
    assert_eq!(status.text, None);
    let last = status.last.expect("resumo");
    assert_eq!(last.outcome, OutcomeKind::TRANSCRIBED);

    let shown = daemon.ctl(Command::ShowLast).await;
    assert!(shown.ok);
    assert_eq!(shown.text.as_deref(), Some(MARKER_TEXT));

    daemon.stop().await;
}

#[tokio::test]
async fn cancel_erases_the_recoverable_text() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
    transcribe_once(&daemon, &pipeline).await;

    assert!(daemon.ctl(Command::Cancel).await.ok);
    let shown = daemon.ctl(Command::ShowLast).await;
    assert_eq!(shown.error_code(), Some(ErrorCode::NoResult));
    assert_eq!(shown.text, None);

    daemon.stop().await;
}

#[tokio::test]
async fn result_of_a_cancelled_session_never_shows_up() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    daemon.ctl(Command::Cancel).await;
    // O worker antigo termina depois do cancel (o fake libera no abort).
    tokio::time::sleep(Duration::from_millis(50)).await;

    let shown = daemon.ctl(Command::ShowLast).await;
    assert_eq!(shown.error_code(), Some(ErrorCode::NoResult));
    assert_ne!(
        daemon.ctl(Command::Status).await.last.map(|l| l.outcome),
        Some(OutcomeKind::TRANSCRIBED)
    );
    daemon.stop().await;
}

#[tokio::test]
async fn silence_leaves_no_text_behind() {
    let pipeline = Arc::new(GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
    transcribe_once(&daemon, &pipeline).await;

    pipeline.set_outcome(Outcome::NoSpeech(Metrics::default()));
    transcribe_once(&daemon, &pipeline).await;

    let status = daemon.ctl(Command::Status).await;
    assert_eq!(status.last.map(|l| l.outcome), Some(OutcomeKind::NO_SPEECH));
    let shown = daemon.ctl(Command::ShowLast).await;
    assert_eq!(
        shown.error_code(),
        Some(ErrorCode::NoResult),
        "texto anterior não reaparece"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn stuck_key_closes_the_microphone_at_the_limit() {
    // `press` sem `release` (tecla presa, Alt solto fora de ordem): o
    // microfone não pode ficar aberto indefinidamente.
    let pipeline = Arc::new(GatedPipeline::default());
    *pipeline.max_recording.lock().expect("lock") = Duration::from_millis(150);
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    let pressed = daemon.ctl(Command::Press).await;
    assert!(pressed.ok);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let status = daemon.ctl(Command::Status).await;
    assert_eq!(status.state, Some(StateName::Idle));
    assert_eq!(
        status.last.map(|l| l.outcome.as_str()),
        Some("recording_too_long".into())
    );
    assert_eq!(pipeline.aborted.load(Ordering::SeqCst), 1);
    assert_eq!(
        pipeline.finishing.load(Ordering::SeqCst),
        0,
        "nada é transcrito"
    );

    // O release atrasado vira no-op; a sessão seguinte funciona normalmente.
    assert_eq!(
        daemon.ctl(Command::Release).await.state,
        Some(StateName::Idle)
    );
    *pipeline.max_recording.lock().expect("lock") = Duration::from_secs(300);
    assert!(daemon.ctl(Command::Press).await.ok);
    daemon.stop().await;
}

#[tokio::test]
async fn deadline_of_a_finished_session_does_not_touch_the_next_one() {
    let pipeline = Arc::new(GatedPipeline::default());
    *pipeline.max_recording.lock().expect("lock") = Duration::from_millis(200);
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;

    transcribe_once(&daemon, &pipeline).await;
    *pipeline.max_recording.lock().expect("lock") = Duration::from_secs(300);
    let second = daemon.ctl(Command::Press).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let status = daemon.ctl(Command::Status).await;
    assert_eq!(
        status.state,
        Some(StateName::Recording),
        "prazo antigo não aborta a sessão nova"
    );
    assert_eq!(status.session_id, second.session_id);
    daemon.stop().await;
}

// --- M5: status ao vivo para a barra (`subscribe`) ---

mod subscribe {
    use std::sync::Arc;
    use std::time::Duration;

    use fw_core::session::{Command, StateName};
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    use super::common::{GatedPipeline, MARKER_TEXT, TestDaemon};

    struct Sub {
        lines: tokio::io::Lines<BufReader<UnixStream>>,
    }

    impl Sub {
        async fn open(daemon: &TestDaemon) -> Sub {
            let mut stream = UnixStream::connect(&daemon.paths.socket)
                .await
                .expect("connect");
            stream
                .write_all(b"{\"v\":1,\"request_id\":\"sub\",\"cmd\":\"subscribe\"}\n")
                .await
                .expect("write");
            Sub {
                lines: BufReader::new(stream).lines(),
            }
        }

        async fn next(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(3), self.lines.next_line())
                .await
                .expect("evento a tempo")
                .expect("leitura")
                .expect("conexão aberta");
            assert!(!line.contains(MARKER_TEXT), "evento vazou texto: {line}");
            serde_json::from_str(&line).expect("json")
        }

        /// Lê até o estado pedido (eventos intermediários podem vir).
        async fn until_state(&mut self, state: &str) -> Value {
            for _ in 0..20 {
                let event = self.next().await;
                if event["state"] == state {
                    return event;
                }
            }
            panic!("estado {state} não chegou");
        }
    }

    #[tokio::test]
    async fn bar_sees_every_state_change_without_text() {
        let pipeline = Arc::new(GatedPipeline::default());
        let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
        let mut sub = Sub::open(&daemon).await;

        let first = sub.next().await;
        assert_eq!(first["state"], "idle");
        assert_eq!(first["asr"], "ready");
        let first_seq = first["seq"].as_u64().expect("seq");

        daemon.ctl(Command::Press).await;
        let recording = sub.until_state("recording").await;
        assert!(recording["seq"].as_u64().expect("seq") > first_seq);

        daemon.ctl(Command::Release).await;
        sub.until_state("transcribing").await;
        pipeline.release_one();
        let done = sub.until_state("idle").await;
        assert_eq!(done["last"]["outcome"], "transcribed");
        assert!(done.get("text").is_none());

        daemon.stop().await;
    }

    #[tokio::test]
    async fn a_subscriber_that_never_reads_cannot_stall_dictation() {
        let pipeline = Arc::new(GatedPipeline::default());
        let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
        let mut sub = Sub::open(&daemon).await;
        sub.next().await;

        // 200 mudanças sem ler nada: o actor nunca espera o assinante.
        let started = std::time::Instant::now();
        for _ in 0..100 {
            daemon.ctl(Command::Press).await;
            daemon.ctl(Command::Cancel).await;
        }
        daemon.ctl(Command::Press).await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            daemon.ctl(Command::Status).await.state,
            Some(StateName::Recording)
        );

        // Quando volta a ler, chega ao estado atual com seq crescente.
        let mut last_seq = 0;
        let mut latest = Value::Null;
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_millis(300), sub.lines.next_line()).await
        {
            latest = serde_json::from_str(&line).expect("json");
            let seq = latest["seq"].as_u64().expect("seq");
            assert!(seq > last_seq, "seq sempre crescente");
            last_seq = seq;
        }
        assert_eq!(latest["state"], "recording");
        daemon.stop().await;
    }
}
