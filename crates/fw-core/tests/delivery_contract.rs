//! Contrato de entrega: destino capturado no press,
//! resultado preparado antes de colar, foco mudado → copy-only, sessão
//! cancelada nunca entrega, copy-last nunca cola.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{FakeDesktop, GatedPipeline, MARKER_TEXT, TestDaemon, window};
use fw_core::inject::{CopyReason, Delivery};
use fw_core::ipc::ErrorCode;
use fw_core::pipeline::{Metrics, Outcome, OutcomeKind};
use fw_core::session::{Command, StateName};

async fn setup() -> (Arc<GatedPipeline>, Arc<FakeDesktop>, TestDaemon) {
    let pipeline = Arc::new(GatedPipeline::default());
    let desktop = Arc::new(FakeDesktop::default());
    let daemon = TestDaemon::start_desktop(Arc::clone(&pipeline), Arc::clone(&desktop)).await;
    (pipeline, desktop, daemon)
}

async fn wait_idle(daemon: &TestDaemon) {
    for _ in 0..200 {
        if daemon.ctl(Command::Status).await.state == Some(StateName::Idle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("não voltou para idle");
}

#[tokio::test]
async fn terminal_or_desktop_harness_enables_vibe_formatting_in_the_pipeline() {
    let (pipeline, desktop, daemon) = setup().await;
    for (terminal, harness, expected) in [
        (false, false, false),
        (true, false, true),
        (false, true, true),
    ] {
        let mut target = window("0xc");
        target.terminal = terminal;
        target.harness = harness;
        desktop.focus(Some(target));
        daemon.ctl(Command::Press).await;
        daemon.ctl(Command::Release).await;
        pipeline.release_one();
        wait_idle(&daemon).await;
        assert_eq!(
            pipeline.literal.lock().expect("lock").last(),
            Some(&expected)
        );
    }
    daemon.stop().await;
}

#[tokio::test]
async fn pastes_into_the_window_focused_at_press() {
    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    let delivered = desktop.delivered.lock().expect("lock").clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, MARKER_TEXT);
    assert_eq!(
        delivered[0].1.as_ref().map(|t| t.address.as_str()),
        Some("0xa")
    );
    let last = daemon.ctl(Command::Status).await.last.expect("resumo");
    assert_eq!(last.delivery, Some(Delivery::Pasted));
    assert!(
        last.metrics.e2e_ms.is_some(),
        "release → entrega medido: {:?}",
        last.metrics
    );
    daemon.stop().await;
}

#[tokio::test]
async fn focus_change_during_asr_never_pastes_into_the_new_window() {
    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;

    // O usuário troca de janela enquanto o Whisper trabalha.
    desktop.focus(Some(window("0xb")));
    pipeline.release_one();
    wait_idle(&daemon).await;

    let delivered = desktop.delivered.lock().expect("lock").clone();
    assert_eq!(
        delivered[0].1.as_ref().map(|t| t.address.as_str()),
        Some("0xa"),
        "o destino é o do press, não o foco atual"
    );
    let status = daemon.ctl(Command::Status).await;
    assert_eq!(
        status.last.and_then(|l| l.delivery),
        Some(Delivery::Copied(CopyReason::FocusChanged))
    );
    // O texto continua recuperável.
    assert_eq!(
        daemon.ctl(Command::ShowLast).await.text.as_deref(),
        Some(MARKER_TEXT)
    );
    daemon.stop().await;
}

#[tokio::test]
async fn cancelled_or_silent_sessions_deliver_nothing() {
    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));

    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    daemon.ctl(Command::Cancel).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    pipeline.set_outcome(Outcome::NoSpeech(Metrics::default()));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    assert!(desktop.delivered.lock().expect("lock").is_empty());
    assert_eq!(
        daemon.ctl(Command::Status).await.last.map(|l| l.outcome),
        Some(OutcomeKind::NO_SPEECH)
    );
    daemon.stop().await;
}

#[tokio::test]
async fn failures_are_announced_without_dictated_text() {
    let (pipeline, desktop, daemon) = setup().await;
    pipeline.refuse_start(Some(fw_core::pipeline::Failure::DeviceUnavailable));
    daemon.ctl(Command::Press).await;
    pipeline.refuse_start(None);
    pipeline.set_outcome(Outcome::Failed(
        fw_core::pipeline::Failure::DeviceLost,
        Metrics::default(),
    ));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    let notices = desktop.notices.lock().expect("lock").clone();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("não está conectado"), "{notices:?}");
    assert!(notices[1].contains("desconectou"), "{notices:?}");
    assert!(notices.iter().all(|n| !n.contains(MARKER_TEXT)));
    daemon.stop().await;
}

