//! `fastwisper history`: resumo do histórico de métricas (sem texto).

use anyhow::Context;
use clap::{Args, Subcommand};
use fw_core::history::{self, History, Percentiles};
use std::io::{self, IsTerminal};

#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Janela em dias.
    #[arg(long, default_value_t = 7)]
    days: u32,
    #[command(subcommand)]
    command: Option<HistoryCommand>,
}

#[derive(Debug, Subcommand)]
enum HistoryCommand {
    /// Apaga todas as métricas de uso. Seguro com o daemon rodando.
    Clear {
        /// Confirma sem perguntar (necessário em scripts).
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(args: HistoryArgs) -> anyhow::Result<()> {
    let path = history::default_path().context("sem HOME")?;
    if let Some(HistoryCommand::Clear { yes }) = args.command {
        if !yes {
            anyhow::ensure!(
                io::stdin().is_terminal(),
                "para apagar em scripts, use fastwisper history clear --yes"
            );
            crate::out!(
                "{}",
                crate::human(
                    "Apagar todo o histórico de uso? Esta ação não pode ser desfeita. [s/N] "
                )
            );
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(
                answer.trim().to_lowercase().as_str(),
                "s" | "sim" | "y" | "yes"
            ) {
                crate::human_print!("Histórico mantido.");
                return Ok(());
            }
        }
        let deleted = history::clear(&path)?;
        crate::human_print!("Histórico apagado: {deleted} registros removidos.");
        return Ok(());
    }
    if !path.exists() {
        crate::human_print!("sem histórico ainda ({})", path.display());
        return Ok(());
    }
    let stats = History::open(&path)?.stats(args.days)?;
    crate::human_print!("últimos {} dias · {}", args.days, path.display());
    let total: i64 = stats.outcomes.iter().map(|(_, n)| n).sum();
    crate::human_print!("ditados   {total}");
    for (outcome, n) in &stats.outcomes {
        crate::outln!("  {outcome:<22} {n}");
    }
    if !stats.deliveries.is_empty() {
        crate::human_print!("entrega");
        for (delivery, n) in &stats.deliveries {
            crate::outln!("  {delivery:<22} {n}");
        }
    }
    if stats.profiles.iter().any(|(profile, _)| profile != "-") {
        crate::human_print!("perfil de VRAM");
        for (profile, n) in &stats.profiles {
            crate::outln!("  {profile:<22} {n}");
        }
    }
    for (name, p) in [
        ("asr_ms", stats.asr_ms),
        ("llm_ms", stats.llm_ms),
        ("e2e_ms", stats.e2e_ms),
        ("load_ms", stats.load_ms),
        ("llm_load", stats.llm_load_ms),
    ] {
        print_percentiles(name, p);
    }
    Ok(())
}

fn print_percentiles(name: &str, p: Percentiles) {
    if p.count == 0 {
        return;
    }
    crate::human_print!(
        "{name:<9} p50 {} · p95 {} · máx {} ({} amostras)",
        p.p50,
        p.p95,
        p.max,
        p.count
    );
}
