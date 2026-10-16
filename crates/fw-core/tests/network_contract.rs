//! Contrato de rede do LLM, contra um
//! servidor HTTP falso em loopback: prazo, stream quebrado, modelo ausente,
//! redirect, saída gigante, guarda, provider OpenAI-compatível e as bordas
//! de quando o LLM nem é chamado. Em toda falha o texto das regras sai.

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fw_core::asr::Language;
use fw_core::config::{Config, LlmEngineKind, Snippet};
use fw_core::llm::LlmOutcome;
use fw_core::polish::Polisher;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
enum Reply {
    /// Linhas NDJSON do `/api/chat`, com pausa antes de cada uma.
    Ollama(Vec<(u64, String)>),
    Status(u16, &'static str),
    Redirect(String),
    Json(String),
    /// Cabeçalho e o começo do stream, e depois nada até o cliente desistir
    /// e fechar a conexão, o que é avisado em `hung_up`.
    Hang(Arc<tokio::sync::Notify>),
}

struct Fake {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    /// Resposta de `GET /api/tags`.
    inventory: Arc<Mutex<Reply>>,
    /// Resposta de todo o resto (`/api/chat`, `/api/generate`...).
    reply: Arc<Mutex<Reply>>,
}

impl Fake {
    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("lock")
            .iter()
            .map(|r| r.lines().next().unwrap_or_default().to_owned())
            .collect()
    }

    fn count(&self, path: &str) -> usize {
        self.paths().iter().filter(|p| p.as_str() == path).count()
    }

    fn set_inventory(&self, models: serde_json::Value) {
        *self.inventory.lock().expect("inventory") = inventory(models);
    }

    fn set_reply(&self, reply: Reply) {
        *self.reply.lock().expect("reply") = reply;
    }
}

const TAGS: &str = "GET /api/tags HTTP/1.1";
const CHAT: &str = "POST /api/chat HTTP/1.1";
const GENERATE: &str = "POST /api/generate HTTP/1.1";

fn inventory(models: serde_json::Value) -> Reply {
    Reply::Json(serde_json::json!({ "models": models }).to_string())
}

fn pinned() -> serde_json::Value {
    serde_json::json!([{"name": fw_core::models::OLLAMA_MODEL, "digest": fw_core::models::OLLAMA_DIGEST}])
}

async fn fake(reply: Reply) -> Fake {
    fake_with_inventory(reply, pinned()).await
}

async fn fake_with_inventory(reply: Reply, models: serde_json::Value) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    let inventory = Arc::new(Mutex::new(inventory(models)));
    let serve_inventory = Arc::clone(&inventory);
    let reply = Arc::new(Mutex::new(reply));
    let serve_reply = Arc::clone(&reply);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let inventory = Arc::clone(&serve_inventory);
            let reply = Arc::clone(&serve_reply);
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let request = read_request(&mut socket).await;
                let reply = if request.starts_with("GET /api/tags ") {
                    inventory.lock().expect("inventory").clone()
                } else {
                    reply.lock().expect("reply").clone()
                };
                seen.lock().expect("lock").push(request);
                let _ = respond(&mut socket, reply).await;
            });
        }
    });
    Fake {
        url,
        requests,
        inventory,
        reply,
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

/// Um pedido por conexão, então toda resposta diz `connection: close`: sem
/// isso o cliente devolve a conexão ao pool e o pedido seguinte (o
/// `/api/chat` depois do `/api/tags`) pode sair por ela já fechada, o que
/// vira `Unavailable` sob carga (A-24, `proxy_environment_child`).
async fn respond(socket: &mut tokio::net::TcpStream, reply: Reply) -> std::io::Result<()> {
    match reply {
        Reply::Ollama(lines) => {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n")
                .await?;
            for (delay, line) in lines {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let chunk = format!("{line}\n");
                socket.write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes()).await?;
            }
            socket.write_all(b"0\r\n\r\n").await
        }
        Reply::Status(code, body) => {
            socket
                .write_all(format!("HTTP/1.1 {code} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes())
                .await
        }
        Reply::Redirect(to) => {
            socket
                .write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes())
                .await
        }
        Reply::Json(body) => {
            socket
                .write_all(
                    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                        .as_bytes(),
                )
                .await
        }
        Reply::Hang(hung_up) => {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n")
                .await?;
            let chunk = format!("{}\n", piece("<out>").1);
            socket.write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes()).await?;
            let mut buf = [0u8; 64];
            while matches!(socket.read(&mut buf).await, Ok(n) if n > 0) {}
            hung_up.notify_one();
            Ok(())
        }
    }
}

