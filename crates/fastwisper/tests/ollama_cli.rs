//! `fastwisper ollama` com config e runtime temporários e um Ollama falso no
//! loopback: o Ollama e o serviço do dono nunca são tocados (`--no-apply`).
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("HOME", root.join("home"))
        .env_remove("XDG_CONFIG_HOME")
        .args(args)
        .output()
        .expect("CLI")
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

/// Responde uma requisição com `body` e devolve a linha pedida.
fn fake_ollama(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let server = TcpListener::bind("127.0.0.1:0").expect("porta");
    let endpoint = format!("http://{}", server.local_addr().expect("endereço"));
    let handler = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("conexão");
        let mut request = [0u8; 2048];
        let n = stream.read(&mut request).expect("requisição");
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).expect("resposta");
        String::from_utf8_lossy(&request[..n])
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    });
    (endpoint, handler)
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporário");
    for dir in ["runtime", "home"] {
        std::fs::create_dir(root.path().join(dir)).expect("pasta");
    }
    std::fs::set_permissions(
        root.path().join("runtime"),
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("privado");
    root
}

#[test]
fn choice_is_saved_in_the_temporary_config_without_touching_ollama() {
    let root = fixture();
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        "# minha config\n[llm]\nmodel = \"qwen3:8b\" # meu\n",
    )
    .expect("config");
    let path = config.to_str().expect("utf-8");
    for (action, expected) in [("online", "true"), ("offline", "false")] {
        let out = stdout(cli(
            root.path(),
            &["ollama", action, "--no-apply", "--config", path],
        ));
        assert!(
            out.contains(&format!("llm.ollama_cloud = {expected}")),
            "{out}"
        );
        let text = std::fs::read_to_string(&config).expect("config");
        assert!(text.starts_with("# minha config\n[llm]\nmodel = \"qwen3:8b\" # meu\n"));
        assert!(
            text.contains(&format!("ollama_cloud = {expected}")),
            "{text}"
        );
    }
}

#[test]
fn status_and_models_come_from_the_local_ollama_as_json() {
    let root = fixture();
    let config = root.path().join("config.toml");
    let path = config.to_str().expect("utf-8");

    let (endpoint, server) = fake_ollama(r#"{"cloud":{"disabled":true,"source":"env"}}"#);
    std::fs::write(&config, format!("[llm]\nendpoint = \"{endpoint}\"\n")).expect("config");
    let out = stdout(cli(root.path(), &["ollama", "--json", "--config", path]));
    assert_eq!(server.join().expect("servidor"), "GET /api/status HTTP/1.1");
    let json: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(json["cloud"], false);
    assert_eq!(json["ollama"]["disabled"], true);

    let (endpoint, server) = fake_ollama(
        r#"{"models":[{"name":"qwen3:8b"},{"name":"gpt-oss:120b-cloud"},{"name":"qwen3:4b-instruct"}]}"#,
    );
    std::fs::write(&config, format!("[llm]\nendpoint = \"{endpoint}\"\n")).expect("config");
    let out = stdout(cli(
        root.path(),
        &["ollama", "models", "--json", "--config", path],
    ));
    assert_eq!(server.join().expect("servidor"), "GET /api/tags HTTP/1.1");
    let models: Vec<String> = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(
        models,
        ["qwen3:4b-instruct", "qwen3:8b"],
        "sem modelos de nuvem"
    );
}

#[test]
fn remote_provider_is_never_asked_for_models() {
    let root = fixture();
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        "[llm]\nengine = \"openai_compatible\"\nendpoint = \"https://api.example.com\"\nremote_consent = true\n",
    )
    .expect("config");
    let output = cli(
        root.path(),
        &[
            "ollama",
            "models",
            "--json",
            "--config",
            config.to_str().expect("utf-8"),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not \"ollama\""));
}

/// C-09: `apply` é o nome principal (ajuda em inglês nos dois idiomas) e
/// `aplicar` continua aceito como apelido. Só o Ollama falso responde.
#[test]
fn apply_is_the_english_name_and_aplicar_stays_an_alias() {
    let root = fixture();
    let user_config = root.path().join("home/.config/fastwisper/config.toml");
    std::fs::create_dir_all(user_config.parent().expect("pasta")).expect("pasta");
    for language in ["en", "pt-BR"] {
        std::fs::write(&user_config, format!("[ui]\nlanguage = \"{language}\"\n")).expect("config");
        for flag in ["--help", "-h"] {
            let help = stdout(cli(root.path(), &["ollama", flag]));
            for name in ["offline", "online", "apply", "models"] {
                assert!(help.contains(name), "{language} {flag}: {help}");
            }
            assert!(!help.contains("aplicar"), "{language} {flag}: {help}");
        }
    }

    let config = root.path().join("config.toml");
    let path = config.to_str().expect("utf-8");
    for action in ["apply", "aplicar"] {
        let (endpoint, server) = fake_ollama(r#"{"cloud":{"disabled":true,"source":"env"}}"#);
        std::fs::write(&config, format!("[llm]\nendpoint = \"{endpoint}\"\n")).expect("config");
        stdout(cli(
            root.path(),
            &["ollama", action, "--no-apply", "--config", path],
        ));
        assert_eq!(server.join().expect("servidor"), "GET /api/status HTTP/1.1");
    }
    std::fs::write(
        &config,
        "[llm]\nengine = \"openai_compatible\"\nendpoint = \"https://api.example.com\"\nremote_consent = true\n",
    )
    .expect("config");
    std::fs::write(&user_config, "[ui]\nlanguage = \"en\"\n").expect("config");
    for action in ["apply", "aplicar"] {
        let out = stdout(cli(root.path(), &["ollama", action, "--config", path]));
        assert!(out.contains("Nothing to apply"), "{out}");
    }
    let out =
        String::from_utf8_lossy(&cli(root.path(), &["ollama", "aplica", "--config", path]).stderr)
            .into_owned();
    assert!(out.contains("apply") && !out.contains("aplicar"), "{out}");
}