#[tokio::test]
async fn refused_press_does_not_keep_a_stale_target() {
    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));
    pipeline.refuse_start(Some(fw_core::pipeline::Failure::DeviceUnavailable));
    assert!(!daemon.ctl(Command::Press).await.ok);

    pipeline.refuse_start(None);
    desktop.focus(Some(window("0xc")));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    let delivered = desktop.delivered.lock().expect("lock").clone();
    assert_eq!(
        delivered[0].1.as_ref().map(|t| t.address.as_str()),
        Some("0xc")
    );
    daemon.stop().await;
}

#[tokio::test]
async fn copy_last_only_copies_and_paste_last_is_explicit() {
    let (pipeline, desktop, daemon) = setup().await;

    // Sem resultado: nada é copiado nem colado.
    let none = daemon.ctl(Command::CopyLast).await;
    assert_eq!(none.error_code(), Some(ErrorCode::NoResult));
    assert_eq!(
        daemon.ctl(Command::PasteLast).await.error_code(),
        Some(ErrorCode::NoResult)
    );

    desktop.focus(Some(window("0xa")));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    let copied = daemon.ctl(Command::CopyLast).await;
    assert!(copied.ok);
    assert_eq!(
        copied.delivery,
        Some(Delivery::Copied(CopyReason::Requested))
    );
    assert_eq!(copied.text, None, "copy-last não devolve o texto");
    assert_eq!(*desktop.copied.lock().expect("lock"), [MARKER_TEXT]);
    assert!(
        desktop.pasted_last.lock().expect("lock").is_empty(),
        "copy-last nunca cola"
    );

    let pasted = daemon.ctl(Command::PasteLast).await;
    assert_eq!(pasted.delivery, Some(Delivery::Pasted));
    assert_eq!(*desktop.pasted_last.lock().expect("lock"), [MARKER_TEXT]);
    assert_eq!(
        pipeline.started.load(Ordering::SeqCst),
        1,
        "colar não grava"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn cancel_after_delivery_blocks_paste_last() {
    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    daemon.ctl(Command::Cancel).await;
    assert_eq!(
        daemon.ctl(Command::PasteLast).await.error_code(),
        Some(ErrorCode::NoResult)
    );
    assert!(desktop.pasted_last.lock().expect("lock").is_empty());
    daemon.stop().await;
}

mod real_hyprland {
    //! Hyprland + wl-copy de verdade, com janelas Ghostty de teste que gravam
    //! o que recebem. Rouba o foco por alguns segundos:
    //! `cargo test -p fw-core --test delivery_contract -- --ignored --test-threads=1`

    use std::path::{Path, PathBuf};
    use std::process::{Child, Command as Process};
    use std::time::Duration;

    use fw_core::config::InjectConfig;
    use fw_core::hypr::Hyprland;
    use fw_core::inject::{CopyReason, Delivery, Desktop, HyprDesktop};

    struct TestWindow {
        child: Child,
        out: PathBuf,
        class: String,
    }

    impl TestWindow {
        /// Terminal que grava exatamente `len` bytes recebidos em `out`.
        fn open(dir: &Path, name: &str, len: usize) -> TestWindow {
            let out = dir.join(format!("{name}.txt"));
            let class = format!("fw.test.{name}.p{}", std::process::id());
            let child = Process::new("ghostty")
                .args([
                    "--gtk-single-instance=false",
                    &format!("--class={class}"),
                    "-e",
                    "sh",
                    "-c",
                    // Fica aberta depois de receber, como um app de verdade.
                    &format!(
                        "stty raw -echo; head -c {len} > '{}'; sleep 5",
                        out.display()
                    ),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("ghostty");
            TestWindow { child, out, class }
        }

        fn received(&self) -> String {
            std::fs::read_to_string(&self.out).unwrap_or_default()
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    async fn wait_focus(hypr: &Hyprland, class: &str) {
        for _ in 0..60 {
            if let Ok(Some(w)) = hypr.active_window().await
                && w.class == class
            {
                // Janela recém-aberta ainda em animação: o seletor
                // `address:` do Hyprland não a encontra.
                tokio::time::sleep(Duration::from_millis(1200)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("janela {class} não recebeu foco");
    }

    #[tokio::test]
    #[ignore = "abre janelas no Hyprland do usuário"]
    async fn pastes_with_ctrl_shift_v_into_the_captured_terminal_and_copies_on_focus_change() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let hypr = Hyprland::from_env().expect("Hyprland");
        let desktop = HyprDesktop::new(InjectConfig {
            notify: false,
            ..InjectConfig::default()
        });
        let dir = tempfile::tempdir().expect("tempdir");

        let marker = "fw-ditado-áé-123";
        let first = TestWindow::open(dir.path(), "um", marker.len());
        wait_focus(&hypr, &first.class).await;
        let target = desktop.focused().await;
        assert_eq!(
            target.as_ref().map(|t| t.class.as_str()),
            Some(first.class.as_str())
        );

        assert_eq!(
            desktop.deliver(marker.into(), target, false).await,
            Delivery::Pasted
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(first.received(), marker, "Unicode intacto, sem Enter");

        // Captura em `first`, foco vai para `second` antes da entrega.
        let target = desktop.focused().await;
        let second = TestWindow::open(dir.path(), "dois", 4);
        wait_focus(&hypr, &second.class).await;
        assert_eq!(
            desktop.deliver("nunca".into(), target, false).await,
            Delivery::Copied(CopyReason::FocusChanged)
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(second.received(), "", "nada colado na janela nova");
    }
}

// --- "Enviar agora" (opt-in) ----------------------------------------------
//
// `HyprDesktop` de verdade contra um Hyprland falso (socket Unix num
// diretório temporário) e um `wl-copy` falso: nenhuma tecla chega a uma
// janela real nem o clipboard do desktop é tocado.

mod send_now {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use fw_core::config::InjectConfig;
    use fw_core::hypr::Hyprland;
    use fw_core::inject::{CopyReason, Delivery, Desktop, HyprDesktop, Target};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    const PASTE: &str = "key = \"V\"";
    const ENTER: &str = "key = \"Return\"";

    fn window(address: &str, class: &str, terminal: bool) -> serde_json::Value {
        serde_json::json!({
            "address": address, "stableId": "1", "class": class, "initialClass": class,
            "title": "título privado", "pid": 0, "xwayland": false,
            "tags": if terminal { vec!["terminal*"] } else { vec![] },
        })
    }

    #[derive(Default)]
    struct State {
        focused: serde_json::Value,
        /// Foco depois que o `up` da colagem chega (troca entre colar e Enter).
        focus_after_paste: Option<serde_json::Value>,
        reject_paste: bool,
        log: Vec<String>,
    }

    struct World {
        dir: TempDir,
        state: Arc<Mutex<State>>,
        desktop: HyprDesktop,
    }

    impl World {
        async fn new(config: InjectConfig, focused: serde_json::Value) -> World {
            World::build(config, focused, "cat > \"$0.out\"", || false).await
        }

        async fn build(
            config: InjectConfig,
            focused: serde_json::Value,
            wl_copy_body: &str,
            locked: fn() -> bool,
        ) -> World {
            let dir = super::common::runtime_dir();
            let socket = dir.path().join(".socket.sock");
            let listener = UnixListener::bind(&socket).expect("bind");
            let state = Arc::new(Mutex::new(State {
                focused,
                ..State::default()
            }));
            let served = Arc::clone(&state);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let reply = {
                        let mut state = served.lock().expect("lock");
                        state.log.push(request.clone());
                        if request == "j/activewindow" {
                            state.focused.to_string()
                        } else if request.contains(PASTE) && state.reject_paste {
                            "error: recusado".to_owned()
                        } else {
                            if request.contains(PASTE)
                                && request.contains("state = \"up\"")
                                && let Some(next) = state.focus_after_paste.take()
                            {
                                state.focused = next;
                            }
                            "ok".to_owned()
                        }
                    };
                    let _ = stream.write_all(reply.as_bytes()).await;
                }
            });
            let wl_copy = dir.path().join("wl-copy");
            std::fs::write(&wl_copy, format!("#!/bin/sh\n{wl_copy_body}\n")).expect("wl-copy");
            std::fs::set_permissions(&wl_copy, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            let desktop = HyprDesktop::with_parts(
                Some(Hyprland::with_socket(socket)),
                config,
                wl_copy,
                locked,
            );
            World {
                dir,
                state,
                desktop,
            }
        }

        fn log(&self) -> Vec<String> {
            self.state.lock().expect("lock").log.clone()
        }

        fn keys(&self, key: &str) -> Vec<String> {
            self.log().into_iter().filter(|r| r.contains(key)).collect()
        }

        fn clipboard(&self) -> Option<String> {
            let out: PathBuf = self.dir.path().join("wl-copy.out");
            std::fs::read_to_string(out).ok()
        }

        async fn captured(&self) -> Target {
            self.desktop.focused().await.expect("janela no press")
        }
    }

    fn config(send_now: bool) -> InjectConfig {
        InjectConfig {
            send_now,
            // Testes não disparam `notify-send` no desktop de quem roda.
            notify: false,
            ..InjectConfig::default()
        }
    }

    #[tokio::test]
    async fn enter_goes_after_the_paste_into_the_same_window() {
        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        assert!(target.terminal);
        let delivery = world
            .desktop
            .deliver("rode os testes".into(), Some(target), true)
            .await;
        assert_eq!(delivery, Delivery::Submitted);
        assert_eq!(world.clipboard().as_deref(), Some("rode os testes"));

        let log = world.log();
        let index = |needle: &str, state: &str| {
            log.iter()
                .position(|r| r.contains(needle) && r.contains(&format!("state = \"{state}\"")))
                .unwrap_or_else(|| panic!("{needle} {state} ausente: {log:#?}"))
        };
        let (paste_down, paste_up) = (index(PASTE, "down"), index(PASTE, "up"));
        let (enter_down, enter_up) = (index(ENTER, "down"), index(ENTER, "up"));
        assert!(paste_down < paste_up && paste_up < enter_down && enter_down < enter_up);
        // Foco revalidado entre a colagem e o Enter.
        assert!(
            log[paste_up..enter_down]
                .iter()
                .any(|r| r == "j/activewindow"),
            "{log:#?}"
        );
        let enters = world.keys(ENTER);
        assert_eq!(enters.len(), 2, "um Enter só (down + up): {enters:#?}");
        for request in &enters {
            assert!(request.contains("window = \"address:0xa\""), "{request}");
            assert!(
                request.contains("mods = \"\""),
                "sem modificadores: {request}"
            );
        }
        assert!(
            world.keys(PASTE).iter().all(|r| r.contains("address:0xa")),
            "colagem na mesma janela"
        );
        assert!(
            !log.iter().any(|r| r.contains("título")),
            "título nunca vai ao Lua"
        );
    }

    #[tokio::test]
    async fn listed_desktop_harness_gets_ctrl_v_then_enter() {
        // Classe sintética (não é um app real).
        let mut cfg = config(true);
        cfg.harness_apps = vec!["fixture.ai.desktop".into()];
        let world = World::new(cfg, window("0xe", "fixture.ai.desktop", false)).await;
        let target = world.captured().await;
        assert!(target.harness && !target.terminal);
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Submitted);
        let pastes = world.keys(PASTE);
        assert!(
            pastes.iter().all(|r| r.contains("mods = \"CTRL\"")),
            "{pastes:#?}"
        );
        assert_eq!(world.keys(ENTER).len(), 2);
    }

    #[tokio::test]
    async fn no_enter_with_opt_in_off_without_phrase_or_outside_terminals() {
        // Opt-in desligado (padrão), mesmo com a frase.
        let world = World::new(config(false), window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Pasted);
        assert!(world.keys(ENTER).is_empty());

        // Ditado sem a frase.
        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), false).await;
        assert_eq!(delivery, Delivery::Pasted);
        assert!(world.keys(ENTER).is_empty());

        // App que não é terminal: a regra "nunca Enter" continua.
        let world = World::new(config(true), window("0xb", "example-browser", false)).await;
        let target = world.captured().await;
        assert!(!target.terminal);
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Pasted);
        assert_eq!(world.keys(PASTE).len(), 2);
        assert!(world.keys(ENTER).is_empty());
    }

    /// Negação logo antes da frase ("não vou enviar agora"): o polisher não
    /// pede envio e o texto vai inteiro, sem Enter, mesmo num terminal com o
    /// opt-in ligado. Sem LLM: só regras.
    #[tokio::test]
    async fn negated_phrase_is_pasted_whole_without_enter() {
        let mut cfg = fw_core::config::Config::default();
        cfg.llm.enabled = false;
        cfg.inject.send_now = true;
        let polisher = fw_core::polish::Polisher::new(&cfg);
        for (dictated, pasted) in [
            (
                "ajusta o teste mas não vou enviar agora",
                "Ajusta o teste mas não vou enviar agora.",
            ),
            (
                "fix the test, don't send now",
                "Fix the test, don't send now.",
            ),
        ] {
            let out = polisher
                .polish(dictated, Some(fw_core::asr::Language::Pt), true)
                .await;
            assert!(!out.submit, "{dictated}");
            let world = World::new(config(true), window("0xa", "kitty", true)).await;
            let target = world.captured().await;
            let delivery = world
                .desktop
                .deliver(out.text.clone(), Some(target), out.submit)
                .await;
            assert_eq!(delivery, Delivery::Pasted);
            // Texto inteiro, com a frase (regras de terminal: maiúscula e ponto).
            assert_eq!(world.clipboard().as_deref(), Some(pasted));
            assert_eq!(world.keys(PASTE).len(), 2);
            assert!(world.keys(ENTER).is_empty(), "{dictated}");
        }
    }

    #[tokio::test]
    async fn no_enter_in_copy_only_or_locked_or_when_focus_changed_before_paste() {
        let mut copy_only = config(true);
        copy_only.copy_only_apps = vec!["kitty".into()];
        let world = World::new(copy_only, window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Copied(CopyReason::AppNotApproved));
        assert!(world.keys(PASTE).is_empty() && world.keys(ENTER).is_empty());

        let world = World::build(
            config(true),
            window("0xa", "kitty", true),
            "cat > \"$0.out\"",
            || true,
        )
        .await;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Copied(CopyReason::SessionLocked));
        assert!(world.keys(PASTE).is_empty() && world.keys(ENTER).is_empty());

        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        world.state.lock().expect("lock").focused = window("0xc", "kitty", true);
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Copied(CopyReason::FocusChanged));
        assert!(world.keys(PASTE).is_empty() && world.keys(ENTER).is_empty());
    }

    #[tokio::test]
    async fn no_enter_when_focus_moves_between_paste_and_enter() {
        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        let target = world.captured().await;
        world.state.lock().expect("lock").focus_after_paste = Some(window("0xd", "kitty", true));
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Pasted, "colou, mas não enviou");
        assert_eq!(world.keys(PASTE).len(), 2);
        assert!(world.keys(ENTER).is_empty(), "{:#?}", world.log());
    }

    #[tokio::test]
    async fn no_enter_when_paste_falls_back_to_the_clipboard() {
        // O Hyprland recusa o atalho de colar: fica só no clipboard.
        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        world.state.lock().expect("lock").reject_paste = true;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Copied(CopyReason::ShortcutFailed));
        assert_eq!(world.clipboard().as_deref(), Some("x"));
        assert!(world.keys(ENTER).is_empty());

        // Nem o clipboard funcionou: nada de atalho nem Enter.
        let world = World::build(config(true), window("0xa", "kitty", true), "exit 1", || {
            false
        })
        .await;
        let target = world.captured().await;
        let delivery = world.desktop.deliver("x".into(), Some(target), true).await;
        assert_eq!(delivery, Delivery::Failed);
        assert!(world.keys(PASTE).is_empty() && world.keys(ENTER).is_empty());
    }

    #[tokio::test]
    async fn paste_last_and_copy_last_never_send_enter() {
        let world = World::new(config(true), window("0xa", "kitty", true)).await;
        assert_eq!(
            world.desktop.paste_focused("x".into()).await,
            Delivery::Pasted
        );
        assert_eq!(
            world.desktop.copy("x".into()).await,
            Delivery::Copied(CopyReason::Requested)
        );
        assert!(world.keys(ENTER).is_empty());
    }
}