fn piece(content: &str) -> (u64, String) {
    (
        0,
        serde_json::json!({"message": {"content": content}, "done": false}).to_string(),
    )
}

fn done() -> (u64, String) {
    (0, r#"{"message":{"content":""},"done":true}"#.to_owned())
}

fn ollama_says(out: &str) -> Reply {
    Reply::Ollama(vec![piece("<out>"), piece(out), piece("</out>"), done()])
}

/// Prazo longo: o servidor falso responde na hora, e o assunto da maioria
/// dos testes não é o prazo. Com os 400 ms de antes, carga na máquina (build
/// paralelo, outro processo de teste) virava `Timeout` em vez de `Used`. Quem
/// testa o prazo usa [`SHORT_DEADLINE_MS`] e não mede parede contra um teto.
fn config(url: &str) -> Config {
    let mut config = Config::default();
    config.llm.endpoint = url.to_owned();
    config.llm.timeout_ms = 30_000;
    config
}

const SHORT_DEADLINE_MS: u32 = 400;

const INPUT: &str = "manda o relatório na quinta";
const PT: Option<Language> = Some(Language::Pt);

#[tokio::test]
async fn formatted_text_comes_back_from_the_stream() {
    let server = fake(ollama_says("Manda o relatório na quinta.")).await;
    let out = Polisher::new(&config(&server.url))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!(out.llm, LlmOutcome::Used);
    assert_eq!(out.text, "Manda o relatório na quinta.");
    assert_eq!(
        server.paths(),
        ["GET /api/tags HTTP/1.1", "POST /api/chat HTTP/1.1"]
    );
    let body = server
        .requests
        .lock()
        .expect("lock")
        .last()
        .expect("request")
        .clone();
    assert!(
        body.contains("\"temperature\":0") && body.contains("\"keep_alive\":-1"),
        "{body}"
    );
}

/// Provider que começa a responder e para: o prazo (`timeout_ms`) corta,
/// o texto das regras sai e a conexão é fechada. Sem medir parede contra um
/// teto (instável sob carga): o servidor **nunca** termina a resposta, então
/// só o prazo do cliente devolve o ditado; o teto de 30 s só evita travar
/// a suíte se o prazo for ignorado. O piso é determinístico: o prazo não
/// dispara antes de `timeout_ms`.
#[tokio::test]
async fn slow_provider_hits_the_deadline_and_rules_text_is_used() {
    let hung_up = Arc::new(tokio::sync::Notify::new());
    let server = fake(Reply::Hang(Arc::clone(&hung_up))).await;
    let mut cfg = config(&server.url);
    cfg.llm.timeout_ms = SHORT_DEADLINE_MS;
    let started = Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        Polisher::new(&cfg).polish(INPUT, PT, false),
    )
    .await
    .expect("o prazo do LLM foi ignorado: o ditado esperou o provider");
    assert!(started.elapsed() >= Duration::from_millis(u64::from(cfg.llm.timeout_ms)));
    assert_eq!(out.llm, LlmOutcome::Timeout);
    assert_eq!(out.text, INPUT, "texto das regras");
    // O cliente abandonou o stream (não fica gerando em segundo plano).
    tokio::time::timeout(Duration::from_secs(30), hung_up.notified())
        .await
        .expect("conexão com o provider fechada depois do prazo");
    assert_eq!(server.count(CHAT), 1);
}

#[tokio::test]
async fn broken_stream_or_missing_model_falls_back_without_downloading() {
    let broken = fake(Reply::Ollama(vec![
        piece("<out>"),
        (0, "{isto não é json".into()),
    ]))
    .await;
    let out = Polisher::new(&config(&broken.url))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Unavailable, INPUT)
    );

    let missing = fake(Reply::Status(404, r#"{"error":"model 'x' not found"}"#)).await;
    let out = Polisher::new(&config(&missing.url))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Unavailable, INPUT)
    );
    assert_eq!(
        missing.paths(),
        ["GET /api/tags HTTP/1.1", "POST /api/chat HTTP/1.1"],
        "nenhum /api/pull"
    );
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let other = fake(ollama_says("Manda o relatório na quinta.")).await;
    let server = fake(Reply::Redirect(format!("{}/api/chat", other.url))).await;
    let out = Polisher::new(&config(&server.url))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!(out.llm, LlmOutcome::Unavailable);
    assert!(other.paths().is_empty(), "o texto não foi para outro host");
}

