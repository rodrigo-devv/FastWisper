//! `models verify` sem id confere o modelo configurado, não o perfil GPU:
//! numa máquina só com CPU o setup grava o `small`. HOME/XDG temporários.
use std::process::{Command, Stdio};

#[test]
fn verify_without_id_checks_the_configured_model() {
    let root = tempfile::tempdir().expect("temporário");
    let config = root.path().join("config/fastwisper");
    let models = root.path().join("data/fastwisper/models");
    std::fs::create_dir_all(&config).expect("pasta da config");
    std::fs::create_dir_all(&models).expect("pasta dos modelos");
    let small = models.join("ggml-small-q5_1.bin");
    std::fs::write(&small, "modelo falso").expect("modelo falso");
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[audio]\ndevice = \"mic\"\n[asr]\nmodel = \"{}\"\n",
            small.display()
        ),
    )
    .expect("config");
    let output = Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .args(["models", "verify"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .stdin(Stdio::null())
        .output()
        .expect("CLI");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("ggml-small-q5_1.bin"), "{text}");
    assert!(!text.contains("large-v3-turbo"), "{text}");
}
