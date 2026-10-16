//! Contrato do bloqueio de rede do daemon:
//! com Landlock, o processo só conecta por TCP na porta do endpoint LLM
//! configurado; `bind` TCP é negado e sockets Unix seguem livres.
//!
//! Landlock não tem volta, então cada cenário roda num processo filho (este
//! mesmo binário, filtrado no teste `child`) que aplica `restrict_network`
//! e tenta conectar nos servidores falsos que o pai abriu em 127.0.0.1.
//! Kernel sem regras de rede (ABI < 4) → cenário pulado com aviso, a menos
//! que `FASTWISPER_REQUIRE_LANDLOCK=1` exija o bloqueio.

use std::collections::HashMap;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;

use fw_core::config::LlmConfig;
use fw_core::sandbox::{NetworkSandbox, restrict_network};

const CHILD_LLM: &str = "FW_SANDBOX_LLM";
const CHILD_PROBES: &str = "FW_SANDBOX_PROBES";
const MARK: &str = "FWSANDBOX";

/// Lado do filho. Sem as variáveis, não faz nada (roda junto com os demais).
#[test]
fn child() {
    let Ok(llm) = std::env::var(CHILD_LLM) else {
        return;
    };
    let config: LlmConfig = toml::from_str(&llm).expect("[llm] do pai");
    let probes: Vec<u16> = std::env::var(CHILD_PROBES)
        .expect("portas do pai")
        .split(',')
        .map(|p| p.parse().expect("porta"))
        .collect();

    let sandbox = restrict_network(&config);
    println!(
        "{MARK} sandbox {}",
        serde_json::to_string(&sandbox).expect("json")
    );
    for port in &probes {
        let outcome = outcome(TcpStream::connect(("127.0.0.1", *port)));
        println!("{MARK} connect:{port} {outcome}");
    }
    // Thread criada depois herda a restrição.
    let port = probes[0];
    let late = std::thread::spawn(move || outcome(TcpStream::connect(("127.0.0.1", port))))
        .join()
        .expect("thread");
    println!("{MARK} thread:{port} {late}");
    println!("{MARK} bind {}", outcome(TcpListener::bind("127.0.0.1:0")));
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.sock");
    let unix = UnixListener::bind(&path).and_then(|_listener| UnixStream::connect(&path));
    println!("{MARK} unix {}", outcome(unix));
}

fn outcome<T>(result: io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::ACCESS.raw_os_error()) => {
            "eacces".into()
        }
        Err(e) => format!("erro:{e}"),
    }
}

/// Resultado de um cenário no filho.
struct Report {
    sandbox: NetworkSandbox,
    lines: HashMap<String, String>,
}

impl Report {
    fn get(&self, key: &str) -> &str {
        self.lines
            .get(key)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("filho não reportou {key}: {:?}", self.lines))
    }
}

