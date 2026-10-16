//! Atalhos editáveis de ponta a ponta com a CLI real, `hyprctl` falso e o
//! `hyprland.lua` instalado rodando num Hyprland falso em Lua
//! (`packaging/omarchy/tests/hotkeys.sh`). HOME/XDG/PATH descartáveis.
#[test]
fn hotkeys_change_reload_release_and_conflicts_in_a_fake_hyprland() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("bash")
        .arg(root.join("packaging/omarchy/tests/hotkeys.sh"))
        .env("FASTWISPER_BIN", env!("CARGO_BIN_EXE_fastwisper"))
        .output()
        .expect("bash");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
