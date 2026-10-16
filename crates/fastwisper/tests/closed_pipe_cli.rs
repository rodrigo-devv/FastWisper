//! Saída num pipe que quem lê fecha cedo (`fastwisper ... | head -1`): nenhum
//! comando entra em pânico nem mostra stack trace, e o pipe fechado é fim
//! normal (código 0). A árvore de comandos vem da ajuda gerada pelo clap;
//! comando novo sem entrada em [`plan`] reprova o teste.
//!
//! Cada caso roda sem daemon e contra um daemon de teste (pipeline nulo, sem
//! desktop) num runtime `0700` temporário, com HOME/XDG descartáveis, sem
//! sessão gráfica e com `hyprctl`/`systemctl`/`pkexec`/`sudo` falsos no PATH:
//! nada alcança a sessão de quem roda o teste.
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fw_core::config::Config;
use fw_core::daemon::{Daemon, DaemonPaths};
use fw_core::inject::NoDesktop;
use fw_core::ipc::Request;
use fw_core::pipeline::NullPipeline;
use fw_core::session::{Command as SessionCommand, StateName};

const BIN: &str = env!("CARGO_BIN_EXE_fastwisper");
/// Prazo de um caso; um `--follow` que não percebe o leitor saindo estoura.
const DEADLINE: Duration = Duration::from_secs(30);

/// Como um caso roda.
#[derive(Clone, Copy)]
struct Case {
    args: &'static [&'static str],
    /// Segue imprimindo até quem lê sair (`--follow`, medidor).
    streams: bool,
    /// Com daemon, precisa de uma gravação em andamento (medidor).
    recording: bool,
}

const fn once(args: &'static [&'static str]) -> Case {
    Case {
        args,
        streams: false,
        recording: false,
    }
}
const fn streams(args: &'static [&'static str]) -> Case {
    Case {
        args,
        streams: true,
        recording: false,
    }
}
const fn while_recording(args: &'static [&'static str]) -> Case {
    Case {
        args,
        streams: true,
        recording: true,
    }
}

