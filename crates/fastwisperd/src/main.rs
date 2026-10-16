//! `fastwisperd`: daemon de ditado. Dono da sessão, do microfone e do modelo.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use fw_core::config::Config;
use fw_core::daemon::{Daemon, DaemonPaths};
use fw_core::dictation::{DictationConfig, DictationPipeline};
use fw_core::game_detector::SystemGameDetector;
use fw_core::gpu_probe;
use fw_core::inject::{Desktop, HyprDesktop};
use fw_core::polish::Polisher;
use fw_core::vram::VramPolicy;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version, about = "Daemon de ditado local do FastWisper")]
struct Args {
    /// Config alternativa (padrão: ~/.config/fastwisper/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    // Filho da sondagem da GPU (`fw_core::gpu_probe`): abre o Vulkan, lista
    // as GPUs e sai, antes de config, socket e logs.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == gpu_probe::CHILD_ARG)
    {
        return gpu_probe::child_main();
    }
    gpu_probe::enable();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("FASTWISPER_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        // journald não interpreta cores ANSI.
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> anyhow::Result<()> {
    let config = Config::load(args.config.as_deref())?;
    // `reload_config` relê este arquivo (sem `--config`, ausente = padrões).
    let (config_path, required) = match args.config {
        Some(path) => (path, true),
        None => (Config::default_path()?, false),
    };
    // Antes de tudo que abre o Vulkan (o Whisper) e de threads: driver
    // quebrado (ou `asr.gpu = "cpu"`) reexecuta este processo sem Vulkan.
    // Uma sondagem por início; `asr.gpu` exige reinício.
    let gpu = gpu_probe::prepare(&config.asr.gpu);
    if let Some(failure) = gpu.probe.failure() {
        // Só o código do motivo (`timeout`, `signal 6`...).
        tracing::warn!(reason = %failure, "gpu: a sondagem do driver Vulkan falhou; ASR na CPU");
    }
    let paths = DaemonPaths::from_env()?;
    // Antes do runtime: ainda há uma thread só, e as do tokio, PipeWire e
    // whisper herdam a restrição. Trocar o endpoint exige reiniciar.
    let network = fw_core::sandbox::restrict_network(&config.llm);
    if network.is_enforced() {
        tracing::info!("rede: {network}");
    } else {
        tracing::warn!("rede: {network}");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("falha ao iniciar o runtime tokio")?;
    runtime.block_on(async {
        let mut daemon = Daemon::bind(paths)?
            .with_network_sandbox(network)
            .with_vram(VramPolicy::from_config(&config.performance))
            .with_config(config_path, required, config.clone())
            .with_game_detector(Arc::new(SystemGameDetector::default()));
        // Histórico só de métricas; falha ao abrir não impede o ditado.
        match fw_core::history::default_path().map(|p| fw_core::history::History::open(&p)) {
            Some(Ok(history)) => daemon = daemon.with_history(history),
            Some(Err(e)) => tracing::warn!("{e}"),
            None => {}
        }
        let polisher = Polisher::new(&config);
        let pipeline = DictationPipeline::new(DictationConfig::from_config(&config), polisher)
            .context("falha ao conectar ao PipeWire")?;
        let desktop = HyprDesktop::new(config.inject.clone());
        if gpu.probe.failure().is_some() {
            desktop.reconfigure_language(config.ui.language);
            desktop.notify(gpu_probe::NOTICE);
        }
        daemon
            .run(Arc::new(pipeline), Arc::new(desktop), shutdown_signal()?)
            .await;
        Ok(())
    })
}

/// SIGTERM (systemd) ou SIGINT (Ctrl+C no terminal).
fn shutdown_signal() -> anyhow::Result<impl Future<Output = ()>> {
    let mut term = signal(SignalKind::terminate()).context("falha ao registrar SIGTERM")?;
    let mut int = signal(SignalKind::interrupt()).context("falha ao registrar SIGINT")?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    })
}
