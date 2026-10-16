//! Exceção opt-in local: nada no ditado, só comandos explícitos de nota.
mod common;
use common::{FakeDesktop, GatedPipeline, MARKER_TEXT, TestDaemon, window};
use fw_core::{
    asr::Language,
    config::Config,
    inject::{CopyReason, Delivery},
    ipc::{ErrorCode, Request},
    notes::Notes,
    pipeline::{Failure, Metrics, Outcome, OutcomeKind},
    session::{Command, Mode, StateName},
};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, atomic::Ordering};

#[test]
fn store_is_lazy_private_separate_and_clearable() {
    let dir = tempfile::tempdir().expect("contrato de notas");
    let path = dir.path().join("private/notes.sqlite");
    let notes = Notes::new(&path);
    assert!(notes.list().expect("contrato de notas").is_empty());
    assert_eq!(notes.clear().expect("contrato de notas"), 0);
    assert!(notes.save(false, "segredo", "pt").is_err());
    assert!(!path.exists());
    assert!(!path.parent().expect("contrato de notas").exists());
    notes
        .save(true, "primeira nota", "pt")
        .expect("contrato de notas");
    notes
        .save(true, "second voice note", "en")
        .expect("contrato de notas");
    assert_eq!(
        std::fs::metadata(&path)
            .expect("contrato de notas")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().expect("contrato de notas"))
            .expect("contrato de notas")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let list = notes.list().expect("contrato de notas");
    assert_eq!(list[0].text, "second voice note");
    assert_eq!(list[0].words, 3);
    assert_eq!(notes.delete(list[0].id).expect("contrato de notas"), 1);
    assert_eq!(notes.clear().expect("contrato de notas"), 1);
    assert!(notes.list().expect("contrato de notas").is_empty());
    let bytes = std::fs::read(&path).expect("contrato de notas");
    assert!(
        !bytes
            .windows("primeira nota".len())
            .any(|w| w == b"primeira nota")
    );
    notes
        .save(true, "depois de limpar", "pt")
        .expect("contrato de notas");
    assert_eq!(notes.list().expect("contrato de notas").len(), 1);
}

#[test]
fn store_refuses_symlinks_and_insecure_directories() {
    let dir = tempfile::tempdir().expect("contrato de notas");
    let unsafe_dir = dir.path().join("public");
    std::fs::create_dir(&unsafe_dir).expect("contrato de notas");
    std::fs::set_permissions(&unsafe_dir, std::fs::Permissions::from_mode(0o755))
        .expect("contrato de notas");
    assert!(
        Notes::new(unsafe_dir.join("notes.sqlite"))
            .save(true, "segredo", "pt")
            .is_err()
    );
    let path = dir.path().join("notes.sqlite");
    let target = dir.path().join("target");
    std::fs::write(&target, b"unchanged").expect("contrato de notas");
    std::os::unix::fs::symlink(&target, &path).expect("contrato de notas");
    assert!(Notes::new(path).save(true, "segredo", "pt").is_err());
    assert_eq!(
        std::fs::read(target).expect("contrato de notas"),
        b"unchanged"
    );
}

async fn wait_idle(d: &TestDaemon) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if d.ctl(Command::Status).await.state == Some(StateName::Idle) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("contrato de notas");
}

#[tokio::test]
async fn disabled_daemon_refuses_notes_and_normal_dictation_never_creates_store() {
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("notes/notes.sqlite");
    let p = Arc::new(GatedPipeline::default());
    let d = TestDaemon::start_configured(p.clone(), Arc::new(FakeDesktop::default()), |d| {
        d.with_notes_path(&path)
    })
    .await;
    for cmd in [
        Command::NoteStart,
        Command::NoteSave,
        Command::NotesList,
        Command::NotesClear,
    ] {
        assert_eq!(
            d.ctl(cmd).await.error_code(),
            Some(ErrorCode::NotesDisabled)
        );
    }
    assert_eq!(p.started.load(Ordering::SeqCst), 0);
    assert!(d.ctl(Command::Press).await.ok);
    d.ctl(Command::Release).await;
    p.release_one();
    wait_idle(&d).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!path.exists());
    assert!(!path.parent().expect("contrato de notas").exists());
    d.stop().await;
}