#[tokio::test]
async fn runaway_output_is_cut_and_answers_are_rejected() {
    let long = "palavra ".repeat(200);
    let runaway = fake(Reply::Ollama(vec![
        piece("<out>"),
        piece(&long),
        piece(&long),
        done(),
    ]))
    .await;
    let out = Polisher::new(&config(&runaway.url))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!((out.llm, out.text.as_str()), (LlmOutcome::Rejected, INPUT));

    let answer = fake(ollama_says("Paris.")).await;
    let out = Polisher::new(&config(&answer.url))
        .polish("qual a capital da França", PT, false)
        .await;
    assert_eq!(out.llm, LlmOutcome::Rejected);
    assert_eq!(out.text, "qual a capital da França");
}

#[tokio::test]
async fn when_the_llm_is_not_called() {
    let server = fake(ollama_says("Ok.")).await;
    let mut cfg = config(&server.url);
    cfg.snippets = vec![Snippet {
        trigger: "meu email".into(),
        expansion: "eu@x.com".into(),
    }];
    let polisher = Polisher::new(&cfg);

    // Limiar exclusivo (4): 3 palavras pulam, 4 chamam.
    assert_eq!(
        polisher.polish("manda o relatório", PT, false).await.llm,
        LlmOutcome::SkippedShort
    );
    let four = polisher.polish("manda o relatório amanhã", PT, false).await;
    assert_ne!(four.llm, LlmOutcome::SkippedShort);

    let snippet = polisher
        .polish("manda para o meu email por favor", PT, false)
        .await;
    assert_eq!(
        (snippet.llm, snippet.text.as_str()),
        (
            LlmOutcome::SkippedSnippet,
            "manda para o eu@x.com por favor"
        )
    );

    // Terminal: passa pelo LLM com regras de shell, mesmo curto; `false` = literal.
    assert_ne!(
        polisher.polish("git status", PT, true).await.llm,
        LlmOutcome::SkippedShort
    );
    cfg.llm.format_in_terminals = false;
    assert_eq!(
        Polisher::new(&cfg)
            .polish("Git status.", PT, true)
            .await
            .text,
        "Git status.",
        "literal não mexe"
    );
    cfg.llm.format_in_terminals = true;
    cfg.llm.enabled = false;
    assert_eq!(
        Polisher::new(&cfg).polish(INPUT, PT, false).await.llm,
        LlmOutcome::Disabled
    );
}

/// "Enviar agora" (opt-in): a frase sai antes do LLM, que nunca a vê. Fora do
/// terminal, ou com o opt-in desligado, é texto comum.
#[tokio::test]
async fn send_now_phrase_is_stripped_before_the_llm() {
    let server = fake(ollama_says("rode os testes do projeto")).await;
    let chats = |server: &Fake| -> Vec<String> {
        server
            .requests
            .lock()
            .expect("lock")
            .iter()
            .filter(|r| r.starts_with(CHAT))
            .cloned()
            .collect()
    };
    let mut cfg = config(&server.url);
    cfg.llm.timeout_ms = 5000;
    cfg.inject.send_now = true;
    let polisher = Polisher::new(&cfg);

    let out = polisher
        .polish("rode os testes do projeto, Enviar agora.", PT, true)
        .await;
    assert!(out.submit);
    assert_eq!(out.llm, LlmOutcome::Used);
    // Frase no terminal ganha maiúscula e ponto (`finish_terminal`).
    assert_eq!(out.text, "Rode os testes do projeto.");
    let sent = chats(&server);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("rode os testes do projeto"), "{}", sent[0]);
    for word in ["Enviar", "enviar", "agora"] {
        assert!(!sent[0].contains(word), "o LLM viu a frase: {}", sent[0]);
    }

    // Só a frase: nada a colar nem a enviar, e o LLM nem é chamado.
    let alone = polisher.polish("Enviar agora.", PT, true).await;
    assert_eq!((alone.text.as_str(), alone.submit), ("", false));
    assert_eq!(chats(&server).len(), 1);

    // Idioma fora de `asr.languages`: a frase dele é texto comum.
    let mut pt_only = cfg.clone();
    pt_only.asr.languages = vec![Language::Pt];
    let english = Polisher::new(&pt_only)
        .polish("git status send now", PT, true)
        .await;
    assert!(!english.submit);
    assert!(chats(&server).last().expect("chat").contains("send now"));

    // Fora do terminal e com o opt-in desligado: texto comum, vai ao LLM.
    let app = polisher
        .polish("rode os testes do projeto enviar agora", PT, false)
        .await;
    assert!(!app.submit);
    assert!(
        chats(&server)
            .last()
            .expect("chat")
            .contains("enviar agora")
    );
    cfg.inject.send_now = false;
    let off = Polisher::new(&cfg)
        .polish("rode os testes do projeto enviar agora", PT, true)
        .await;
    assert!(!off.submit);
    assert!(
        chats(&server)
            .last()
            .expect("chat")
            .contains("enviar agora")
    );

    // Literal (`format_in_terminals = false`): sem LLM, a frase sai igual.
    cfg.inject.send_now = true;
    cfg.llm.format_in_terminals = false;
    let literal = Polisher::new(&cfg)
        .polish("cargo test enviar agora", PT, true)
        .await;
    assert_eq!(
        (literal.text.as_str(), literal.submit, literal.llm),
        ("cargo test", true, LlmOutcome::SkippedLiteral)
    );
}

