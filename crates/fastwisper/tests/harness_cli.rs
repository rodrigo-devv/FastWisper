//! Paridade de harnesses desktop: catálogo, get/set/list e config temporária.
use std::process::Command;

#[test]
fn harness_apps_have_cli_catalog_parity_and_no_unconfirmed_defaults() {
    let root = tempfile::tempdir().expect("config temporária");
    let config = root.path().join("config.toml");
    std::fs::write(&config, "# configuração temporária\n").expect("config");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fastwisper"))
            .env("XDG_RUNTIME_DIR", root.path())
            .args(args)
            .args(["--config", config.to_str().expect("path")])
            .output()
            .expect("CLI")
    };
    let catalog: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../ui/src/lib/settings/fields.json"))
            .expect("catálogo");
    assert!(catalog.iter().any(|field| {
        field["key"] == "inject.harness_apps"
            && field["kind"] == "list"
            && field["page"] == "vibe-coding"
    }));
    let default = run(&["config", "get", "inject.harness_apps"]);
    assert!(default.status.success());
    assert_eq!(String::from_utf8_lossy(&default.stdout), "[]\n");
    let list = run(&["config", "list"]);
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("inject.harness_apps"));
    let set = run(&[
        "config",
        "set",
        "inject.harness_apps",
        r#"["fixture.ai.desktop"]"#,
    ]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let get = run(&["config", "get", "inject.harness_apps"]);
    assert!(get.status.success());
    assert_eq!(
        String::from_utf8_lossy(&get.stdout),
        "[\"fixture.ai.desktop\"]\n"
    );
    assert_eq!(
        fw_core::config::Config::load_path(&config, true)
            .expect("config válida")
            .inject
            .harness_apps,
        ["fixture.ai.desktop"]
    );
    let before = std::fs::read(&config).expect("config");
    assert!(
        !run(&["config", "set", "inject.harness_apps", "true"])
            .status
            .success()
    );
    assert_eq!(std::fs::read(config).expect("config"), before);
}
