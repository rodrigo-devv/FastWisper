//! Entrada/saída automáticas com detector real, /proc e GameMode falsos.
mod common;

use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};
use std::time::{Duration, Instant};

use common::{FakeDesktop, TestDaemon};
use fw_core::daemon::GAME_MODE_NOTICE;
use fw_core::game_detector::{GameModeSource, SystemGameDetector};
use fw_core::pipeline::NullPipeline;
use fw_core::session::Command;
use fw_core::vram::{Profile, VramMode, VramPolicy};

struct FakeBus(Arc<AtomicI32>);
impl GameModeSource for FakeBus {
    fn client_count(&self) -> Option<u32> {
        u32::try_from(self.0.load(Ordering::SeqCst)).ok()
    }
}
async fn wait_profile(daemon: &TestDaemon, expected: Profile) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let status = daemon.ctl(Command::Status).await.vram.expect("VRAM");
        assert_eq!(status.mode, VramMode::Automatico);
        if status.profile == expected {
            break;
        }
        assert!(Instant::now() < deadline, "perfil não mudou: {status:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn real_detector_drives_automatic_profile_and_content_free_notice() {
    let proc = tempfile::tempdir().expect("proc falso");
    let bus = Arc::new(AtomicI32::new(-1)); // D-Bus indisponível.
    let detector = Arc::new(SystemGameDetector::new(
        FakeBus(Arc::clone(&bus)),
        proc.path().into(),
        rustix::process::getuid().as_raw(),
    ));
    let desktop = Arc::new(FakeDesktop::default());
    let daemon = TestDaemon::start_configured(Arc::new(NullPipeline), Arc::clone(&desktop), |d| {
        d.with_vram(VramPolicy::default())
            .with_game_detector(detector)
    })
    .await;
    wait_profile(&daemon, Profile::SobDemanda).await;
    let pid = proc.path().join("123");
    fs::create_dir(&pid).expect("pid");
    fs::write(
        pid.join("environ"),
        b"PRIVATE=segredo-do-jogo\0SteamGameId=999999\0",
    )
    .expect("env");
    wait_profile(&daemon, Profile::Jogo).await;
    // Steam fecha, mas GameMode ainda tem um cliente: continua em jogo.
    bus.store(1, Ordering::SeqCst);
    fs::remove_dir_all(&pid).expect("fechou Steam");
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        daemon
            .ctl(Command::Status)
            .await
            .vram
            .expect("VRAM")
            .profile,
        Profile::Jogo
    );
    bus.store(0, Ordering::SeqCst);
    wait_profile(&daemon, Profile::SobDemanda).await;
    assert_eq!(
        desktop.notices.lock().expect("notices").as_slice(),
        [GAME_MODE_NOTICE]
    );
    let status = daemon.ctl(Command::Status).await;
    assert!(
        !serde_json::to_string(&status)
            .expect("JSON")
            .contains("segredo-do-jogo")
    );
    daemon.stop().await;
}

struct CountDetector(Arc<std::sync::atomic::AtomicUsize>);
impl fw_core::vram::GameDetector for CountDetector {
    fn is_game_running(&self) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        false
    }
    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_millis(10))
    }
}

#[tokio::test]
async fn detector_only_probes_in_automatic_mode() {
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let daemon = TestDaemon::start_configured(
        Arc::new(NullPipeline),
        Arc::new(FakeDesktop::default()),
        |d| {
            d.with_vram(VramPolicy::new(
                VramMode::Jogo,
                Duration::ZERO,
                fw_core::vram::GameLlm::Off,
            ))
            .with_game_detector(Arc::new(CountDetector(Arc::clone(&count))))
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    common::send(
        &daemon.paths.socket,
        fw_core::ipc::Request::set_vram("auto", VramMode::Automatico),
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(2);
    while count.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    common::send(
        &daemon.paths.socket,
        fw_core::ipc::Request::set_vram("manual", VramMode::Desempenho),
    )
    .await;
    // Uma consulta já disparada pode terminar depois do comando.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let stopped = count.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(count.load(Ordering::SeqCst), stopped);
    daemon.stop().await;
}