#[tokio::test]
async fn one_generation_at_a_time() {
    let slow = Reply::Ollama(vec![piece("<out>"), (1000, piece("x").1), done()]);
    let server = fake(slow).await;
    let mut cfg = config(&server.url);
    cfg.llm.timeout_ms = SHORT_DEADLINE_MS;
    let polisher = Arc::new(Polisher::new(&cfg));
    let (a, b) = tokio::join!(
        polisher.polish(INPUT, PT, false),
        polisher.polish(INPUT, PT, false)
    );
    let outcomes = [a.llm, b.llm];
    assert!(outcomes.contains(&LlmOutcome::Busy), "{outcomes:?}");
    assert_eq!(server.paths().len(), 2, "a segunda não abriu outra geração");
}

#[tokio::test]
async fn remote_endpoint_without_consent_keeps_dictation_local() {
    let mut cfg = Config::default();
    cfg.llm.endpoint = "https://api.openai.com".into();
    assert!(cfg.clone().llm.endpoint.starts_with("https"));
    let polisher = Polisher::new(&cfg);
    assert!(
        !polisher.llm_enabled(),
        "sem remote_consent o LLM remoto não é usado"
    );
    assert_eq!(
        polisher.polish(INPUT, PT, false).await.llm,
        LlmOutcome::Disabled
    );
    assert!(
        Config::parse(
            "[llm]\nendpoint = \"https://api.openai.com\"\n",
            std::path::Path::new("c")
        )
        .is_err()
    );
    assert!(
        Config::parse(
            "[llm]\nendpoint = \"https://api.openai.com\"\nremote_consent = true\n",
            std::path::Path::new("c")
        )
        .is_ok()
    );
}

#[tokio::test]
async fn openai_compatible_provider_with_key_file() {
    let body = serde_json::json!({"choices": [{"message": {"content": "<out>Manda o relatório na quinta.</out>"}}]}).to_string();
    let server = fake(Reply::Json(body)).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let key = dir.path().join("key");
    std::fs::write(&key, "sk-teste-123\n").expect("write");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let mut cfg = config(&server.url);
    cfg.llm.engine = LlmEngineKind::OpenaiCompatible;
    cfg.llm.api_key_file = key.display().to_string();
    let out = Polisher::new(&cfg).polish(INPUT, PT, false).await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Used, "Manda o relatório na quinta.")
    );
    let request = server
        .requests
        .lock()
        .expect("lock")
        .last()
        .expect("request")
        .clone();
    assert!(
        request.starts_with("POST /v1/chat/completions"),
        "{request}"
    );
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer sk-teste-123")
    );

    // Chave legível por outros: o provider não é usado.
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(!Polisher::new(&cfg).llm_enabled());
}

/// Log do `tracing` desta thread (os testes `tokio::test` rodam numa thread só).
struct Logs {
    buffer: Arc<Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("logs").extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture() -> Logs {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || LogWriter(Arc::clone(&writer)))
            .finish();
        Logs {
            buffer,
            _guard: tracing::subscriber::set_default(subscriber),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().expect("logs")).into_owned()
    }
}

fn sent_dictation(server: &Fake) -> bool {
    server
        .requests
        .lock()
        .expect("lock")
        .iter()
        .any(|r| r.contains(INPUT))
}

