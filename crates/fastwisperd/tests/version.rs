//! `fastwisperd --version` sai antes de abrir config, socket ou modelo.
use std::process::Command;

#[test]
fn daemon_reports_the_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_fastwisperd"))
        .arg("--version")
        .output()
        .expect("fastwisperd --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("fastwisperd {}\n", env!("CARGO_PKG_VERSION"))
    );
}