#[tokio::test]
async fn note_uses_pipeline_without_delivery_last_text_window_or_metrics() {
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("notes/notes.sqlite");
    let cfg_path = root.path().join("config.toml");
    std::fs::write(&cfg_path, "[experimental]\nnotes=true\n").expect("contrato de notas");
    let cfg = Config::load_path(&cfg_path, true).expect("contrato de notas");
    let p = Arc::new(GatedPipeline::default());
    let desktop = Arc::new(FakeDesktop::default());
    let history_path = root.path().join("history.sqlite");
    let d = TestDaemon::start_configured(p.clone(), desktop.clone(), |d| {
        d.with_notes_path(&path)
            .with_config(&cfg_path, true, cfg)
            .with_history(
                fw_core::history::History::open(&history_path).expect("contrato de notas"),
            )
    })
    .await;
    let start = d.ctl(Command::NoteStart).await;
    assert!(start.ok && start.note_session);
    assert_eq!(start.mode, Some(Mode::Note));
    for cmd in [Command::Press, Command::Release, Command::Toggle] {
        assert_eq!(d.ctl(cmd).await.state, Some(StateName::Recording));
    }
    assert!(!path.exists());
    d.ctl(Command::NoteSave).await;
    assert!(d.ctl(Command::Status).await.note_session);
    p.release_one();
    wait_idle(&d).await;
    assert_eq!(*p.literal.lock().expect("contrato de notas"), vec![false]);
    let listed = d.ctl(Command::NotesList).await;
    let note = &listed.notes.as_ref().expect("contrato de notas")[0];
    assert_eq!(note.text, MARKER_TEXT);
    assert!(
        desktop
            .delivered
            .lock()
            .expect("contrato de notas")
            .is_empty()
    );
    assert_eq!(
        d.ctl(Command::ShowLast).await.error_code(),
        Some(ErrorCode::NoResult)
    );
    let status = d.ctl(Command::Status).await;
    assert!(
        !serde_json::to_string(&status)
            .expect("contrato de notas")
            .contains(MARKER_TEXT)
    );
    assert!(status.last.is_none());
    assert_eq!(
        fw_core::history::History::open(&history_path)
            .expect("contrato de notas")
            .stats(90)
            .expect("contrato de notas")
            .outcomes
            .len(),
        0
    );
    let mut copy = Request::new("copy-note", Command::NotesCopy);
    copy.args.id = Some(note.id);
    assert!(common::send(&d.paths.socket, copy).await.ok);
    assert_eq!(desktop.copied.lock().expect("contrato de notas").len(), 1);
    assert_eq!(d.ctl(Command::NotesClear).await.deleted, Some(1));
    assert!(
        d.ctl(Command::NotesList)
            .await
            .notes
            .expect("contrato de notas")
            .is_empty()
    );
    // Normal dictation with consent enabled still does not become a note.
    d.ctl(Command::Press).await;
    d.ctl(Command::NoteSave).await;
    assert_eq!(
        d.ctl(Command::Status).await.state,
        Some(StateName::Recording)
    );
    d.ctl(Command::NoteCancel).await;
    assert_eq!(
        d.ctl(Command::Status).await.state,
        Some(StateName::Recording)
    );
    d.ctl(Command::Release).await;
    p.release_one();
    wait_idle(&d).await;
    assert!(
        d.ctl(Command::NotesList)
            .await
            .notes
            .expect("contrato de notas")
            .is_empty()
    );
    assert_eq!(
        desktop.delivered.lock().expect("contrato de notas").len(),
        1
    );
    d.stop().await;
}

