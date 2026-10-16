//! Contrato do IPC no M0: socket privado,
//! instância única, envelope versionado, allowlist, limites e erros tipados.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Arc;
use std::time::Duration;

use common::{TestDaemon, exchange, line};
use fw_core::client::{self, ClientError};
use fw_core::daemon::{Daemon, DaemonError, DaemonPaths};
use fw_core::ipc::{MAX_LINE_BYTES, Request};
use fw_core::pipeline::NullPipeline;
use fw_core::sandbox::NetworkSandbox;
use fw_core::session::Command;
use serde_json::{Value, json};

const STATUS: &str = r#"{"v":1,"request_id":"r1","cmd":"status"}"#;

fn code(response: &Value) -> &str {
    response["error"]["code"].as_str().unwrap_or("<sem erro>")
}

async fn null_daemon() -> TestDaemon {
    TestDaemon::start(Arc::new(NullPipeline)).await
}

#[tokio::test]
async fn socket_is_private_and_status_reports_only_state() {
    let daemon = null_daemon().await;

    let mode = fs::metadata(&daemon.paths.socket)
        .expect("socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let mut responses = exchange(&daemon.paths.socket, &[&line(STATUS)]).await;
    // Contagem regressiva da descarga depois do aquecimento do início: o
    // valor depende do relógio; confere só que, se veio, é um número.
    if let Some(vram) = responses[0]["vram"].as_object_mut()
        && let Some(secs) = vram.remove("unload_in_secs")
    {
        assert!(secs.is_u64(), "{secs}");
    }
    assert_eq!(
        responses,
        vec![
            json!({"v":1,"request_id":"r1","ok":true,"state":"idle","generation":0,"asr":"ready","note_session":false,
                "vram":{"mode":"automatico","profile":"sob_demanda","game":false,"asr":"loaded","llm":"disabled"}})
        ],
        "status não carrega texto, janela nem áudio"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn status_reports_the_network_sandbox() {
    let dir = common::runtime_dir();
    let paths = DaemonPaths::in_dir(dir.path());
    let sandbox = NetworkSandbox::Enforced {
        abi: 6,
        connect_ports: vec![11434],
    };
    let daemon = Daemon::bind(paths.clone())
        .expect("bind")
        .with_network_sandbox(sandbox.clone());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(daemon.run(
        Arc::new(NullPipeline),
        Arc::new(common::FakeDesktop::default()),
        async {
            let _ = rx.await;
        },
    ));

    let responses = exchange(&paths.socket, &[&line(STATUS)]).await;
    assert_eq!(
        responses[0]["network"],
        json!({"state":"enforced","abi":6,"connect_ports":[11434]})
    );
    let status = common::ctl(&paths.socket, Command::Status).await;
    assert_eq!(status.network, Some(sandbox));
    // Só o `status` carrega o estado da rede.
    assert_eq!(
        common::ctl(&paths.socket, Command::Press).await.network,
        None
    );

    let _ = tx.send(());
    handle.await.expect("daemon");
}

#[tokio::test]
async fn cli_client_round_trip() {
    let daemon = null_daemon().await;

    let pressed = daemon.ctl(Command::Press).await;
    assert!(pressed.ok);
    assert_eq!(pressed.session_id.map(|s| s.0), Some(1));

    let released = daemon.ctl(Command::Release).await;
    assert!(released.ok);

    daemon.stop().await;
}

#[tokio::test]
async fn invalid_json_keeps_the_connection_usable() {
    let daemon = null_daemon().await;

    let responses = exchange(&daemon.paths.socket, &[b"isto nao e json\n", &line(STATUS)]).await;
    assert_eq!(responses.len(), 2);
    assert_eq!(code(&responses[0]), "invalid_json");
    assert_eq!(responses[0]["request_id"], Value::Null);
    assert_eq!(responses[1]["ok"], true);

    daemon.stop().await;
}

#[tokio::test]
async fn envelope_errors_are_typed() {
    let daemon = null_daemon().await;
    let cases = [
        (
            r#"{"v":2,"request_id":"a","cmd":"status"}"#,
            "unsupported_version",
            Some("a"),
        ),
        (
            r#"{"request_id":"b","cmd":"status"}"#,
            "invalid_request",
            Some("b"),
        ),
        (
            r#"{"v":1,"request_id":"c","cmd":"rm -rf"}"#,
            "unknown_command",
            Some("c"),
        ),
        (
            r#"{"v":1,"request_id":"d","cmd":"paste-last"}"#,
            "unknown_command",
            Some("d"),
        ),
        (
            r#"{"v":1,"request_id":"e","cmd":"status","extra":1}"#,
            "invalid_request",
            Some("e"),
        ),
        (
            r#"{"v":1,"request_id":"f","cmd":"press","args":{"mode":"ptt"}}"#,
            "invalid_args",
            Some("f"),
        ),
        (
            r#"{"v":1,"request_id":"g","cmd":"press","args":"x"}"#,
            "invalid_args",
            Some("g"),
        ),
        (r#"{"v":1,"cmd":"status"}"#, "invalid_request", None),
        (
            r#"{"v":1,"request_id":"tem espaço","cmd":"status"}"#,
            "invalid_request",
            None,
        ),
        (
            r#"{"v":1,"request_id":7,"cmd":"status"}"#,
            "invalid_request",
            None,
        ),
        (r#"[1,2,3]"#, "invalid_request", None),
    ];

    for (request, expected, id) in cases {
        let responses = exchange(&daemon.paths.socket, &[&line(request)]).await;
        assert_eq!(responses.len(), 1, "{request}");
        assert_eq!(responses[0]["ok"], false, "{request}");
        assert_eq!(code(&responses[0]), expected, "{request}");
        assert_eq!(responses[0]["request_id"].as_str(), id, "{request}");
    }

    let long_id = "x".repeat(129);
    let request = format!(r#"{{"v":1,"request_id":"{long_id}","cmd":"status"}}"#);
    let responses = exchange(&daemon.paths.socket, &[&line(&request)]).await;
    assert_eq!(code(&responses[0]), "invalid_request");

    // Nada disso mexeu na sessão.
    assert_eq!(daemon.ctl(Command::Status).await.generation, Some(0));
    daemon.stop().await;
}

#[tokio::test]
async fn empty_args_object_is_accepted() {
    let daemon = null_daemon().await;
    let request = r#"{"v":1,"request_id":"x","cmd":"status","args":{}}"#;
    let responses = exchange(&daemon.paths.socket, &[&line(request)]).await;
    assert_eq!(responses[0]["ok"], true);
    daemon.stop().await;
}

#[tokio::test]
async fn oversized_line_is_rejected_and_connection_closed() {
    let daemon = null_daemon().await;

    let mut big = vec![b'a'; MAX_LINE_BYTES + 10];
    big.push(b'\n');
    let responses = exchange(&daemon.paths.socket, &[&big, &line(STATUS)]).await;
    assert_eq!(
        responses.len(),
        1,
        "não tenta ressincronizar depois do limite"
    );
    assert_eq!(code(&responses[0]), "line_too_long");

    daemon.stop().await;
}

#[tokio::test]
async fn line_exactly_at_the_limit_is_accepted() {
    let daemon = null_daemon().await;

    let mut request = STATUS.as_bytes().to_vec();
    request.resize(MAX_LINE_BYTES, b' ');
    request.push(b'\n');
    let responses = exchange(&daemon.paths.socket, &[&request]).await;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["ok"], true);

    daemon.stop().await;
}

#[tokio::test]
async fn multiple_requests_in_one_write_are_answered_in_order() {
    let daemon = null_daemon().await;

    let batch = [
        r#"{"v":1,"request_id":"p","cmd":"press"}"#,
        r#"{"v":1,"request_id":"s","cmd":"status"}"#,
        r#"{"v":1,"request_id":"c","cmd":"cancel"}"#,
    ]
    .join("\n")
        + "\n";
    let responses = exchange(&daemon.paths.socket, &[batch.as_bytes()]).await;
    let ids: Vec<_> = responses.iter().map(|r| r["request_id"].as_str()).collect();
    assert_eq!(ids, [Some("p"), Some("s"), Some("c")]);
    assert_eq!(responses[1]["state"], "recording");
    assert_eq!(responses[2]["state"], "idle");

    daemon.stop().await;
}

#[tokio::test]
async fn fragmented_line_is_reassembled() {
    let daemon = null_daemon().await;

    let responses = exchange(
        &daemon.paths.socket,
        &[
            b"{\"v\":1,\"requ",
            b"est_id\":\"frag\",\"cm",
            b"d\":\"status\"}\n",
        ],
    )
    .await;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["request_id"], "frag");
    assert_eq!(responses[0]["ok"], true);

    daemon.stop().await;
}

#[tokio::test]
async fn request_without_trailing_newline_is_answered_at_eof() {
    let daemon = null_daemon().await;
    let responses = exchange(&daemon.paths.socket, &[STATUS.as_bytes()]).await;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["ok"], true);
    daemon.stop().await;
}

#[tokio::test]
async fn partial_line_times_out() {
    let daemon = TestDaemon::start_with(Arc::new(NullPipeline), Duration::from_millis(150)).await;

    let mut stream = tokio::net::UnixStream::connect(&daemon.paths.socket)
        .await
        .expect("connect");
    tokio::io::AsyncWriteExt::write_all(&mut stream, br#"{"v":1,"#)
        .await
        .expect("write");
    // Sem fechar a escrita: o daemon espera o resto e desiste.
    let responses = common::read_lines(&mut stream).await;
    assert_eq!(responses.len(), 1);
    assert_eq!(code(&responses[0]), "timeout");

    daemon.stop().await;
}

#[tokio::test]
async fn second_instance_is_refused() {
    let daemon = null_daemon().await;

    match Daemon::bind(daemon.paths.clone()) {
        Err(DaemonError::AlreadyRunning(_)) => {}
        Err(other) => panic!("erro inesperado: {other}"),
        Ok(_) => panic!("segunda instância não pode subir"),
    }
    // A primeira continua atendendo.
    assert!(daemon.ctl(Command::Status).await.ok);

    daemon.stop().await;
}

#[tokio::test]
async fn stale_socket_from_a_dead_daemon_is_replaced() {
    let dir = common::runtime_dir();
    let paths = DaemonPaths::in_dir(dir.path());
    // Socket que sobrou de um processo morto (sem lock).
    drop(std::os::unix::net::UnixListener::bind(&paths.socket).expect("bind"));
    assert!(paths.socket.exists());

    let daemon = Daemon::bind(paths.clone()).expect("substitui o socket antigo");
    drop(daemon);
}

#[tokio::test]
async fn foreign_files_at_the_socket_path_are_not_touched() {
    let dir = common::runtime_dir();
    let paths = DaemonPaths::in_dir(dir.path());

    // Arquivo comum.
    fs::write(&paths.socket, b"importante").expect("write");
    assert!(matches!(
        Daemon::bind(paths.clone()),
        Err(DaemonError::UnsafeSocketPath(_))
    ));
    assert_eq!(fs::read(&paths.socket).expect("read"), b"importante");
    fs::remove_file(&paths.socket).expect("rm");

    // Symlink para outro arquivo: não segue nem remove o alvo.
    let target = dir.path().join("alvo");
    fs::write(&target, b"alvo").expect("write");
    symlink(&target, &paths.socket).expect("symlink");
    assert!(matches!(
        Daemon::bind(paths.clone()),
        Err(DaemonError::UnsafeSocketPath(_))
    ));
    assert_eq!(fs::read(&target).expect("read"), b"alvo");
}

#[tokio::test]
async fn insecure_runtime_dir_is_refused() {
    let dir = common::runtime_dir();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("chmod");

    let result = Daemon::bind(DaemonPaths::in_dir(dir.path()));
    assert!(matches!(
        result,
        Err(DaemonError::InsecureRuntimeDir { .. })
    ));
    assert!(!dir.path().join("fastwisper.sock").exists());
}

#[test]
fn client_reports_daemon_not_running() {
    let dir = common::runtime_dir();
    let socket = DaemonPaths::in_dir(dir.path()).socket;
    let result = client::send(&socket, &Request::new("x", Command::Status));
    assert!(matches!(result, Err(ClientError::NotRunning(_))));
}

// --- Fase 2: nível do microfone para a pílula (`meter`) ---

mod meter {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use fw_core::daemon::METER_INTERVAL;
    use fw_core::session::Command;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
    use tokio::net::UnixStream;

    use super::common::{GatedPipeline, TestDaemon};

    struct Stream {
        lines: Lines<BufReader<UnixStream>>,
    }

    impl Stream {
        async fn open(daemon: &TestDaemon, cmd: &str) -> Stream {
            let mut stream = UnixStream::connect(&daemon.paths.socket)
                .await
                .expect("connect");
            let request = format!("{{\"v\":1,\"request_id\":\"{cmd}\",\"cmd\":\"{cmd}\"}}\n");
            stream.write_all(request.as_bytes()).await.expect("write");
            Stream {
                lines: BufReader::new(stream).lines(),
            }
        }

        /// Próxima linha em até `wait`; `None` se nada chegou.
        async fn within(&mut self, wait: Duration) -> Option<Value> {
            match tokio::time::timeout(wait, self.lines.next_line()).await {
                Ok(line) => {
                    let line = line.expect("leitura").expect("conexão aberta");
                    Some(serde_json::from_str(&line).expect("json"))
                }
                Err(_) => None,
            }
        }

        async fn next(&mut self) -> Value {
            self.within(Duration::from_secs(3))
                .await
                .expect("evento a tempo")
        }
    }

    fn keys(event: &Value) -> BTreeSet<String> {
        event.as_object().expect("objeto").keys().cloned().collect()
    }

    /// ~25 Hz sem contar eventos numa janela de parede (instável sob carga:
    /// com a thread sem CPU, o `interval` pula tiques e vinham menos de 18
    /// em 1 s). O contrato verificável de forma determinística: o intervalo
    /// é [`METER_INTERVAL`] (40 ms), N eventos nunca chegam antes de
    /// (N-1) intervalos desde o press (o `interval` não adianta nem acumula
    /// tiques perdidos: `MissedTickBehavior::Skip`), e as transições são
    /// esperadas por evento, não por tempo.
    #[tokio::test]
    async fn level_streams_at_about_25_hz_only_while_recording() {
        let pipeline = Arc::new(GatedPipeline::default());
        // O "microfone" reporta nível mesmo parado: quem filtra é o daemon.
        pipeline.set_level(0.42);
        let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
        let mut meter = Stream::open(&daemon, "meter").await;

        assert_eq!(
            meter.within(Duration::from_millis(400)).await,
            None,
            "fora da gravação não publica nada"
        );

        assert_eq!(METER_INTERVAL, Duration::from_millis(40), "25 Hz");
        let pressed = Instant::now();
        daemon.ctl(Command::Press).await;
        let mut events = Vec::new();
        for _ in 0..25 {
            events.push(meter.next().await);
        }
        let took = pressed.elapsed();
        assert!(
            took >= METER_INTERVAL * 24,
            "25 eventos em {took:?}: mais rápido que 25 Hz"
        );
        for event in &events {
            assert_eq!(event["state"], "recording");
            assert_eq!(event["session_id"], 1);
            assert_eq!(event["level"], 0.42);
            assert_eq!(event["v"], 1);
            // Só o número: nada de áudio, texto ou janela.
            let allowed: BTreeSet<String> = ["v", "state", "session_id", "level"]
                .map(String::from)
                .into();
            assert!(keys(event).is_subset(&allowed), "{event}");
        }

        // Fim da gravação: um evento com o estado novo e nível 0, e silêncio.
        daemon.ctl(Command::Cancel).await;
        let end = loop {
            let event = meter.next().await;
            if event["state"] != "recording" {
                break event;
            }
        };
        assert_eq!(end["state"], "idle");
        assert_eq!(end["level"], 0.0);
        assert_eq!(meter.within(Duration::from_millis(400)).await, None);

        // A mesma conexão segue a próxima gravação, até o release.
        daemon.ctl(Command::Press).await;
        let second = meter.next().await;
        assert_eq!(second["state"], "recording");
        assert_eq!(second["session_id"], 2);
        pipeline.set_level(0.0);
        // Espera pelo evento (não por tempo) o primeiro tique já em silêncio.
        loop {
            let event = meter.next().await;
            assert_eq!(event["state"], "recording");
            if event["level"] == 0.0 {
                break;
            }
        }
        daemon.ctl(Command::Release).await;
        let end = loop {
            let event = meter.next().await;
            if event["state"] != "recording" {
                break event;
            }
            assert_eq!(event["level"], 0.0, "nível 0 no silêncio");
        };
        assert_eq!(end["state"], "transcribing");
        assert_eq!(end["level"], 0.0);

        pipeline.release_one();
        daemon.stop().await;
    }

    #[tokio::test]
    async fn subscribe_does_not_receive_the_meter() {
        let pipeline = Arc::new(GatedPipeline::default());
        let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
        let mut bar = Stream::open(&daemon, "subscribe").await;
        let mut meter = Stream::open(&daemon, "meter").await;
        assert_eq!(bar.next().await["state"], "idle");

        daemon.ctl(Command::Press).await;
        let recording = bar.next().await;
        assert_eq!(recording["state"], "recording");
        assert!(recording.get("level").is_none(), "{recording}");

        // Nível variando: o medidor recebe cada tique (contados por evento,
        // não numa janela de parede), a barra nada.
        let mut level = 0.0;
        for _ in 0..25 {
            level = (level + 0.1) % 1.0;
            pipeline.set_level(level);
            assert_eq!(meter.next().await["state"], "recording");
        }
        assert_eq!(
            bar.within(Duration::from_millis(100)).await,
            None,
            "a barra não recebe 25 Hz"
        );

        daemon.ctl(Command::Cancel).await;
        assert_eq!(bar.next().await["state"], "idle");
        daemon.stop().await;
    }

    #[tokio::test]
    async fn a_meter_client_that_never_reads_cannot_stall_dictation() {
        let pipeline = Arc::new(GatedPipeline::default());
        pipeline.set_level(0.5);
        let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
        let _meter = Stream::open(&daemon, "meter").await;

        let started = Instant::now();
        for _ in 0..50 {
            daemon.ctl(Command::Press).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            daemon.ctl(Command::Cancel).await;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(daemon.ctl(Command::Status).await.generation, Some(100));
        daemon.stop().await;
    }
}

/// Primeiro evento do `subscribe` (o estado atual).
async fn subscribed(socket: &std::path::Path) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .expect("connect");
    stream
        .write_all(&Request::new("sub", Command::Subscribe).to_line())
        .await
        .expect("write");
    let mut first = String::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        BufReader::new(stream).read_line(&mut first),
    )
    .await
    .expect("evento")
    .expect("read");
    serde_json::from_str(&first).expect("json")
}

/// ASR que falhou ao carregar já no início do daemon.
struct BrokenAsr;

impl fw_core::pipeline::Pipeline for BrokenAsr {
    fn start(
        &self,
        _ticket: fw_core::session::Ticket,
    ) -> impl Future<Output = Result<(), fw_core::pipeline::Failure>> + Send + 'static {
        std::future::ready(Err(fw_core::pipeline::Failure::AsrUnavailable))
    }

    fn finish(
        &self,
        _ticket: fw_core::session::Ticket,
        _literal: bool,
    ) -> impl Future<Output = fw_core::pipeline::Outcome> + Send + 'static {
        std::future::ready(fw_core::pipeline::Outcome::NoSpeech(
            fw_core::pipeline::Metrics::default(),
        ))
    }

    fn abort(&self, _ticket: fw_core::session::Ticket) {}

    fn readiness(&self) -> fw_core::pipeline::Readiness {
        fw_core::pipeline::Readiness::Failed
    }
}

/// `failures` (A-03): conta ditados que falharam, inclusive dois iguais
/// seguidos; o ASR que falhou no início do daemon e o cancel não contam.
#[tokio::test]
async fn subscribe_counts_failed_dictations_only() {
    let daemon = TestDaemon::start(Arc::new(BrokenAsr)).await;
    let event = subscribed(&daemon.paths.socket).await;
    assert_eq!(event["asr"], "failed");
    assert_eq!(event["failures"], 0, "{event}");
    assert!(daemon.ctl(Command::Press).await.error.is_some());
    assert_eq!(subscribed(&daemon.paths.socket).await["failures"], 1);
    daemon.stop().await;

    let pipeline = Arc::new(common::GatedPipeline::default());
    let daemon = TestDaemon::start(Arc::clone(&pipeline)).await;
    assert_eq!(subscribed(&daemon.paths.socket).await["failures"], 0);
    pipeline.refuse_start(Some(fw_core::pipeline::Failure::DeviceUnavailable));
    let mut lasts = Vec::new();
    for expected in 1..=2 {
        assert!(daemon.ctl(Command::Press).await.error.is_some());
        let event = subscribed(&daemon.paths.socket).await;
        assert_eq!(event["failures"], expected, "{event}");
        lasts.push(event["last"].clone());
    }
    assert_eq!(lasts[0], lasts[1], "duas falhas idênticas, mesmo resumo");
    // Cancel: a gravação descartada não é falha nova.
    pipeline.refuse_start(None);
    assert!(daemon.ctl(Command::Press).await.error.is_none());
    assert!(daemon.ctl(Command::Cancel).await.error.is_none());
    let event = subscribed(&daemon.paths.socket).await;
    assert_eq!(event["state"], "idle");
    assert_eq!(event["failures"], 2, "{event}");
    assert_eq!(event["last"], lasts[1]);
    daemon.stop().await;
}
