//! Driver Vulkan instalado mas quebrado (ICD falso que trava ou aborta) não
//! derruba o fastwisperd: a sondagem roda num filho, falha, e o daemon se
//! reexecuta sem Vulkan e segue na CPU, com aviso no log e na notificação.
//! Só no build com Vulkan (o build da CI é só CPU e nunca abre o Vulkan).
#![cfg(feature = "vulkan")]

mod fake_icd;

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fake_icd::{FakeIcd, Mode};
use fw_core::ipc::Request;
use fw_core::session::Command as SessionCommand;
use fw_core::vram::VramMode;

const NOTICE_EN: &str =
    "Vulkan driver failed: transcription uses the CPU (slower). See `fastwisper doctor`.";
const PROBE_LOG: &str = "gpu: sondagem do Vulkan";

fn daemon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fastwisperd"))
}

/// O driver falso de fato derruba (ou prende) quem abre o Vulkan: o filho da
/// sondagem morre por SIGABRT ou fica preso, sem imprimir o relatório.
#[test]
fn the_fake_driver_breaks_the_probe_child() {
    let icd = FakeIcd::build(Mode::Abort);
    let mut command = daemon();
    command.arg(fw_core::gpu_probe::CHILD_ARG);
    icd.apply(&mut command);
    let output = command.output().expect("fastwisperd --probe-gpu");
    assert_eq!(output.status.signal(), Some(6), "{output:?}");
    assert!(output.stdout.is_empty());

    let icd = FakeIcd::build(Mode::Hang);
    let mut command = daemon();
    command
        .arg(fw_core::gpu_probe::CHILD_ARG)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    icd.apply(&mut command);
    let mut child = command.spawn().expect("fastwisperd --probe-gpu");
    std::thread::sleep(Duration::from_secs(1));
    assert!(child.try_wait().expect("estado do filho").is_none());
    child.kill().expect("matar o filho preso");
    let _ = child.wait();
}

/// Quem sondou morreu (SIGTERM no meio da sondagem, CLI morta pelo hub): o
/// filho preso no driver morre junto, sem ficar órfão.
#[test]
fn a_hung_probe_child_dies_with_its_parent() {
    let icd = FakeIcd::build(Mode::Hang);
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "\"$0\" --probe-gpu >/dev/null 2>&1 & echo $!; sleep 1",
            env!("CARGO_BIN_EXE_fastwisperd"),
        ])
        .stderr(Stdio::null());
    icd.apply(&mut command);
    let output = command.output().expect("pai da sondagem");
    let pid: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("PID do filho");
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        if Instant::now() > deadline {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            panic!("o filho da sondagem ficou órfão e preso no driver");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Processo existe e não é zumbi.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let state = stat.rsplit_once(") ")?.1.chars().next()?;
            Some(state != 'Z' && state != 'X')
        })
        .unwrap_or(false)
}

/// Filhos da sondagem zumbis de `pid` (um filho morto no prazo e não
/// recolhido antes do `exec` ficaria assim enquanto o daemon vivesse).
fn zombie_probe_children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|c| c.parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|&child| {
            std::fs::read_to_string(format!("/proc/{child}/stat")).is_ok_and(|stat| {
                stat.contains("(fastwisperd)")
                    && stat
                        .rsplit_once(") ")
                        .is_some_and(|(_, s)| s.starts_with('Z'))
            })
        })
        .collect()
}

/// Daemon isolado (HOME/XDG/runtime temporários, `notify-send` falso que
/// grava as mensagens). Conecta ao PipeWire real só para a captura subir.
struct TestDaemon {
    dir: tempfile::TempDir,
    child: Child,
}

impl TestDaemon {
    fn start(icd: &FakeIcd, gpu: &str) -> TestDaemon {
        let dir = tempfile::tempdir().expect("diretório isolado");
        let path = |sub: &str| dir.path().join(sub);
        for sub in ["home", "config", "data", "state", "bin", "run"] {
            std::fs::create_dir_all(path(sub)).expect("diretório isolado");
        }
        std::fs::set_permissions(path("run"), std::fs::Permissions::from_mode(0o700))
            .expect("runtime 0700");
        let notify = path("bin/notify-send");
        std::fs::write(
            &notify,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                path("notifications").display()
            ),
        )
        .expect("notify-send falso");
        std::fs::set_permissions(&notify, std::fs::Permissions::from_mode(0o700))
            .expect("notify-send falso");
        let config = path("config.toml");
        std::fs::write(
            &config,
            format!(
                "[asr]\ngpu = \"{gpu}\"\n\n[audio]\nsounds = false\n\n[llm]\nenabled = false\n"
            ),
        )
        .expect("config isolada");
        let pipewire = std::env::var_os("PIPEWIRE_RUNTIME_DIR")
            .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
            .expect("PipeWire do usuário (XDG_RUNTIME_DIR)");
        let system_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![path("bin")];
        paths.extend(std::env::split_paths(&system_path));
        let log = std::fs::File::create(path("daemon.log")).expect("log do daemon");
        let mut command = daemon();
        command
            .arg("--config")
            .arg(&config)
            .env("HOME", path("home"))
            .env("XDG_CONFIG_HOME", path("config"))
            .env("XDG_DATA_HOME", path("data"))
            .env("XDG_STATE_HOME", path("state"))
            .env("XDG_RUNTIME_DIR", path("run"))
            .env("PIPEWIRE_RUNTIME_DIR", pipewire)
            .env("PATH", std::env::join_paths(paths).expect("PATH"))
            .env("FASTWISPER_LOG", "info")
            .env(fw_core::gpu_probe::TIMEOUT_ENV, "1500")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        icd.apply(&mut command);
        let child = command.spawn().expect("fastwisperd");
        TestDaemon { dir, child }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn read(&self, sub: &str) -> String {
        std::fs::read_to_string(self.path(sub)).unwrap_or_default()
    }

