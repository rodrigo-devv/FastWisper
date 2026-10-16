//! CLI real + actor real; modelos e desktop falsos, runtime/config temporários.
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use fw_core::daemon::{Daemon, DaemonPaths};
use fw_core::inject::NoDesktop;
use fw_core::pipeline::{Failure, Metrics, Outcome, Pipeline};
use fw_core::session::Ticket;
use fw_core::vram::{ModelOp, ModelState, Models, Residency, VramPolicy};

#[derive(Default)]
struct FakePipeline(Arc<AtomicBool>);
impl Pipeline for FakePipeline {
    fn start(&self, _: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        std::future::ready(Ok(()))
    }
    fn finish(&self, _: Ticket, _: bool) -> impl Future<Output = Outcome> + Send + 'static {
        std::future::ready(Outcome::NoSpeech(Metrics::default()))
    }
    fn abort(&self, _: Ticket) {}
    fn load(&self, _: ModelOp, _: Models) -> impl Future<Output = ()> + Send + 'static {
        self.0.store(true, Ordering::SeqCst);
        std::future::ready(())
    }
    fn unload(&self, _: ModelOp, _: Models) -> impl Future<Output = ()> + Send + 'static {
        self.0.store(false, Ordering::SeqCst);
        std::future::ready(())
    }
    fn residency(&self) -> Residency {
        let loaded = self.0.load(Ordering::SeqCst);
        Residency {
            asr: if loaded {
                ModelState::Loaded
            } else {
                ModelState::Unloaded
            },
            llm: ModelState::Unloaded,
            vram_bytes: Some(if loaded { 32 << 20 } else { 0 }),
        }
    }
}
async fn cli(runtime: &Path, args: &[&str]) -> Output {
    let runtime = runtime.to_owned();
    let args: Vec<_> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_fastwisper"))
            .env("XDG_RUNTIME_DIR", runtime)
            .args(args)
            .output()
            .expect("CLI")
    })
    .await
    .expect("worker")
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("pt-BR");
    println!("{text}");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vram_query_switch_persistence_bar_and_immediate_unload() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let config = root.path().join("config.toml");
    let original = "# config de teste\n[performance]\nvram = \"automatico\" # comentário pessoal\nunload_after_secs = 300\n";
    fs::write(&config, original).expect("config");
    let paths = DaemonPaths::in_dir(root.path());
    let daemon = Daemon::bind(paths)
        .expect("daemon")
        .with_vram(VramPolicy::default());
    let pipeline = Arc::new(FakePipeline::default());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(
        daemon.run(Arc::clone(&pipeline), Arc::new(NoDesktop), async {
            let _ = stopped.await;
        }),
    );
    let config_path = config.to_str().expect("path");
    let output = success(cli(root.path(), &["vram", "--config", config_path]).await);
    assert!(output.contains("Mode: automatico · active profile: sob_demanda"));
    assert!(output.contains("32 MiB"));
    success(cli(root.path(), &["vram", "jogo", "--config", config_path]).await);
    assert_eq!(
        fs::read_to_string(&config).expect("config"),
        original.replace("\"automatico\"", "\"jogo\"")
    );
    assert_eq!(
        fs::metadata(&config).expect("metadata").mode() & 0o777,
        0o600
    );
    let bar = success(cli(root.path(), &["status", "--bar"]).await);
    let bar: serde_json::Value = serde_json::from_str(&bar).expect("JSON");
    assert!(
        bar["tooltip"]
            .as_str()
            .expect("tooltip")
            .contains("Perfil: jogo")
    );
    assert_eq!(bar["profile"], "jogo");
    assert!(!bar.to_string().contains("texto ditado"));
    success(
        cli(
            root.path(),
            &["vram", "desempenho", "--config", config_path],
        )
        .await,
    );
    assert!(pipeline.0.load(Ordering::SeqCst));
    success(cli(root.path(), &["ctl", "unload"]).await);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let output = success(cli(root.path(), &["vram"]).await);
    assert!(output.contains("0 MiB"));
    assert!(!pipeline.0.load(Ordering::SeqCst));
    let before = fs::read_to_string(&config).expect("config");
    assert!(
        !cli(root.path(), &["vram", "inválido", "--config", config_path])
            .await
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&config).expect("config"), before);
    stop.send(()).expect("stop");
    worker.await.expect("daemon");
    let output = success(cli(root.path(), &["vram", "jogo", "--config", config_path]).await);
    assert!(output.contains("changes apply on the next start"));
    assert_eq!(
        fs::read_to_string(&config).expect("config"),
        before.replace("\"desempenho\"", "\"jogo\""),
        "sem daemon salva para o próximo início"
    );
}