#[tokio::test]
async fn missing_or_cloud_model_falls_back_to_rules_without_prompt_or_download() {
    let cloud = serde_json::json!([{
        "name": fw_core::models::OLLAMA_MODEL,
        "digest": fw_core::models::OLLAMA_DIGEST,
        "remote_host": "https://ollama.com",
    }]);
    for models in [serde_json::json!([]), cloud] {
        let server = fake_with_inventory(ollama_says("Manda o relatório na quinta."), models).await;
        let polisher = Polisher::new(&config(&server.url));
        polisher.warmup().await;
        let out = polisher.polish(INPUT, PT, false).await;
        assert_eq!(
            (out.llm, out.text.as_str()),
            (LlmOutcome::Unavailable, INPUT)
        );
        // Warmup confere e não carrega; a geração reconfere (ainda sem modelo
        // usável) e não manda o texto. Nada de /api/pull.
        assert_eq!(server.paths(), [TAGS, TAGS]);
        assert!(!sent_dictation(&server));
    }
}

#[tokio::test]
async fn divergent_digest_warns_and_generation_continues() {
    for digest in [serde_json::json!("outro"), serde_json::Value::Null] {
        let models = serde_json::json!([{"name": fw_core::models::OLLAMA_MODEL, "digest": digest}]);
        let server = fake_with_inventory(ollama_says("Manda o relatório na quinta."), models).await;
        let logs = Logs::capture();
        let polisher = Polisher::new(&config(&server.url));
        polisher.warmup().await;
        let out = polisher.polish(INPUT, PT, false).await;
        assert_eq!(
            (out.llm, out.text.as_str()),
            (LlmOutcome::Used, "Manda o relatório na quinta.")
        );
        assert_eq!(server.paths(), [TAGS, GENERATE, CHAT]);
        let logs = logs.text();
        assert!(
            logs.contains("digest do modelo Ollama difere do fixado"),
            "{logs}"
        );
    }
}

