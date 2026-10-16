//! CLI e hub usam a mesma limpeza do fw-core, sempre com XDG temporário.
use fw_core::asr::Language;
use fw_core::history::{History, Stats};
use fw_core::pipeline::{Metrics, Outcome};
use std::process::{Command, Stdio};

fn cli(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .env("XDG_STATE_HOME", root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("CLI")
}

#[test]
fn history_clear_requires_confirmation_and_zeros_stats_with_a_live_writer() {
    let root = tempfile::tempdir().expect("temporário");
    let path = root.path().join("fastwisper/history.sqlite");
    let writer = History::open(&path).expect("conexão do daemon");
    let summary = Outcome::Transcribed {
        text: "texto sintético nunca armazenado".into(),
        language: Language::Pt,
        submit: false,
        metrics: Metrics {
            words: Some(12),
            asr_ms: Some(100),
            speech_ms: Some(2000),
            ..Metrics::default()
        },
    }
    .summary();
    writer.record(&summary, None).expect("ditado");
    let rejected = cli(root.path(), &["history", "clear"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--yes"));
    assert_eq!(writer.stats(7).expect("mantido").asr_ms.count, 1);
    let cleared = cli(root.path(), &["history", "clear", "--yes"]);
    assert!(cleared.status.success(), "{:?}", cleared);
    assert_eq!(writer.stats(90).expect("zerado"), Stats::default());
    assert!(
        String::from_utf8_lossy(&cli(root.path(), &["history"]).stdout).contains("dictations 0")
    );
    writer
        .record(&summary, None)
        .expect("próximo ditado, mesma conexão");
    assert_eq!(fw_core::history::clear(&path).expect("ação irmã do hub"), 1);
    assert_eq!(writer.stats(7).expect("paridade"), Stats::default());
}

#[test]
fn clearing_an_absent_history_creates_nothing() {
    let root = tempfile::tempdir().expect("temporário");
    assert!(
        cli(root.path(), &["history", "clear", "--yes"])
            .status
            .success()
    );
    assert!(!root.path().join("fastwisper/history.sqlite").exists());
}

#[test]
fn gpus_json_is_an_array_of_names_without_config() {
    let root = tempfile::tempdir().expect("temporário");
    let output = cli(root.path(), &["gpus", "--json"]);
    assert!(output.status.success());
    let _: Vec<String> = serde_json::from_slice(&output.stdout).expect("GPUs");
}