/// C-04: começar (ou concluir, descartar, cancelar, perder) uma nota não apaga
/// o último ditado; `paste-last`/`copy-last` continuam com ele. Desktop falso:
/// nada chega a uma janela real.
#[tokio::test]
async fn note_keeps_last_dictation_for_paste_last_and_copy_last() {
    const NOTE_TEXT: &str = "nota-sintetica-c04";
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("notes/notes.sqlite");
    let cfg_path = root.path().join("config.toml");
    std::fs::write(&cfg_path, "[experimental]\nnotes=true\n").expect("contrato de notas");
    let cfg = Config::load_path(&cfg_path, true).expect("contrato de notas");
    let p = Arc::new(GatedPipeline::default());
    let desktop = Arc::new(FakeDesktop::default());
    let d = TestDaemon::start_configured(p.clone(), desktop.clone(), |d| {
        d.with_notes_path(&path).with_config(&cfg_path, true, cfg)
    })
    .await;

    desktop.focus(Some(window("0xa")));
    assert!(d.ctl(Command::Press).await.ok);
    d.ctl(Command::Release).await;
    p.release_one();
    wait_idle(&d).await;
    assert_eq!(
        desktop.delivered.lock().expect("contrato de notas").len(),
        1
    );

    // Nota concluída com outro texto.
    p.set_outcome(Outcome::Transcribed {
        text: NOTE_TEXT.into(),
        language: Language::Pt,
        submit: false,
        metrics: Metrics::default(),
    });
    let start = d.ctl(Command::NoteStart).await;
    assert!(start.ok && start.note_session);
    assert_eq!(
        d.ctl(Command::ShowLast).await.text.as_deref(),
        Some(MARKER_TEXT),
        "note_start não apaga o último ditado"
    );
    d.ctl(Command::NoteSave).await;
    p.release_one();
    wait_idle(&d).await;
    // `cancel` (o Esc do atalho) com a nota processando ou gravando cancela
    // só a nota.
    assert!(d.ctl(Command::NoteStart).await.ok);
    d.ctl(Command::NoteSave).await;
    assert!(d.ctl(Command::Cancel).await.ok);
    wait_idle(&d).await;
    assert!(d.ctl(Command::NoteStart).await.ok);
    assert!(d.ctl(Command::Cancel).await.ok);
    wait_idle(&d).await;
    assert_eq!(
        d.ctl(Command::ShowLast).await.text.as_deref(),
        Some(MARKER_TEXT),
        "cancel de nota não apaga o último ditado"
    );
    // Nota descartada e nota que nem abriu o microfone.
    assert!(d.ctl(Command::NoteStart).await.ok);
    d.ctl(Command::NoteCancel).await;
    wait_idle(&d).await;
    p.refuse_start(Some(Failure::DeviceUnavailable));
    assert_eq!(
        d.ctl(Command::NoteStart).await.error_code(),
        Some(ErrorCode::DeviceUnavailable)
    );
    p.refuse_start(None);
    wait_idle(&d).await;

    let listed = d.ctl(Command::NotesList).await;
    assert_eq!(listed.notes.expect("contrato de notas")[0].text, NOTE_TEXT);
    assert_eq!(
        d.ctl(Command::ShowLast).await.text.as_deref(),
        Some(MARKER_TEXT)
    );
    let status = d.ctl(Command::Status).await;
    assert_eq!(
        status.last.map(|last| last.outcome),
        Some(OutcomeKind::TRANSCRIBED)
    );
    let copied = d.ctl(Command::CopyLast).await;
    assert_eq!(
        copied.delivery,
        Some(Delivery::Copied(CopyReason::Requested))
    );
    assert_eq!(
        *desktop.copied.lock().expect("contrato de notas"),
        [MARKER_TEXT]
    );
    let pasted = d.ctl(Command::PasteLast).await;
    assert_eq!(pasted.delivery, Some(Delivery::Pasted));
    assert_eq!(
        *desktop.pasted_last.lock().expect("contrato de notas"),
        [MARKER_TEXT]
    );
    // Sem nota ativa, `cancel` continua apagando o texto recuperável.
    d.ctl(Command::Cancel).await;
    assert_eq!(
        d.ctl(Command::PasteLast).await.error_code(),
        Some(ErrorCode::NoResult)
    );
    d.stop().await;
}