#[tokio::test]
async fn non_default_model_is_accepted_and_used() {
    // Espaços nas pontas: o mesmo nome na conferência e no chat (senão cada
    // chat daria 404 e o ditado cairia nas regras depois do warmup ok).
    for configured in ["qwen3:8b", "  qwen3:8b "] {
        let models = serde_json::json!([{"name": "qwen3:8b", "digest": "abc123"}]);
        let server = fake_with_inventory(ollama_says("Manda o relatório na quinta."), models).await;
        let mut cfg = config(&server.url);
        cfg.llm.model = configured.into();
        let polisher = Polisher::new(&cfg);
        assert!(polisher.llm_enabled(), "qualquer modelo local é aceito");
        let logs = Logs::capture();
        polisher.warmup().await;
        assert!(logs.text().contains("fora do manifesto"), "{}", logs.text());
        for _ in 0..2 {
            let out = polisher.polish(INPUT, PT, false).await;
            assert_eq!(
                (out.llm, out.text.as_str()),
                (LlmOutcome::Used, "Manda o relatório na quinta."),
                "{configured:?}"
            );
        }
        assert_eq!(server.paths(), [TAGS, GENERATE, CHAT, CHAT]);
        let requests = server.requests.lock().expect("lock").clone();
        for request in &requests[1..] {
            assert!(request.contains(r#""model":"qwen3:8b""#), "{request}");
        }
    }
}

#[tokio::test]
async fn warmup_checks_inventory_and_never_pulls() {
    let server = fake(Reply::Json("{}".into())).await;
    Polisher::new(&config(&server.url)).warmup().await;
    assert_eq!(server.paths(), [TAGS, GENERATE]);
}

#[tokio::test]
async fn inventory_is_read_at_warmup_and_after_provider_errors_only() {
    let ok = ollama_says("Manda o relatório na quinta.");
    let server = fake(ok.clone()).await;
    let polisher = Polisher::new(&config(&server.url));
    polisher.warmup().await;
    for _ in 0..3 {
        assert_eq!(
            polisher.polish(INPUT, PT, false).await.llm,
            LlmOutcome::Used
        );
    }
    assert_eq!(server.paths(), [TAGS, GENERATE, CHAT, CHAT, CHAT]);

    // Inventário mudou sem erro do provider: não é relido a cada ditado.
    server.set_inventory(
        serde_json::json!([{"name": fw_core::models::OLLAMA_MODEL, "digest": "movido"}]),
    );
    assert_eq!(
        polisher.polish(INPUT, PT, false).await.llm,
        LlmOutcome::Used
    );
    assert_eq!(server.count(TAGS), 1);

    // Modelo sumiu: 404 na geração → regras; a próxima confere o inventário
    // e, sem modelo, nem manda o texto.
    server.set_reply(Reply::Status(404, r#"{"error":"model not found"}"#));
    server.set_inventory(serde_json::json!([]));
    assert_eq!(
        polisher.polish(INPUT, PT, false).await.llm,
        LlmOutcome::Unavailable
    );
    assert_eq!(server.count(TAGS), 1, "o erro só marca para reconferir");
    let chats = server.count(CHAT);
    let out = polisher.polish(INPUT, PT, false).await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Unavailable, INPUT)
    );
    assert_eq!(server.count(TAGS), 2);
    assert_eq!(server.count(CHAT), chats, "sem modelo, sem prompt");

    // Modelo de volta (o usuário instalou): confere uma vez e segue.
    server.set_inventory(pinned());
    server.set_reply(ok);
    for _ in 0..2 {
        assert_eq!(
            polisher.polish(INPUT, PT, false).await.llm,
            LlmOutcome::Used
        );
    }
    assert_eq!(server.count(TAGS), 3);
    assert_eq!(server.count(CHAT), chats + 2);
    assert!(server.paths().iter().all(|p| !p.contains("/api/pull")));
}

#[test]
fn default_guards_reject_cloud_or_remote_ollama_before_network() {
    let mut cfg = Config::default();
    assert_eq!(cfg.llm.model, fw_core::models::OLLAMA_MODEL);
    assert_eq!(cfg.llm.endpoint, "http://127.0.0.1:11434");
    assert!(!cfg.llm.remote_consent);
    cfg.llm.model = "qwen3:8b".into();
    assert!(Polisher::new(&cfg).llm_enabled(), "fora do pin é aceito");
    cfg.llm.model = "qwen3:4b-instruct-cloud".into();
    assert!(!Polisher::new(&cfg).llm_enabled());
    cfg.llm.model = fw_core::models::OLLAMA_MODEL.into();
    cfg.llm.endpoint = "https://ollama.com".into();
    cfg.llm.remote_consent = true;
    assert!(!Polisher::new(&cfg).llm_enabled());
}

#[tokio::test]
async fn ambient_proxies_are_ignored_in_a_separate_process() {
    let server = fake(ollama_says("Manda o relatório na quinta.")).await;
    let proxy = fake(Reply::Status(500, "proxy must not receive dictation")).await;
    let output = tokio::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "proxy_environment_child", "--ignored"])
        .env("FASTWISPER_PROXY_TEST_ENDPOINT", &server.url)
        .env("HTTP_PROXY", &proxy.url)
        .env("HTTPS_PROXY", &proxy.url)
        .env("ALL_PROXY", &proxy.url)
        .env("http_proxy", &proxy.url)
        .env("https_proxy", &proxy.url)
        .env("all_proxy", &proxy.url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .await
        .expect("child");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        proxy.paths().is_empty(),
        "nenhum texto ou inventário foi ao proxy"
    );
    assert_eq!(
        server.paths(),
        ["GET /api/tags HTTP/1.1", "POST /api/chat HTTP/1.1"]
    );
}

#[tokio::test]
#[ignore = "invocado pelo teste proxy com endpoint isolado explícito"]
async fn proxy_environment_child() {
    let endpoint =
        std::env::var("FASTWISPER_PROXY_TEST_ENDPOINT").expect("endpoint de teste obrigatório");
    // O assunto aqui é o proxy, não o prazo: num processo novo sob carga
    // (runtime e primeira conexão frios), o prazo curto de antes dava
    // `Timeout` em vez de `Used`; `config()` agora tem prazo longo.
    let out = Polisher::new(&config(&endpoint))
        .polish(INPUT, PT, false)
        .await;
    assert_eq!(out.llm, LlmOutcome::Used);
}

#[tokio::test]
#[ignore = "somente harness isolated-egress; nunca usar Ollama produção"]
async fn isolated_real_ollama() {
    let endpoint =
        std::env::var("FASTWISPER_ISOLATED_OLLAMA_ENDPOINT").expect("endpoint isolado obrigatório");
    let mut cfg = config(&endpoint);
    cfg.llm.timeout_ms = 30000; // captura funcional; não é gate de latência
    let polisher = Polisher::new(&cfg);
    polisher.warmup().await;
    let out = polisher.polish(INPUT, PT, false).await;
    assert_eq!(out.llm, LlmOutcome::Used);
    assert_eq!(out.text, "Manda o relatório na quinta.");
}

