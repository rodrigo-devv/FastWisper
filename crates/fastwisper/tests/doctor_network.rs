//! Exercise the actual doctor CLI with isolated config and hostile proxy settings.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn doctor_checks_inventory_warns_off_pin_and_ignores_redirects_and_proxies() {
    let root = std::env::temp_dir().join(format!("fw-doctor-contract-{}", std::process::id()));
    std::fs::create_dir(&root).expect("doctor contract fixture");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    for dir in ["home", "config", "data", "runtime", "bin"] {
        std::fs::create_dir(root.join(dir)).expect("doctor contract fixture");
    }
    for program in ["systemctl", "hyprctl", "wl-copy", "wl-paste", "notify-send"] {
        let path = root.join("bin").join(program);
        std::fs::write(&path, "#!/bin/sh\nexit 1\n").expect("doctor contract fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("doctor contract fixture");
    }
    let sink = TcpListener::bind("127.0.0.1:0").expect("doctor contract fixture");
    sink.set_nonblocking(true).expect("doctor contract fixture");
    let sink_url = format!(
        "http://{}",
        sink.local_addr().expect("doctor contract fixture")
    );
    let good = serde_json::json!({"models": [{
        "name": fw_core::models::OLLAMA_MODEL,
        "digest": fw_core::models::OLLAMA_DIGEST,
    }]})
    .to_string();
    let mismatch = serde_json::json!({"models": [{
        "name": fw_core::models::OLLAMA_MODEL, "digest": "wrong",
    }]})
    .to_string();
    let other = serde_json::json!({"models": [{"name": "qwen3:8b", "digest": "abc"}]}).to_string();
    let cloud = serde_json::json!({"models": [{
        "name": "qwen3:8b", "digest": "abc", "remote_host": "https://ollama.com",
    }]})
    .to_string();
    let pinned = fw_core::models::OLLAMA_MODEL;
    // Pin é verificação: fora dele é aviso e o ditado usa o modelo; ausente
    // cai nas regras. Só o padrão recomendado é sugerido.
    for (status, model, body, expected) in [
        (200, pinned, good.as_str(), "verified"),
        (201, pinned, good.as_str(), "verified"),
        (
            200,
            pinned,
            "{\"models\":[]}",
            "ollama pull qwen3:4b-instruct",
        ),
        (
            200,
            pinned,
            mismatch.as_str(),
            "digest wrong differing from the pinned digest",
        ),
        (
            200,
            "qwen3:8b",
            other.as_str(),
            "qwen3:8b is outside the manifest",
        ),
        (200, "qwen3:8b", cloud.as_str(), "cloud alias"),
        (307, pinned, good.as_str(), "Ollama did not respond"),
    ] {
        let server = TcpListener::bind("127.0.0.1:0").expect("doctor contract fixture");
        server
            .set_nonblocking(true)
            .expect("doctor contract fixture");
        let endpoint = format!(
            "http://{}",
            server.local_addr().expect("doctor contract fixture")
        );
        let config = root.join("doctor.toml");
        std::fs::write(
            &config,
            format!("[audio]\ndevice=\"\"\n[llm]\nendpoint=\"{endpoint}\"\nmodel=\"{model}\"\n"),
        )
        .expect("doctor contract fixture");
        let body = body.to_owned();
        let location = sink_url.clone();
        let handler = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let (mut stream, request) = next_request(&server, deadline);
            assert!(request.starts_with(b"GET /api/tags HTTP/1.1\r\n"));
            write!(stream, "HTTP/1.1 {status} Test\r\nLocation: {location}/redirect\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("doctor contract fixture");
            drop(stream);
            // Depois do inventário, o doctor confere a nuvem do Ollama (mesmo
            // endpoint local); nenhuma outra requisição é aceita.
            let (mut stream, request) = next_request(&server, deadline);
            assert!(request.starts_with(b"GET /api/status HTTP/1.1\r\n"));
            let cloud = r#"{"cloud":{"disabled":true,"source":"env"}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cloud}",
                cloud.len()
            )
            .expect("doctor contract fixture");
            // Keep the listener alive until the CLI exits to detect extra requests.
            server
        });
        let output = Command::new(env!("CARGO_BIN_EXE_fastwisper"))
            .args(["doctor", "--config"])
            .arg(config)
            .env_clear()
            .env("PATH", root.join("bin"))
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("HTTP_PROXY", &sink_url)
            .env("HTTPS_PROXY", &sink_url)
            .env("ALL_PROXY", &sink_url)
            .env("http_proxy", &sink_url)
            .env("https_proxy", &sink_url)
            .env("all_proxy", &sink_url)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .output()
            .expect("doctor contract fixture");
        let server = handler.join().expect("doctor contract fixture");
        let stdout = String::from_utf8(output.stdout).expect("doctor contract fixture");
        assert!(stdout.contains(expected), "status {status}: {stdout}");
        if expected != "verified" {
            assert!(
                !stdout.contains(&format!(
                    "digest {} verified",
                    fw_core::models::OLLAMA_DIGEST
                )),
                "status {status}: {stdout}"
            );
        }
        if !expected.starts_with("ollama pull") {
            assert!(!stdout.contains("ollama pull"), "status {status}: {stdout}");
        }
        if model != pinned {
            assert!(
                stdout.contains(&format!("recommended default: {pinned}"))
                    || stdout.contains(&format!("verified default: {pinned}")),
                "só o padrão é sugerido: {stdout}"
            );
        }
        assert_eq!(
            server
                .accept()
                .expect_err("unexpected network request")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            sink.accept()
                .expect_err("unexpected network request")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

/// Próxima conexão no listener não bloqueante e o cabeçalho do pedido.
fn next_request(
    server: &TcpListener,
    deadline: std::time::Instant,
) -> (std::net::TcpStream, Vec<u8>) {
    let mut stream = loop {
        match server.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(std::time::Instant::now() < deadline, "no doctor request");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    stream
        .set_nonblocking(false)
        .expect("doctor contract fixture");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("doctor contract fixture");
    let mut request = Vec::new();
    let mut byte = [0];
    while !request.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("doctor contract fixture");
        request.push(byte[0]);
        assert!(request.len() < 16384);
    }
    (stream, request)
}

/// Ollama que aceita a conexão e não responde (A-20): o `doctor` termina no
/// prazo total, com aviso de LLM sem resposta (nunca espera para sempre).
#[test]
fn doctor_finishes_when_the_llm_accepts_and_never_answers() {
    let root = tempfile::tempdir().expect("doctor contract fixture");
    let root = root.path();
    for dir in ["home", "config", "data", "runtime", "bin"] {
        std::fs::create_dir(root.join(dir)).expect("doctor contract fixture");
    }
    for program in ["systemctl", "hyprctl", "wl-copy", "notify-send"] {
        let path = root.join("bin").join(program);
        std::fs::write(&path, "#!/bin/sh\nexit 1\n").expect("doctor contract fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("doctor contract fixture");
    }
    let server = TcpListener::bind("127.0.0.1:0").expect("doctor contract fixture");
    let endpoint = format!(
        "http://{}",
        server.local_addr().expect("doctor contract fixture")
    );
    // Aceita e segura as conexões, sem responder nada.
    std::thread::spawn(move || {
        let mut open = Vec::new();
        while let Ok((stream, _)) = server.accept() {
            open.push(stream);
        }
    });
    let config = root.join("doctor.toml");
    std::fs::write(
        &config,
        format!("[audio]\ndevice=\"\"\n[llm]\nendpoint=\"{endpoint}\"\n"),
    )
    .expect("doctor contract fixture");
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .args(["doctor", "--config"])
        .arg(&config)
        .env_clear()
        .env("PATH", root.join("bin"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .output()
        .expect("doctor contract fixture");
    let took = started.elapsed();
    let stdout = String::from_utf8(output.stdout).expect("doctor contract fixture");
    let secs = fw_core::doctor::LLM_CHECK_TIMEOUT.as_secs();
    assert!(
        stdout.contains(&format!("no response from the LLM in {secs} s")),
        "{stdout}"
    );
    assert!(
        !stdout.contains("cloud"),
        "nuvem não é consultada: {stdout}"
    );
    // Prazo do LLM mais folga para o resto do doctor; nunca indefinido.
    assert!(
        took < fw_core::doctor::LLM_CHECK_TIMEOUT + std::time::Duration::from_secs(20),
        "{took:?}"
    );
}