enum Plan {
    /// Roda estes casos com a saída fechada cedo.
    Run(Vec<Case>),
    /// Só agrupa subcomandos (a ajuda é conferida igual).
    Group,
    /// Efeito colateral fora da sandbox; só a ajuda é conferida.
    HelpOnly(&'static str),
}

/// Todo comando da árvore do clap precisa estar aqui.
fn plan(path: &str) -> Option<Plan> {
    use Plan::{Group, HelpOnly, Run};
    Some(match path {
        "hub" => HelpOnly("abre uma janela"),
        "ctl" => Run(vec![
            once(&["ctl", "status"]),
            once(&["ctl", "status", "--json"]),
            once(&["ctl", "show-last"]),
        ]),
        "models" | "config" | "hotkeys" | "audio" | "notes" => Group,
        "models llm-manifest" => Run(vec![once(&["models", "llm-manifest"])]),
        "models pull-llm" => HelpOnly("baixa da rede"),
        "models list" => Run(vec![once(&["models", "list"])]),
        "models pull" => HelpOnly("baixa da rede"),
        "models use" => Run(vec![once(&[
            "models",
            "use",
            fw_core::models::DEFAULT_MODEL_ID,
        ])]),
        "models verify" => Run(vec![once(&["models", "verify"])]),
        "transcribe" | "bench" => HelpOnly("precisa de WAV e modelo de transcrição"),
        "doctor" => Run(vec![once(&["doctor"])]),
        "gpus" => Run(vec![once(&["gpus"]), once(&["gpus", "--json"])]),
        "history" => Run(vec![once(&["history"])]),
        "history clear" => Run(vec![once(&["history", "clear", "--yes"])]),
        "notes list" => Run(vec![once(&["notes", "list"])]),
        "notes clear" => Run(vec![once(&["notes", "clear", "--yes"])]),
        "status" => Run(vec![
            once(&["status"]),
            once(&["status", "--bar"]),
            once(&["status", "--bar", "--localized"]),
            streams(&["status", "--follow"]),
            streams(&["status", "--follow", "--bar"]),
            streams(&["status", "--follow", "--bar", "--localized"]),
        ]),
        // Sem daemon, `--follow` zera a pílula e espera; sem `--follow`, erro.
        "meter" => Run(vec![
            while_recording(&["meter"]),
            while_recording(&["meter", "--follow"]),
        ]),
        "vram" => Run(vec![once(&["vram"])]),
        "config list" => Run(vec![
            once(&["config", "list"]),
            once(&["config", "list", "--json"]),
        ]),
        "config get" => Run(vec![
            once(&["config", "get", "audio.sounds"]),
            once(&["config", "get", "audio.sounds", "--json"]),
        ]),
        "config set" => Run(vec![once(&["config", "set", "audio.sounds", "false"])]),
        "config reload" => Run(vec![once(&["config", "reload"])]),
        "hotkeys list" => Run(vec![
            once(&["hotkeys", "list"]),
            once(&["hotkeys", "list", "--json"]),
        ]),
        "hotkeys check" => Run(vec![
            once(&["hotkeys", "check", "dictate", "SUPER+F12"]),
            once(&["hotkeys", "check", "dictate", "SUPER+F12", "--json"]),
        ]),
        "hotkeys apply" => Run(vec![once(&["hotkeys", "apply"])]),
        "hotkeys keys" => Run(vec![once(&["hotkeys", "keys"])]),
        // `offline`/`online`/`aplicar` mexem no serviço do Ollama: nunca aqui.
        "ollama" => Run(vec![
            once(&["ollama"]),
            once(&["ollama", "--json"]),
            once(&["ollama", "models"]),
        ]),
        "restart" => HelpOnly("reinicia o serviço do usuário"),
        "setup" => Run(vec![once(&["setup", "--dry-run"])]),
        "audio sources" => Run(vec![
            once(&["audio", "sources"]),
            once(&["audio", "sources", "--json"]),
        ]),
        "audio test-sound" => HelpOnly("toca som"),
        // Grava pelo daemon (de teste) e precisa cancelar mesmo com o pipe fechado.
        "audio test" => Run(vec![
            once(&["audio", "test", "--secs", "1"]),
            once(&["audio", "test", "--secs", "1", "--json"]),
        ]),
        _ => return None,
    })
}

/// HOME, XDG, PATH e runtime descartáveis.
struct Sandbox {
    root: PathBuf,
    path: std::ffi::OsString,
}

impl Sandbox {
    fn new(root: &Path) -> Self {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        for tool in [
            "hyprctl",
            "systemctl",
            "pkexec",
            "sudo",
            "notify-send",
            "wl-copy",
            "xdg-open",
        ] {
            let file = bin.join(tool);
            fs::write(&file, "#!/bin/sh\nexit 1\n").expect("falso");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let config = root.join("xdg-config/fastwisper");
        fs::create_dir_all(&config).expect("config");
        // LLM numa porta de loopback sem ninguém: nada fala com um Ollama real.
        fs::write(
            config.join("config.toml"),
            "[llm]\nendpoint = \"http://127.0.0.1:9\"\n\n[audio]\nsounds = false\n",
        )
        .expect("config");
        let mut path = std::ffi::OsString::from(&bin);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        Self {
            root: root.to_owned(),
            path,
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("xdg-config/fastwisper/config.toml")
    }

    fn command(&self, runtime: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("xdg-config"))
            .env("XDG_DATA_HOME", self.root.join("xdg-data"))
            .env("XDG_STATE_HOME", self.root.join("xdg-state"))
            .env("XDG_CACHE_HOME", self.root.join("xdg-cache"))
            .env("XDG_RUNTIME_DIR", runtime)
            .env("PATH", &self.path)
            .env("RUST_BACKTRACE", "1")
            .env_remove("FASTWISPER_LOG")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("PIPEWIRE_REMOTE")
            .env_remove("PIPEWIRE_RUNTIME_DIR")
            .stdin(Stdio::null());
        command
    }
}

fn private_dir(path: &Path) -> PathBuf {
    fs::create_dir_all(path).expect("dir");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("0700");
    path.to_owned()
}

/// Espera o processo até [`DEADLINE`]; estourou, mata e reprova.
fn wait(child: &mut Child, what: &str) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what}: não terminou em {DEADLINE:?} com a saída fechada");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Execução normal: houve saída? Com que código terminou (`None`: segue
/// imprimindo, foi interrompido)?
struct Baseline {
    printed: bool,
    code: Option<i32>,
}

fn baseline(sandbox: &Sandbox, runtime: &Path, case: Case, what: &str) -> Baseline {
    let out = sandbox.root.join("baseline.out");
    let mut child = sandbox
        .command(runtime, case.args)
        .stdout(File::create(&out).expect("saída"))
        .stderr(Stdio::null())
        .spawn()
        .expect("CLI");
    let start = Instant::now();
    loop {
        let printed = fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false);
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Baseline {
                printed: fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false),
                code: status.code(),
            };
        }
        if case.streams && printed {
            let _ = child.kill();
            let _ = child.wait();
            return Baseline {
                printed: true,
                code: None,
            };
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what}: a execução normal não terminou nem imprimiu");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Roda com quem lê fechando depois de `keep` bytes (0: antes de tudo).
fn closed_early(
    sandbox: &Sandbox,
    runtime: &Path,
    case: Case,
    keep: usize,
    what: &str,
) -> (ExitStatus, String) {
    let err = sandbox.root.join("closed.err");
    let mut child = sandbox
        .command(runtime, case.args)
        .stdout(Stdio::piped())
        .stderr(File::create(&err).expect("stderr"))
        .spawn()
        .expect("CLI");
    let mut stdout = child.stdout.take().expect("pipe");
    if keep > 0 {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = vec![0u8; keep];
            let _ = stdout.read_exact(&mut buf);
            let _ = tx.send(());
            // `stdout` sai de escopo aqui: o pipe fecha.
        });
        if rx.recv_timeout(DEADLINE).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what}: nada impresso em {DEADLINE:?}");
        }
    } else {
        drop(stdout);
    }
    let status = wait(&mut child, what);
    (status, fs::read_to_string(&err).expect("stderr"))
}

