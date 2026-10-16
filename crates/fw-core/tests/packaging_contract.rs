//! All installer effects stay in disposable HOME/XDG roots with activation mocks.
fn run_script(script: &str) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("bash")
        .arg(root.join(script))
        .output()
        .expect("bash packaging contract");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn isolated_install_upgrade_rollback_and_uninstall() {
    run_script("packaging/omarchy/tests/contract.sh");
}

/// Pílula: instala, habilita, sem restart do shell quando o QML não mudou,
/// rollback/uninstall removem.
#[test]
fn pill_plugin_install_is_idempotent_and_reversible() {
    run_script("packaging/omarchy/tests/pill.sh");
}

/// Caminho de instalação com aspas, barra invertida, `|`, `&` e `$`: Lua/QML escapados,
/// bloco do bindings.lua mesmo com um Alt+Space do usuário, sem duplicar.
#[test]
fn install_escapes_strange_paths_and_keeps_user_binds() {
    run_script("packaging/omarchy/tests/paths.sh");
}