#[tokio::test]
async fn inventory_redirect_cannot_move_the_pin_or_prompt_to_another_host() {
    let other = fake(ollama_says("Manda o relatório na quinta.")).await;
    let server = fake(ollama_says("Manda o relatório na quinta.")).await;
    *server.inventory.lock().expect("inventory") =
        Reply::Redirect(format!("{}/api/tags", other.url));
    assert_eq!(
        Polisher::new(&config(&server.url))
            .polish(INPUT, PT, false)
            .await
            .llm,
        LlmOutcome::Unavailable
    );
    assert_eq!(server.paths(), ["GET /api/tags HTTP/1.1"]);
    assert!(other.paths().is_empty());
}

#[tokio::test]
#[ignore = "somente harness isolated-egress para store sem o modelo (divergente agora usa isolated_real_ollama)"]
async fn isolated_unavailable_ollama() {
    let endpoint =
        std::env::var("FASTWISPER_ISOLATED_OLLAMA_ENDPOINT").expect("endpoint isolado obrigatório");
    let polisher = Polisher::new(&config(&endpoint));
    polisher.warmup().await;
    let out = polisher.polish(INPUT, PT, false).await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Unavailable, INPUT)
    );
}

/// Corpo JSON de um pedido cru (depois do cabeçalho).
fn json_body(request: &str) -> serde_json::Value {
    let body = request.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {request}"))
}

fn chat_bodies(server: &Fake) -> Vec<serde_json::Value> {
    server
        .requests
        .lock()
        .expect("lock")
        .iter()
        .filter(|r| r.starts_with(CHAT))
        .map(|r| json_body(r))
        .collect()
}

/// Camada 1: sem estado entre ditados. Cada pedido ao provider é montado do
/// zero (sistema + exemplos fixos + a fala atual): nada de `context` do
/// `/api/generate`, histórico de mensagens nem texto de um ditado anterior,
/// mesmo depois de uma saída barrada (o modelo "respondeu" com código).
#[tokio::test]
async fn consecutive_dictations_share_nothing() {
    let server = fake(ollama_says(
        "```\nfn soma(a: i32, b: i32) -> i32 { a + b }\n```",
    ))
    .await;
    let polisher = Polisher::new(&config(&server.url));
    polisher.warmup().await;
    let first = "cria uma função que soma dois números inteiros";
    let out = polisher.polish(first, PT, true).await;
    assert_eq!(out.llm, LlmOutcome::Rejected);
    assert_eq!(out.text, "Cria uma função que soma dois números inteiros.");

    server.set_reply(ollama_says("Manda o relatório na quinta."));
    let out = polisher.polish(INPUT, PT, true).await;
    assert_eq!(
        (out.llm, out.text.as_str()),
        (LlmOutcome::Used, "Manda o relatório na quinta.")
    );

    let bodies = chat_bodies(&server);
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        let keys: Vec<&str> = body
            .as_object()
            .expect("objeto")
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["context", "prompt", "template", "raw", "suffix", "images"] {
            assert!(
                !keys.contains(&forbidden),
                "{forbidden} no pedido: {keys:?}"
            );
        }
    }
    let (a, b) = (
        bodies[0]["messages"].as_array().expect("mensagens"),
        bodies[1]["messages"].as_array().expect("mensagens"),
    );
    assert_eq!(a.len(), b.len(), "nenhuma mensagem acumulada");
    assert_eq!(a[..a.len() - 1], b[..b.len() - 1], "prefixo fixo e igual");
    let second = serde_json::to_string(&bodies[1]).expect("json");
    for word in ["função", "soma", "inteiros", "fn soma", "i32"] {
        assert!(!second.contains(word), "{word} vazou para o 2º pedido");
    }
    let last = b.last().expect("fala")["content"].as_str().expect("texto");
    assert!(last.ends_with(&format!("<in>{INPUT}</in>")), "{last}");
    // Warmup (e carga) do Ollama sem prompt: nada que crie `context`.
    for request in server.requests.lock().expect("lock").iter() {
        if request.starts_with(GENERATE) {
            let body = json_body(request);
            assert!(
                body.get("prompt").is_none() && body.get("context").is_none(),
                "{body}"
            );
        }
    }
}