fn assert_clean(what: &str, stderr: &str) {
    for sign in [
        "panicked",
        "stack backtrace",
        "RUST_BACKTRACE",
        "Broken pipe",
        "os error 32",
    ] {
        assert!(
            !stderr.contains(sign),
            "{what}: stderr com {sign:?}:\n{stderr}"
        );
    }
}

fn daemon_state(runtime: &Path) -> Option<StateName> {
    let socket = DaemonPaths::in_dir(runtime).socket;
    fw_core::client::send(&socket, &Request::new("pipe-test", SessionCommand::Status))
        .expect("daemon de teste")
        .state
}

fn session(runtime: &Path, command: SessionCommand) {
    let socket = DaemonPaths::in_dir(runtime).socket;
    let response = fw_core::client::send(&socket, &Request::new("pipe-test", command))
        .expect("daemon de teste");
    assert!(
        response.error.is_none(),
        "{command:?}: {:?}",
        response.error
    );
}

/// Árvore de comandos lida da ajuda do clap (`Commands:`), sem `help`.
fn command_tree(sandbox: &Sandbox, runtime: &Path) -> Vec<Vec<String>> {
    let mut tree = vec![Vec::new()];
    let mut index = 0;
    while index < tree.len() {
        let path = tree[index].clone();
        index += 1;
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let output = sandbox.command(runtime, &args).output().expect("ajuda");
        assert!(output.status.success(), "{args:?}");
        let help = String::from_utf8(output.stdout).expect("UTF-8");
        let mut in_commands = false;
        for line in help.lines() {
            if line == "Commands:" {
                in_commands = true;
                continue;
            }
            if !in_commands {
                continue;
            }
            let Some(name) = line
                .strip_prefix("  ")
                .and_then(|l| l.split_whitespace().next())
            else {
                break;
            };
            if name != "help" {
                let mut child = path.clone();
                child.push(name.to_owned());
                tree.push(child);
            }
        }
    }
    tree
}