/// O pedido de "enviar agora" do pipeline chega à entrega; ditado normal
/// não pede.
#[tokio::test]
async fn daemon_passes_the_send_now_request_to_the_delivery() {
    use fw_core::asr::Language;

    let (pipeline, desktop, daemon) = setup().await;
    desktop.focus(Some(window("0xa")));
    for submit in [false, true] {
        pipeline.set_outcome(Outcome::Transcribed {
            text: MARKER_TEXT.into(),
            language: Language::Pt,
            metrics: Metrics::default(),
            submit,
        });
        daemon.ctl(Command::Press).await;
        daemon.ctl(Command::Release).await;
        pipeline.release_one();
        wait_idle(&daemon).await;
    }
    // Só a frase (texto vazio depois de tirá-la): nada é entregue.
    pipeline.set_outcome(Outcome::Transcribed {
        text: String::new(),
        language: Language::Pt,
        metrics: Metrics::default(),
        submit: false,
    });
    daemon.ctl(Command::Press).await;
    daemon.ctl(Command::Release).await;
    pipeline.release_one();
    wait_idle(&daemon).await;

    assert_eq!(*desktop.submits.lock().expect("lock"), [false, true]);
    assert_eq!(desktop.delivered.lock().expect("lock").len(), 2);
    daemon.stop().await;
}