#[tokio::test]
async fn cancel_and_disable_discard_late_note_completions() {
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("notes/notes.sqlite");
    let cfg_path = root.path().join("config.toml");
    std::fs::write(&cfg_path, "[experimental]\nnotes=true\n").expect("contrato de notas");
    let cfg = Config::load_path(&cfg_path, true).expect("contrato de notas");
    let p = Arc::new(GatedPipeline::default());
    let d = TestDaemon::start_configured(p.clone(), Arc::new(FakeDesktop::default()), |d| {
        d.with_notes_path(&path).with_config(&cfg_path, true, cfg)
    })
    .await;
    d.ctl(Command::NoteStart).await;
    d.ctl(Command::NoteSave).await;
    d.ctl(Command::NoteCancel).await;
    wait_idle(&d).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!path.exists());
    d.ctl(Command::NoteStart).await;
    d.ctl(Command::NoteSave).await;
    std::fs::write(&cfg_path, "[experimental]\nnotes=false\n").expect("contrato de notas");
    assert!(d.ctl(Command::ReloadConfig).await.ok);
    p.release_one();
    wait_idle(&d).await;
    // Wait for pending reload and the discarded worker to settle.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        d.ctl(Command::NoteStart).await.error_code(),
        Some(ErrorCode::NotesDisabled)
    );
    assert!(!path.exists());
    d.stop().await;
}