    fn socket(&self) -> PathBuf {
        fw_core::daemon::DaemonPaths::in_dir(self.path("run")).socket
    }

    fn alive(&mut self) -> bool {
        self.child.try_wait().expect("estado do daemon").is_none()
    }

    /// Espera `ready` ou falha com o log.
    fn wait_for(&mut self, what: &str, ready: impl Fn(&TestDaemon) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready(self) {
            assert!(
                self.alive(),
                "daemon caiu esperando {what}:\n{}",
                self.read("daemon.log")
            );
            assert!(
                Instant::now() < deadline,
                "sem {what}:\n{}",
                self.read("daemon.log")
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn request(&self, request: &Request) -> fw_core::ipc::Response {
        fw_core::client::send(&self.socket(), request).expect("resposta do daemon")
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// Driver que aborta e driver que trava: o daemon continua de pé (mesmo
/// PID), avisa uma vez (log só com o código, notificação no idioma da
/// interface), carrega o ASR na CPU e não sonda de novo nas cargas seguintes.
#[test]
#[ignore = "PipeWire real (só conecta a captura; audio.sounds = false)"]
fn the_daemon_survives_a_broken_driver_on_the_cpu() {
    for (mode, reason) in [(Mode::Abort, "signal 6"), (Mode::Hang, "timeout")] {
        let icd = FakeIcd::build(mode);
        let mut daemon = TestDaemon::start(&icd, "auto");
        let pid = daemon.child.id();
        daemon.wait_for("a notificação", |d| {
            d.read("notifications").contains(NOTICE_EN)
        });
        daemon.wait_for("o socket", |d| d.socket().exists());
        let status = daemon.request(&Request::new("status", SessionCommand::Status));
        assert!(status.ok, "{status:?}");
        // A carga do início (aquecimento) já rodou na CPU, sem sondar.
        daemon.wait_for("a carga do ASR", |d| {
            d.read("daemon.log").contains("asr indisponível")
        });

        // Outra carga (perfil desempenho): nenhuma sondagem nova.
        let set = daemon.request(&Request::set_vram("vram", VramMode::Desempenho));
        assert!(set.ok, "{set:?}");
        daemon.wait_for("a segunda carga", |d| {
            count(&d.read("daemon.log"), "asr indisponível") >= 2
        });
        assert!(daemon.alive());
        assert_eq!(daemon.child.id(), pid);
        assert_eq!(zombie_probe_children(pid), Vec::<u32>::new(), "{mode:?}");

        let log = daemon.read("daemon.log");
        assert_eq!(count(&log, PROBE_LOG), 1, "{log}");
        assert!(
            log.contains(&format!("reason={reason}"))
                && log.contains("a sondagem do driver Vulkan falhou"),
            "{log}"
        );
        assert_eq!(count(&daemon.read("notifications"), NOTICE_EN), 1);
    }
}

/// `asr.gpu = "cpu"`: nem sonda nem abre o Vulkan, então nem o driver que
/// aborta derruba o daemon, e não há aviso.
#[test]
#[ignore = "PipeWire real (só conecta a captura; audio.sounds = false)"]
fn cpu_never_probes_nor_opens_the_driver() {
    let icd = FakeIcd::build(Mode::Abort);
    let mut daemon = TestDaemon::start(&icd, "cpu");
    daemon.wait_for("o socket", |d| d.socket().exists());
    daemon.wait_for("a carga do ASR", |d| {
        d.read("daemon.log").contains("asr indisponível")
    });
    assert!(daemon.alive());
    let log = daemon.read("daemon.log");
    assert_eq!(count(&log, PROBE_LOG), 0, "{log}");
    assert!(!daemon.read("notifications").contains(NOTICE_EN));
}
