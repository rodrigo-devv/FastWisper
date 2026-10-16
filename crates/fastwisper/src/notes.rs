//! Acesso explícito às notas locais; texto só em stdout, nunca em tracing.
use anyhow::Context;
use clap::{Args, Subcommand};
use fw_core::notes::{self, Notes};
use std::io::{self, IsTerminal};

#[derive(Debug, Args)]
pub struct NotesArgs {
    #[command(subcommand)]
    command: NotesCommand,
}
#[derive(Debug, Subcommand)]
enum NotesCommand {
    /// Lista as notas mais recentes primeiro (texto apenas no terminal).
    List,
    /// Apaga todas as notas locais; não modifica o histórico de métricas.
    Clear {
        #[arg(long)]
        yes: bool,
    },
}
pub fn run(args: NotesArgs) -> anyhow::Result<()> {
    let store = Notes::new(notes::default_path().context("sem HOME")?);
    match args.command {
        NotesCommand::List => {
            for note in store.list()? {
                crate::outln!(
                    "{} · {} · {} {} · {}\n{}\n",
                    note.id,
                    note.ts,
                    note.words,
                    crate::human("palavras"),
                    note.language,
                    note.text
                );
            }
        }
        NotesCommand::Clear { yes } => {
            if !yes {
                anyhow::ensure!(
                    io::stdin().is_terminal(),
                    "para apagar em scripts, use fastwisper notes clear --yes"
                );
                crate::out!(
                    "{}",
                    crate::human("Apagar todas as notas? Esta ação não pode ser desfeita. [s/N] ")
                );
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !matches!(
                    answer.trim().to_lowercase().as_str(),
                    "s" | "sim" | "y" | "yes"
                ) {
                    crate::human_print!("Notas mantidas.");
                    return Ok(());
                }
            }
            crate::human_print!("Notas apagadas: {} registros removidos.", store.clear()?);
        }
    }
    Ok(())
}
