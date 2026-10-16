use fw_core::notes::Notes;
use std::process::{Command, Stdio};
fn cli(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .env("XDG_STATE_HOME", root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("contrato de notas")
}
#[test]
fn list_and_clear_are_explicit_and_never_print_text_in_stderr() {
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("fastwisper/notes/notes.sqlite");
    assert!(cli(root.path(), &["notes", "list"]).status.success());
    assert!(
        cli(root.path(), &["notes", "clear", "--yes"])
            .status
            .success()
    );
    assert!(!path.exists());
    let notes = Notes::new(&path);
    notes
        .save(true, "synthetic private note", "en")
        .expect("contrato de notas");
    let list = cli(root.path(), &["notes", "list"]);
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("synthetic private note"));
    assert!(!String::from_utf8_lossy(&list.stderr).contains("synthetic private note"));
    let rejected = cli(root.path(), &["notes", "clear"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--yes"));
    assert_eq!(notes.list().expect("contrato de notas").len(), 1);
    assert!(
        cli(root.path(), &["notes", "clear", "--yes"])
            .status
            .success()
    );
    assert!(notes.list().expect("contrato de notas").is_empty());
}
#[test]
fn consent_has_cli_and_catalog_parity_and_is_off_by_default() {
    let root = tempfile::tempdir().expect("contrato de notas");
    let config = root.path().join("config.toml");
    std::fs::write(&config, "# temporary config\n").expect("contrato de notas");
    let path = config.to_str().expect("contrato de notas");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fastwisper"))
            .env("XDG_RUNTIME_DIR", root.path())
            .args(args)
            .output()
            .expect("contrato de notas")
    };
    let catalog: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../ui/src/lib/settings/fields.json"))
            .expect("contrato de notas");
    assert!(
        catalog
            .iter()
            .any(|f| f["key"] == "experimental.notes" && f["kind"] == "toggle")
    );
    assert_eq!(
        String::from_utf8_lossy(
            &run(&["config", "get", "experimental.notes", "--config", path]).stdout
        ),
        "false\n"
    );
    assert!(
        run(&[
            "config",
            "set",
            "experimental.notes",
            "true",
            "--config",
            path
        ])
        .status
        .success()
    );
    assert_eq!(
        String::from_utf8_lossy(
            &run(&["config", "get", "experimental.notes", "--config", path]).stdout
        ),
        "true\n"
    );
    assert!(
        run(&[
            "config",
            "set",
            "experimental.notes",
            "false",
            "--config",
            path
        ])
        .status
        .success()
    );
    assert!(
        !fw_core::config::Config::load_path(&config, true)
            .expect("contrato de notas")
            .experimental
            .notes
    );
}
