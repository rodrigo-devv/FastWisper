//! O launcher só executa o hub irmão, sem procurar executáveis no PATH.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn hub_launches_sibling_binary_and_reports_missing_installation() {
    let dir = tempfile::tempdir().expect("diretório temporário");
    let cli = dir.path().join("fastwisper");
    fs::copy(env!("CARGO_BIN_EXE_fastwisper"), &cli).expect("copiar CLI");
    let home = dir.path().join("home");
    let missing = |marker: Option<&str>| {
        let state = home.join(".local/state/fastwisper/packaging");
        let _ = fs::remove_dir_all(&state);
        if let Some(marker) = marker {
            fs::create_dir_all(&state).expect("estado");
            fs::write(state.join("hub-status"), format!("{marker}\n")).expect("marcador");
        }
        let output = Command::new(&cli)
            .arg("hub")
            .env("HOME", &home)
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("executar CLI");
        assert!(!output.status.success());
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    // Sem marcador do install.sh: mensagem genérica, sem culpar o --no-hub.
    let generic = missing(None);
    assert!(
        generic.contains("run packaging/omarchy/install.sh again"),
        "{generic}"
    );
    assert!(!generic.contains("--no-hub"), "{generic}");
    for (marker, reason) in [
        ("no_hub", "without --no-hub"),
        ("missing_node", "node/npm was missing"),
        ("missing_webkit", "webkit2gtk-4.1 was missing"),
        ("build_failed", "its build failed"),
        ("external_build", "FASTWISPER_HUB_BUILD_DIR"),
        ("installed", "run packaging/omarchy/install.sh again"),
    ] {
        let stderr = missing(Some(marker));
        assert!(stderr.contains(reason), "{marker}: {stderr}");
    }
    // Mesmo motivo em pt-BR.
    fs::create_dir_all(home.join(".config/fastwisper")).expect("config");
    fs::write(
        home.join(".config/fastwisper/config.toml"),
        "[ui]\nlanguage = \"pt-BR\"\n",
    )
    .expect("config pt-BR");
    let stderr = missing(Some("missing_webkit"));
    assert!(stderr.contains("faltou webkit2gtk-4.1"), "{stderr}");
    let hub = dir.path().join("fastwisper-hub");
    fs::write(
        &hub,
        "#!/bin/sh\nprintf \"launched $WEBKIT_DISABLE_DMABUF_RENDERER $*\" > \"$HUB_TEST_MARKER\"\n",
    )
    .expect("hub falso");
    fs::set_permissions(&hub, fs::Permissions::from_mode(0o755)).expect("permissões");
    let marker = dir.path().join("launched");
    assert!(
        Command::new(&cli)
            .args(["hub", "temas"])
            .env("HUB_TEST_MARKER", &marker)
            .env_remove("WEBKIT_DISABLE_DMABUF_RENDERER")
            .status()
            .expect("abrir hub")
            .success()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(marker).expect("hub executado"),
        "launched 1 --page temas"
    );
}