#[test]
fn ipc_note_args_are_closed_and_typed() {
    for command in [
        "note_start",
        "note_save",
        "note_cancel",
        "notes_list",
        "notes_clear",
    ] {
        assert!(
            fw_core::ipc::parse_request(
                format!(r#"{{"v":1,"request_id":"n","cmd":"{command}"}}"#).as_bytes()
            )
            .is_ok()
        );
        assert!(
            fw_core::ipc::parse_request(
                format!(r#"{{"v":1,"request_id":"n","cmd":"{command}","args":{{"id":1}}}}"#)
                    .as_bytes()
            )
            .is_err()
        );
    }
    for command in ["notes_copy", "notes_delete"] {
        assert!(
            fw_core::ipc::parse_request(
                format!(r#"{{"v":1,"request_id":"n","cmd":"{command}","args":{{"id":1}}}}"#)
                    .as_bytes()
            )
            .is_ok()
        );
        for args in [
            "{}",
            r#"{"id":0}"#,
            r#"{"id":"1"}"#,
            r#"{"id":1,"text":"private"}"#,
        ] {
            assert!(
                fw_core::ipc::parse_request(
                    format!(r#"{{"v":1,"request_id":"n","cmd":"{command}","args":{args}}}"#)
                        .as_bytes()
                )
                .is_err()
            );
        }
    }
}

#[cfg(all(feature = "pipewire", feature = "whisper"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PipeWire real e modelo Whisper local; microfone virtual, sons desligados, config temporária"]
async fn real_pipewire_note_saves_test_audio_and_cancel_saves_nothing() {
    use common::virtual_mic::VirtualMic;
    use fw_core::{
        audio,
        dictation::{DictationConfig, DictationPipeline},
        polish::Polisher,
    };
    let root = tempfile::tempdir().expect("contrato de notas");
    let path = root.path().join("notes/notes.sqlite");
    let config_path = root.path().join("config.toml");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pt-casa-velha.wav");
    let pcm = audio::load_wav(&fixture).expect("contrato de notas");
    let mic = VirtualMic::new("Notas opt-in", pcm.samples().to_vec());
    let mut config = Config::default();
    config.experimental.notes = true;
    config.audio.sounds = false;
    config.audio.device = mic.description.clone();
    config.llm.enabled = false;
    if let Ok(model) = std::env::var("FASTWISPER_TEST_MODEL") {
        config.asr.model = model;
    }
    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("contrato de notas"),
    )
    .expect("contrato de notas");
    let pipeline = Arc::new(
        DictationPipeline::new(
            DictationConfig::from_config(&config),
            Polisher::new(&config),
        )
        .expect("contrato de notas"),
    );
    let desktop = Arc::new(FakeDesktop::default());
    let daemon = TestDaemon::start_configured(pipeline, desktop.clone(), |d| {
        d.with_notes_path(&path)
            .with_config(&config_path, true, config)
    })
    .await;
    assert!(daemon.ctl(Command::NoteStart).await.ok);
    tokio::time::sleep(std::time::Duration::from_millis(
        pcm.duration().as_millis() as u64 + 500,
    ))
    .await;
    assert!(daemon.ctl(Command::NoteSave).await.ok);
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            if daemon.ctl(Command::Status).await.state == Some(StateName::Idle) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("contrato de notas");
    let list = daemon
        .ctl(Command::NotesList)
        .await
        .notes
        .expect("contrato de notas");
    assert_eq!(list.len(), 1);
    assert!(!list[0].text.trim().is_empty());
    assert_eq!(list[0].language, "pt");
    assert!(
        desktop
            .delivered
            .lock()
            .expect("contrato de notas")
            .is_empty()
    );
    assert!(desktop.copied.lock().expect("contrato de notas").is_empty());
    daemon.ctl(Command::NotesClear).await;
    assert!(daemon.ctl(Command::NoteStart).await.ok);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(daemon.ctl(Command::NoteCancel).await.ok);
    assert!(
        daemon
            .ctl(Command::NotesList)
            .await
            .notes
            .expect("contrato de notas")
            .is_empty()
    );
    daemon.stop().await;
}

/// "Apagar notas" e "apagar histórico" (A-25): texto-sentinela de uma nota
/// e uma classe de app sintética no histórico somem do arquivo e de
/// qualquer `-wal`/`-journal` da pasta. Limite (SSD, backups, snapshots):
/// cópias fora destes arquivos não são alcançadas.
#[test]
fn clearing_notes_and_history_leaves_no_bytes_in_the_files() {
    use fw_core::history::{self, History};
    use fw_core::pipeline::{LastSummary, Metrics, OutcomeKind};
    let dir = tempfile::tempdir().expect("contrato de notas");
    let leftovers = |folder: &std::path::Path, needle: &str| -> Vec<String> {
        std::fs::read_dir(folder)
            .expect("contrato de notas")
            .map(|entry| entry.expect("contrato de notas").path())
            .filter(|file| {
                std::fs::read(file)
                    .unwrap_or_default()
                    .windows(needle.len())
                    .any(|w| w == needle.as_bytes())
            })
            .map(|file| file.display().to_string())
            .collect()
    };

    let notes_path = dir.path().join("notes/notes.sqlite");
    let notes = Notes::new(&notes_path);
    let sentinel = "sentinela-nota-a25 texto que precisa sumir";
    for _ in 0..20 {
        notes.save(true, sentinel, "pt").expect("contrato de notas");
    }
    let folder = notes_path.parent().expect("contrato de notas");
    assert!(!leftovers(folder, sentinel).is_empty(), "fixture gravada");
    assert_eq!(notes.clear().expect("contrato de notas"), 20);
    assert_eq!(leftovers(folder, sentinel), Vec::<String>::new());

    let history_path = dir.path().join("state/history.sqlite");
    let class = "fixture.sentinela-a25.app";
    // Conexão do "daemon" aberta durante a limpeza, como na vida real.
    let daemon = History::open(&history_path).expect("contrato de notas");
    let summary = LastSummary {
        outcome: OutcomeKind::TRANSCRIBED,
        language: None,
        metrics: Metrics::default(),
        delivery: None,
    };
    for _ in 0..20 {
        daemon
            .record(&summary, Some(class))
            .expect("contrato de notas");
    }
    let folder = history_path.parent().expect("contrato de notas");
    assert!(!leftovers(folder, class).is_empty(), "fixture gravada");
    assert_eq!(
        history::clear(&history_path).expect("contrato de notas"),
        20
    );
    assert_eq!(leftovers(folder, class), Vec::<String>::new());
    daemon
        .record(&summary, None)
        .expect("o daemon continua gravando");
}