/// Roda o cenário num filho. `None` = kernel sem suporte (pulado).
fn run_child(scenario: &str, llm: &str, probes: &[u16]) -> Option<Report> {
    let probes: Vec<String> = probes.iter().map(u16::to_string).collect();
    let output = Command::new(std::env::current_exe().expect("binário de teste"))
        .args(["child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_LLM, llm)
        .env(CHILD_PROBES, probes.join(","))
        .output()
        .expect("inicia o filho");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{scenario}: filho falhou\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut sandbox = None;
    let mut lines = HashMap::new();
    for line in stdout.lines() {
        // O libtest imprime "test child ... " antes, na mesma linha.
        let Some(rest) = line.find(MARK).map(|at| line[at + MARK.len()..].trim()) else {
            continue;
        };
        let (key, value) = rest.split_once(' ').expect("chave valor");
        if key == "sandbox" {
            sandbox = Some(serde_json::from_str::<NetworkSandbox>(value).expect("estado"));
        } else {
            lines.insert(key.to_owned(), value.to_owned());
        }
    }
    let sandbox = sandbox.unwrap_or_else(|| panic!("{scenario}: filho sem estado\n{stdout}"));
    if let NetworkSandbox::Unavailable { .. } = sandbox {
        assert!(
            std::env::var_os("FASTWISPER_REQUIRE_LANDLOCK").is_none(),
            "{scenario}: FASTWISPER_REQUIRE_LANDLOCK definido, mas {sandbox}"
        );
        eprintln!("PULADO {scenario}: {sandbox} (regras de rede exigem Linux 6.7+)");
        return None;
    }
    Some(Report { sandbox, lines })
}

/// Dois servidores TCP falsos em 127.0.0.1 (a conexão completa no backlog).
fn two_servers() -> (TcpListener, TcpListener) {
    let a = TcpListener::bind("127.0.0.1:0").expect("servidor A");
    let b = TcpListener::bind("127.0.0.1:0").expect("servidor B");
    (a, b)
}

fn port(listener: &TcpListener) -> u16 {
    listener.local_addr().expect("endereço").port()
}

#[test]
fn local_endpoint_port_is_the_only_tcp_allowed() {
    let (allowed, other) = two_servers();
    let (a, b) = (port(&allowed), port(&other));
    let llm = format!("endpoint = \"http://127.0.0.1:{a}\"");
    let Some(report) = run_child("local", &llm, &[a, b]) else {
        return;
    };
    assert!(
        matches!(&report.sandbox, NetworkSandbox::Enforced { connect_ports, .. } if connect_ports == &[a]),
        "{}",
        report.sandbox
    );
    assert_eq!(report.get(&format!("connect:{a}")), "ok");
    assert_eq!(report.get(&format!("connect:{b}")), "eacces");
    assert_eq!(report.get(&format!("thread:{a}")), "ok");
    assert_eq!(report.get("bind"), "eacces");
    assert_eq!(report.get("unix"), "ok", "sockets Unix não são afetados");
}

#[test]
fn remote_port_is_denied_without_consent() {
    let (remote, local) = two_servers();
    let (r, l) = (port(&remote), port(&local));
    // O host não é resolvido: a regra do Landlock é por porta, então o
    // servidor falso em 127.0.0.1 nessa porta faz o papel do provider.
    let llm = format!(
        "engine = \"openai_compatible\"\nendpoint = \"https://llm.example.com:{r}\"\nremote_consent = false"
    );
    let Some(report) = run_child("remoto sem consentimento", &llm, &[r, l]) else {
        return;
    };
    assert!(
        matches!(&report.sandbox, NetworkSandbox::Enforced { connect_ports, .. } if connect_ports.is_empty()),
        "{}",
        report.sandbox
    );
    assert_eq!(report.get(&format!("connect:{r}")), "eacces");
    assert_eq!(report.get(&format!("connect:{l}")), "eacces");
    assert_eq!(report.get(&format!("thread:{r}")), "eacces");
    assert_eq!(report.get("bind"), "eacces");
}

#[test]
fn remote_port_is_allowed_only_with_consent() {
    let (remote, local) = two_servers();
    let (r, l) = (port(&remote), port(&local));
    let llm = format!(
        "engine = \"openai_compatible\"\nendpoint = \"https://llm.example.com:{r}\"\nremote_consent = true"
    );
    let Some(report) = run_child("remoto com consentimento", &llm, &[r, l]) else {
        return;
    };
    assert_eq!(report.get(&format!("connect:{r}")), "ok");
    assert_eq!(report.get(&format!("connect:{l}")), "eacces");
    assert_eq!(report.get("bind"), "eacces");
}

#[test]
fn disabled_llm_allows_no_tcp() {
    let (one, two) = two_servers();
    let (a, b) = (port(&one), port(&two));
    let llm = format!("enabled = false\nendpoint = \"http://127.0.0.1:{a}\"");
    let Some(report) = run_child("LLM desligado", &llm, &[a, b]) else {
        return;
    };
    assert_eq!(report.get(&format!("connect:{a}")), "eacces");
    assert_eq!(report.get(&format!("connect:{b}")), "eacces");
    assert_eq!(report.get("unix"), "ok");
}
