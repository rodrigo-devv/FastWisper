//! Consulta o daemon e salva somente a escolha de perfil, preservando o TOML
//! (fluxo em `fw_core::vram_switch`, o mesmo do hub).
use anyhow::{Context, bail};
use clap::Args;
use fw_core::config::Config;
use fw_core::daemon::DaemonPaths;
use fw_core::ipc::Request;
use fw_core::session::Command;
use fw_core::vram::VramMode;
use fw_core::vram_switch::{ModeSwitch, switch_mode};

#[derive(Debug, Args)]
pub struct VramArgs {
    /// automatico, desempenho, sob_demanda ou jogo.
    #[arg(value_parser = parse_mode)]
    mode: Option<VramMode>,
    #[command(flatten)]
    config: crate::ConfigArgs,
}

fn parse_mode(name: &str) -> Result<VramMode, String> {
    VramMode::from_name(name).ok_or_else(|| {
        crate::human("perfil inválido: use automatico, desempenho, sob_demanda ou jogo")
    })
}

pub fn run(args: VramArgs) -> anyhow::Result<()> {
    let paths = DaemonPaths::from_env()?;
    let id = crate::ctl::request_id();
    let status = match args.mode {
        Some(mode) => {
            let path = args.config.config.map_or_else(Config::default_path, Ok)?;
            match switch_mode(&paths.socket, &path, mode, &id)? {
                ModeSwitch::Saved => {
                    crate::human_print!(
                        "Perfil salvo: {mode}. O daemon está parado; vale no próximo início."
                    );
                    return Ok(());
                }
                ModeSwitch::Applied { status, reload } => {
                    if let Some(notice) = reload.as_ref().and_then(crate::config::restart_notice) {
                        crate::human_print!("{notice}");
                    }
                    status
                }
            }
        }
        None => {
            let response =
                fw_core::client::send(&paths.socket, &Request::new(&id, Command::Status))?;
            if !response.ok {
                bail!(
                    "daemon recusou o perfil: {}",
                    response
                        .error
                        .map_or_else(|| "erro desconhecido".into(), |e| e.message)
                );
            }
            response
                .vram
                .context("daemon não informou o perfil de VRAM")?
        }
    };
    let memory = status.vram_mb.map_or_else(
        || "indisponível (não medida)".into(),
        |n| format!("{n} MiB"),
    );
    crate::human_print!(
        "Modo: {} · perfil ativo: {}\nVRAM usada pelo FastWisper: {memory}",
        status.mode,
        status.profile
    );
    Ok(())
}
