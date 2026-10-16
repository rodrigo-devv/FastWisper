//! Driver Vulkan instalado mas quebrado (ICD falso que trava ou aborta): a
//! CLI sonda num processo filho e segue na CPU, sem cair. Só no build com
//! Vulkan (o build da CI é só CPU e nunca abre o Vulkan).
#![cfg(feature = "vulkan")]

#[path = "../../fastwisperd/tests/fake_icd/mod.rs"]
mod fake_icd;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use fake_icd::{FakeIcd, Mode};

const BROKEN: &str = "Vulkan driver failed the probe";

/// Diretórios isolados (nada da config ou do daemon da sessão).
struct Isolated {
    dir: tempfile::TempDir,
}

impl Isolated {
    fn new(gpu: &str) -> Isolated {
        let dir = tempfile::tempdir().expect("diretório isolado");
        for sub in ["home", "config/fastwisper", "data", "runtime"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("diretório isolado");
        }
        std::fs::write(
            dir.path().join("config/fastwisper/config.toml"),
            format!("[asr]\ngpu = \"{gpu}\"\n\n[llm]\nenabled = false\n"),
        )
        .expect("config isolada");
        Isolated { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn run(&self, icd: &FakeIcd, args: &[&str]) -> (Output, Duration) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastwisper"));
        command
            .args(args)
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("FASTWISPER_LOG")
            .env(fw_core::gpu_probe::TIMEOUT_ENV, "1500");
        icd.apply(&mut command);
        let started = Instant::now();
        let output = command.output().expect("fastwisper");
        (output, started.elapsed())
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Sem sinal: o processo da CLI terminou sozinho (não foi derrubado).
fn exited(output: &Output) -> bool {
    use std::os::unix::process::ExitStatusExt;
    output.status.signal().is_none() && output.status.code().is_some()
}

#[test]
fn gpus_lists_nothing_and_warns_with_a_broken_driver() {
    let home = Isolated::new("auto");
    for (mode, reason) in [(Mode::Abort, "signal 6"), (Mode::Hang, "timeout")] {
        let icd = FakeIcd::build(mode);
        let (output, elapsed) = home.run(&icd, &["gpus", "--json"]);
        assert!(output.status.success(), "{mode:?}: {output:?}");
        assert_eq!(text(&output.stdout).trim(), "[]");
        assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");

        let (output, _) = home.run(&icd, &["gpus"]);
        assert!(output.status.success(), "{mode:?}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(
            text(&output.stderr).contains(&format!("{BROKEN} ({reason})")),
            "{}",
            text(&output.stderr)
        );
    }
}

#[test]
fn doctor_warns_about_a_broken_driver_and_keeps_going() {
    let home = Isolated::new("auto");
    for (mode, reason) in [(Mode::Abort, "signal 6"), (Mode::Hang, "timeout")] {
        let icd = FakeIcd::build(mode);
        let (output, elapsed) = home.run(&icd, &["doctor"]);
        let stdout = text(&output.stdout);
        assert!(exited(&output), "{mode:?}: {output:?}");
        assert!(
            stdout.contains(&format!(
                "! gpu          {BROKEN} ({reason}): transcription uses the CPU"
            )),
            "{stdout}"
        );
        assert!(stdout.contains("fastwisper restart"), "{stdout}");
        // As checagens depois da GPU também rodaram.
        assert!(stdout.contains("model"), "{stdout}");
        assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
    }
}

/// `asr.gpu = "cpu"` não sonda nem abre o Vulkan: nem o driver que aborta
/// derruba o doctor, e não há aviso de driver.
#[test]
fn cpu_skips_the_probe() {
    let home = Isolated::new("cpu");
    let icd = FakeIcd::build(Mode::Abort);
    let (output, _) = home.run(&icd, &["doctor"]);
    let stdout = text(&output.stdout);
    assert!(exited(&output), "{output:?}");
    assert!(!stdout.contains(BROKEN), "{stdout}");
    assert!(stdout.contains("asr.gpu = \"cpu\""), "{stdout}");
}

/// `transcribe` carrega o Whisper no próprio processo: com o driver
/// quebrado, a CLI se reexecuta sem Vulkan e transcreve na CPU. Usa o modelo
/// e o áudio reais (`asr_contract`); sem o modelo, o teste é pulado.
#[test]
#[ignore = "modelo real do Whisper na CPU (lento)"]
fn transcribe_falls_back_to_the_cpu() {
    let models = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
        .map(|data| data.join("fastwisper/models"));
    let model = std::env::var_os("FASTWISPER_TEST_MODEL")
        .map(PathBuf::from)
        .into_iter()
        .chain(models.into_iter().flat_map(|dir| {
            ["ggml-small-q5_1.bin", "ggml-large-v3-turbo-q5_0.bin"].map(|m| dir.join(m))
        }))
        .find(|path| path.is_file());
    let Some(model) = model else {
        eprintln!("sem modelo (FASTWISPER_TEST_MODEL): pulado");
        return;
    };
    let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/en-jfk.wav");
    let home = Isolated::new("auto");
    let icd = FakeIcd::build(Mode::Abort);
    let model = model.to_str().expect("caminho UTF-8");
    let wav = wav.to_str().expect("caminho UTF-8");
    let (output, _) = home.run(&icd, &["transcribe", wav, "--model", model, "--lang", "en"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr).contains(&format!("{BROKEN} (signal 6)")),
        "{}",
        text(&output.stderr)
    );
    assert!(
        text(&output.stdout).to_lowercase().contains("country"),
        "{}",
        text(&output.stdout)
    );
}