fn run_case(sandbox: &Sandbox, runtime: &Path, online: bool, case: Case) {
    let what = format!(
        "fastwisper {} ({})",
        case.args.join(" "),
        if online { "com daemon" } else { "sem daemon" }
    );
    let recording = online && case.recording;
    let start_recording = || {
        if recording {
            session(runtime, SessionCommand::Press);
        }
    };
    let stop_recording = || {
        if recording {
            session(runtime, SessionCommand::Cancel);
        }
        if online {
            // Nada fica gravando depois do comando (ex.: `audio test`).
            assert_eq!(daemon_state(runtime), Some(StateName::Idle), "{what}");
        }
    };

    start_recording();
    let normal = baseline(sandbox, runtime, case, &what);
    stop_recording();
    if !normal.printed {
        // Nada a fechar (ex.: daemon ausente): só o código de sempre.
        assert!(
            normal.code.is_some(),
            "{what}: um caso que segue imprimindo precisa imprimir"
        );
        assert!(
            !(online && case.recording),
            "{what}: nada impresso durante a gravação"
        );
    }

    for keep in [0, 1] {
        if keep > 0 && !normal.printed {
            continue;
        }
        start_recording();
        let (status, stderr) = closed_early(sandbox, runtime, case, keep, &what);
        stop_recording();
        let what = format!("{what}, leitor fecha após {keep} byte(s)");
        assert_clean(&what, &stderr);
        let code = status.code();
        let expected = if !normal.printed {
            vec![normal.code]
        } else if keep == 0 {
            vec![Some(0)]
        } else {
            // Tudo coube no pipe antes de o leitor fechar: código de sempre.
            vec![Some(0), normal.code]
        };
        assert!(
            expected.contains(&code),
            "{what}: código {status:?}, esperado {expected:?}\n{stderr}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_command_ends_cleanly_when_the_reader_closes_the_pipe() {
    let root = tempfile::tempdir().expect("temporário");
    let root = private_dir(root.path());
    let sandbox = Sandbox::new(&root);
    let offline = private_dir(&root.join("run-offline"));
    let online = private_dir(&root.join("run-online"));

    let config = sandbox.config();
    let boot = Config::load(Some(&config)).expect("config válida");
    let daemon = Daemon::bind(DaemonPaths::in_dir(&online))
        .expect("daemon")
        .with_config(&config, true, boot);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(
        daemon.run(Arc::new(NullPipeline), Arc::new(NoDesktop), async {
            let _ = stopped.await;
        }),
    );

    let checked = tokio::task::spawn_blocking(move || {
        let tree = command_tree(&sandbox, &offline);
        let mut unclassified = Vec::new();
        let mut cases = Vec::new();
        for path in &tree {
            // A ajuda de todo comando, com a saída fechada.
            let mut args: Vec<&'static str> = path
                .iter()
                .map(|s| &*Box::leak(s.clone().into_boxed_str()))
                .collect();
            args.push("--help");
            let help = once(Box::leak(args.into_boxed_slice()));
            let (status, stderr) = closed_early(&sandbox, &offline, help, 0, "--help");
            assert_clean(&format!("{:?}", help.args), &stderr);
            assert_eq!(status.code(), Some(0), "{:?}\n{stderr}", help.args);

            if path.is_empty() {
                continue;
            }
            match plan(&path.join(" ")) {
                None => unclassified.push(path.join(" ")),
                Some(Plan::Run(run)) => cases.extend(run),
                Some(Plan::Group) => {}
                Some(Plan::HelpOnly(reason)) => {
                    println!("só a ajuda: {} ({reason})", path.join(" "));
                }
            }
        }
        assert!(
            unclassified.is_empty(),
            "comandos sem entrada em plan(): {unclassified:?}"
        );
        assert!(tree.len() > 20, "árvore do clap incompleta: {tree:?}");

        let (status, stderr) =
            closed_early(&sandbox, &offline, once(&["--version"]), 0, "--version");
        assert_clean("--version", &stderr);
        assert_eq!(status.code(), Some(0));

        for case in &cases {
            run_case(&sandbox, &offline, false, *case);
            run_case(&sandbox, &online, true, *case);
        }
        cases.len()
    })
    .await
    .expect("casos");
    assert!(checked > 30, "{checked} casos");

    let _ = stop.send(());
    worker.await.expect("daemon");
}

/// Outro erro de escrita (disco cheio) não é pipe fechado: falha com
/// mensagem curta, sem pânico.
#[test]
fn a_full_disk_on_stdout_fails_without_panicking() {
    let root = tempfile::tempdir().expect("temporário");
    let root = private_dir(root.path());
    let sandbox = Sandbox::new(&root);
    let runtime = private_dir(&root.join("run"));
    let output = sandbox
        .command(&runtime, &["hotkeys", "keys"])
        .stdout(File::create("/dev/full").expect("/dev/full"))
        .output()
        .expect("CLI");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("fastwisper: could not write output: "),
        "{stderr}"
    );
}