/// Camada 3: comando × texto decidido no código, antes do LLM. Só fala
/// curta começando por um programa conhecido recebe as regras de terminal;
/// fala longa ou com cara de frase vai como texto comum (sem "Destino") e
/// sai com maiúscula e ponto. O daemon passa harness de IA pelo mesmo
/// caminho (`literal`), com a mesma regra.
#[tokio::test]
async fn only_short_command_like_speech_gets_terminal_rules() {
    let server = fake(ollama_says("x")).await;
    let polisher = Polisher::new(&config(&server.url)).with_executables(
        fw_core::terminal::Executables::from_names(["git", "cargo", "write", "example-app"]),
    );
    let destination = |server: &Fake| -> bool {
        chat_bodies(server)
            .last()
            .and_then(|b| b["messages"].as_array().cloned())
            .and_then(|m| m.last().cloned())
            .and_then(|m| m["content"].as_str().map(str::to_owned))
            .expect("pedido")
            .contains("Destino: comando de terminal")
    };

    server.set_reply(ollama_says("cargo build --release"));
    let out = polisher
        .polish("cargo build traço traço release", PT, true)
        .await;
    assert_eq!(out.text, "cargo build --release");
    assert!(destination(&server));
    let last = chat_bodies(&server);
    assert!(
        serde_json::to_string(&last)
            .expect("json")
            .contains("cargo build --release"),
        "\"traço traço\" vira opção antes do LLM, só em comando"
    );

    let long = "git é uma coisa que eu uso todo dia mas hoje eu fiz um rebase e perdi dois commits sem querer";
    server.set_reply(ollama_says(long));
    let out = polisher.polish(long, PT, true).await;
    assert!(!destination(&server), "fala longa não é comando");
    assert!(
        out.text.starts_with("Git é uma coisa") && out.text.ends_with('.'),
        "{}",
        out.text
    );

    let request = "write a function that adds two numbers";
    server.set_reply(ollama_says(request));
    let out = polisher.polish(request, Some(Language::En), true).await;
    assert!(
        !destination(&server),
        "programa que é palavra comum + frase"
    );
    assert_eq!(out.text, "Write a function that adds two numbers.");

    // Texto curto no terminal segue o limiar de qualquer app (sem LLM).
    let before = chat_bodies(&server).len();
    let out = polisher.polish("valeu demais", PT, true).await;
    assert_eq!(out.llm, LlmOutcome::SkippedShort);
    assert_eq!(chat_bodies(&server).len(), before);

    // Fora do terminal nunca há regras de comando.
    server.set_reply(ollama_says("git status"));
    polisher.polish("git status agora mesmo", PT, false).await;
    assert!(!destination(&server));
}

/// Camada 4: saída que não é a fala volta para as regras, e o log diz o
/// motivo só como código, sem o texto ditado nem a saída do modelo.
#[tokio::test]
async fn rejected_output_falls_back_and_logs_only_the_code() {
    let speech = "escreve um script em bash que apaga os arquivos temporários da pasta";
    let cases = [
        ("```bash\nrm -rf /tmp/*\n```", "code_fence"),
        (
            "Claro! Aqui está o script que apaga os arquivos.",
            "assistant_prefix",
        ),
        ("rm -rf ./tmp/* && echo done", "new_content"),
        ("bash", "too_short"),
    ];
    for (reply, code) in cases {
        let server = fake(ollama_says(reply)).await;
        let logs = Logs::capture();
        let out = Polisher::new(&config(&server.url))
            .polish(speech, PT, true)
            .await;
        assert_eq!(out.llm, LlmOutcome::Rejected, "{reply}");
        assert_eq!(
            out.text,
            "Escreve um script em bash que apaga os arquivos temporários da pasta."
        );
        let logs = logs.text();
        assert!(
            logs.contains(&format!("guard=\"{code}\"")),
            "{code}: {logs}"
        );
        for secret in ["apaga", "temporários", "rm -rf", "Claro"] {
            assert!(!logs.contains(secret), "{secret} no log: {logs}");
        }
    }
}

/// Fala longa demais para o contexto do modelo nem vai ao provider.
#[tokio::test]
async fn speech_longer_than_the_llm_context_uses_the_rules() {
    let server = fake(ollama_says("x")).await;
    let polisher = Polisher::new(&config(&server.url));
    let sentence = "a gente precisa revisar o plano de testes antes da entrega. ";
    let long = sentence.repeat(fw_core::llm::MAX_LLM_INPUT_CHARS / sentence.len() + 1);
    let out = polisher.polish(long.trim(), PT, false).await;
    assert_eq!(out.llm, LlmOutcome::SkippedLong);
    assert_eq!(out.text, long.trim());
    assert_eq!(server.count(CHAT), 0);
}
