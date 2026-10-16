//! Uma versão só: workspace Cargo × hub (npm, Tauri, Cargo) × notas da release.
//! Só lê arquivos do repositório; nada de daemon, rede ou sessão real.
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}
fn json(path: &str) -> serde_json::Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}
fn toml(path: &str) -> toml_edit::DocumentMut {
    read(path).parse().unwrap_or_else(|e| panic!("{path}: {e}"))
}
/// Versão de um pacote no `Cargo.lock`.
fn locked(path: &str, name: &str) -> String {
    let lock = toml(path);
    let packages = lock["package"]
        .as_array_of_tables()
        .unwrap_or_else(|| panic!("{path} sem [[package]]"));
    let found: Vec<_> = packages
        .iter()
        .filter(|p| p["name"].as_str() == Some(name))
        .filter_map(|p| p["version"].as_str())
        .collect();
    assert_eq!(found.len(), 1, "{path}: {name} deve aparecer uma vez");
    found[0].to_owned()
}

#[test]
fn every_manifest_carries_the_workspace_version() {
    let workspace = toml("Cargo.toml");
    assert_eq!(
        workspace["workspace"]["package"]["version"].as_str(),
        Some(VERSION),
        "Cargo.toml [workspace.package]"
    );
    for name in ["fastwisper", "fastwisperd", "fw-core"] {
        assert_eq!(locked("Cargo.lock", name), VERSION, "Cargo.lock {name}");
    }
    let package = json("ui/package.json");
    assert_eq!(package["version"], VERSION, "ui/package.json");
    let lock = json("ui/package-lock.json");
    assert_eq!(lock["version"], VERSION, "ui/package-lock.json");
    assert_eq!(
        lock["packages"][""]["version"], VERSION,
        "ui/package-lock.json packages[\"\"]"
    );
    assert_eq!(
        json("ui/src-tauri/tauri.conf.json")["version"],
        VERSION,
        "ui/src-tauri/tauri.conf.json"
    );
    assert_eq!(
        toml("ui/src-tauri/Cargo.toml")["package"]["version"].as_str(),
        Some(VERSION),
        "ui/src-tauri/Cargo.toml"
    );
    assert_eq!(
        locked("ui/src-tauri/Cargo.lock", "fastwisper-hub"),
        VERSION,
        "ui/src-tauri/Cargo.lock"
    );
}

#[test]
fn changelog_and_release_notes_cover_the_version() {
    let changelog = read("CHANGELOG.md");
    assert!(
        changelog
            .lines()
            .any(|l| l.starts_with(&format!("## [{VERSION}]"))),
        "CHANGELOG.md sem a seção [{VERSION}]"
    );
    let notes = read(&format!("docs/release/v{VERSION}.md"));
    assert!(notes.contains(VERSION), "notas da release sem a versão");
}

#[test]
fn cli_reports_the_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .arg("--version")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .output()
        .expect("fastwisper --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("fastwisper {VERSION}\n")
    );
}
