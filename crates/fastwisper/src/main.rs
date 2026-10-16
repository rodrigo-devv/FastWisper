//! `fastwisper`: CLI do FastWisper. `ctl` fala com o daemon; `models`,
//! `transcribe` e `bench` rodam no próprio processo.

// Toda saída passa por `output` (pipe fechado não vira pânico).
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod asr;
mod audio;
mod config;
mod ctl;
mod doctor;
mod history;
mod hotkeys;
mod hub;
mod meter;
mod models;
mod notes;
mod ollama;
mod output;
mod restart;
mod setup;
mod status;
mod vram;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// Resposta de erro do daemon (busy, comando inválido...) ou falha do comando.
pub const EXIT_REJECTED: u8 = 1;
/// Daemon fora do ar ou falha de comunicação.
pub const EXIT_UNREACHABLE: u8 = 3;

#[derive(Debug, Parser)]
#[command(version, about = "Ditado por voz 100% local")]
struct Cli {
    #[command(subcommand)]
    command: TopLevel,
}

#[derive(Debug, Subcommand)]
enum TopLevel {
    /// Abre ou foca o hub local, opcionalmente numa página (ex.: microfone).
    Hub {
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(hub::PAGES))]
        page: Option<String>,
    },
    /// Envia um comando de sessão ao fastwisperd.
    Ctl {
        #[arg(value_parser = localized_enum::<ctl::Action>())]
        action: ctl::Action,
        /// Imprime a resposta JSON do daemon.
        #[arg(long)]
        json: bool,
    },
    /// Lista, baixa (com SHA-256 fixado) e verifica modelos.
    Models {
        #[command(subcommand)]
        command: models::ModelsCommand,
    },
    /// Transcreve um WAV e imprime o texto.
    Transcribe(asr::TranscribeArgs),
    /// Mede carga, latência e RTF do ASR num WAV (não imprime o texto).
    Bench(asr::BenchArgs),
    /// Confere cada peça (microfone, GPU, modelo, LLM, atalhos) e diz como consertar.
    Doctor(doctor::DoctorArgs),
    /// Lista GPUs Vulkan disponíveis, sem carregar modelos.
    Gpus {
        #[arg(long)]
        json: bool,
    },
    /// Métricas dos ditados (latência, entrega, falhas), sem texto.
    History(history::HistoryArgs),
    /// Notas por voz locais (opt-in), com listagem e limpeza explícitas.
    Notes(notes::NotesArgs),
    /// Estado do daemon; `--follow --bar` alimenta a barra do Omarchy.
    Status(status::StatusArgs),
    /// Nível do microfone enquanto grava (JSON por linha, ~25 Hz) para a pílula.
    Meter(meter::MeterArgs),
    /// Consulta a VRAM ou aplica e salva um perfil em tempo real.
    Vram(vram::VramArgs),
    /// Lê e muda a config (preservando comentários); o daemon aplica na hora o que dá.
    Config {
        #[command(subcommand)]
        command: config::ConfigCommand,
    },
    /// Atalhos do Hyprland: lista, confere um novo e aplica a config ao Lua.
    Hotkeys {
        #[command(subcommand)]
        command: hotkeys::HotkeysCommand,
    },
    /// Nuvem do Ollama local: mostra, liga (online) ou desliga (offline, padrão).
    Ollama(ollama::OllamaArgs),
    /// Reinicia o daemon (serviço do usuário) para valerem as chaves lidas só no início.
    Restart,
    /// Configura esta máquina: microfone padrão, modelo pela GPU e config (downloads separados).
    Setup(setup::SetupArgs),
    /// Microfones vistos pelo PipeWire.
    Audio {
        #[command(subcommand)]
        command: audio::AudioCommand,
    },
}

/// Opções comuns de config.
#[derive(Debug, Clone, clap::Args)]
pub struct ConfigArgs {
    /// Config alternativa (padrão: ~/.config/fastwisper/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
}

static LANGUAGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn human(text: &str) -> String {
    let language = if LANGUAGE.load(std::sync::atomic::Ordering::Relaxed) {
        fw_core::config::UiLanguage::PtBr
    } else {
        fw_core::config::UiLanguage::En
    };
    fw_core::i18n::translate(text, language)
}
macro_rules! human_print {
    ($($args:tt)*) => { crate::outln!("{}", crate::human(&format!($($args)*))) };
}
macro_rules! human_error {
    () => { crate::eoutln!() };
    ($($args:tt)*) => { crate::eoutln!("{}", crate::human(&format!($($args)*))) };
}
pub(crate) use output::{eout, eoutln, out, outln};
pub(crate) use {human_error, human_print};

/// O clap tira o ponto final do resumo dos doc comments; o catálogo guarda a
/// frase inteira.
fn human_help(text: &str) -> String {
    let translated = human(text);
    if translated != text || text.ends_with('.') {
        return translated;
    }
    let with_period = format!("{text}.");
    let translated = human(&with_period);
    if translated == with_period {
        return text.to_owned();
    }
    translated
        .strip_suffix('.')
        .map_or(translated.clone(), ToOwned::to_owned)
}

/// Valores de um `ValueEnum` com a ajuda de cada um no idioma da interface.
/// O derive do clap fixa o doc comment de cada variante e
/// [`localize_command`] não alcança os valores possíveis; o texto é
/// gerado aqui, na construção do `Command` (o idioma já foi lido).
pub fn localized_enum<E>() -> impl clap::builder::TypedValueParser<Value = E>
where
    E: clap::ValueEnum + Clone + Send + Sync + 'static,
{
    use clap::builder::TypedValueParser;
    let values: Vec<clap::builder::PossibleValue> = E::value_variants()
        .iter()
        .filter_map(clap::ValueEnum::to_possible_value)
        .map(|value| match value.get_help().map(ToString::to_string) {
            Some(help) => value.help(human_help(&help)),
            None => value,
        })
        .collect();
    clap::builder::PossibleValuesParser::new(values)
        .map(|name: String| E::from_str(&name, false).expect("valor aceito pelo parser"))
}

fn localize_command(mut command: clap::Command) -> clap::Command {
    if let Some(text) = command.get_about().map(ToString::to_string) {
        command = command.about(human_help(&text));
    }
    if let Some(text) = command.get_long_about().map(ToString::to_string) {
        command = command.long_about(human_help(&text));
    }
    let ids: Vec<_> = command
        .get_arguments()
        .map(|arg| arg.get_id().clone())
        .collect();
    for id in ids {
        command = command.mut_arg(id, |mut arg| {
            if let Some(text) = arg.get_help().map(ToString::to_string) {
                arg = arg.help(human_help(&text));
            }
            if let Some(text) = arg.get_long_help().map(ToString::to_string) {
                arg = arg.long_help(human_help(&text));
            }
            // Nomes de valor escritos à mão (`<CHAVE> <VALOR>`). O clap só
            // aceita `&'static str` aqui (sem a feature `string`): poucos
            // bytes, uma vez por execução.
            if let Some(names) = arg.get_value_names() {
                let names: Vec<&'static str> = names
                    .iter()
                    .map(|n| &*Box::leak(human(n.as_str()).into_boxed_str()))
                    .collect();
                arg = arg.value_names(names);
            }
            arg
        });
    }
    for subcommand in command.get_subcommands_mut() {
        *subcommand = localize_command(subcommand.clone());
    }
    command
}

// O `fmt` do tracing ignora falha de escrita no stderr.
#[allow(clippy::disallowed_methods)]
fn main() -> ExitCode {
    // Filho da sondagem da GPU (`fw_core::gpu_probe`): abre o Vulkan, lista
    // as GPUs e sai, antes do clap e da config.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == fw_core::gpu_probe::CHILD_ARG)
    {
        return fw_core::gpu_probe::child_main();
    }
    fw_core::gpu_probe::enable();
    // Logs do whisper.cpp ficam calados por padrão; FASTWISPER_LOG=whisper_rs=info mostra.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("FASTWISPER_LOG")
                .unwrap_or_else(|_| EnvFilter::new("warn,whisper_rs=off")),
        )
        .with_writer(std::io::stderr)
        .init();

    let arguments: Vec<_> = std::env::args_os().collect();
    let path = arguments
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .map(|pair| PathBuf::from(&pair[1]))
        .or_else(|| {
            arguments.iter().find_map(|argument| {
                argument
                    .to_str()?
                    .strip_prefix("--config=")
                    .map(PathBuf::from)
            })
        });
    let language = fw_core::config::Config::load(path.as_deref())
        .unwrap_or_default()
        .ui
        .language;
    LANGUAGE.store(
        language == fw_core::config::UiLanguage::PtBr,
        std::sync::atomic::Ordering::Relaxed,
    );
    // `--help` e erros de uso: o clap já ignora pipe fechado ao imprimir.
    let matches = localize_command(Cli::command()).get_matches();
    let command = Cli::from_arg_matches(&matches)
        .unwrap_or_else(|error| error.exit())
        .command;
    output::run(|| run(command))
}

fn run(command: TopLevel) -> ExitCode {
    let result = match command {
        TopLevel::Ctl { action, json } => return ctl::run(action, json),
        TopLevel::Status(args) => return status::run(args),
        TopLevel::Meter(args) => return meter::run(args),
        TopLevel::Doctor(args) => return doctor::run(args),
        TopLevel::Models { command } => models::run(command),
        TopLevel::Transcribe(args) => asr::transcribe(args),
        TopLevel::Bench(args) => asr::bench(args),
        TopLevel::Audio { command } => audio::run(command),
        TopLevel::Setup(args) => setup::run(args),
        TopLevel::History(args) => history::run(args),
        TopLevel::Notes(args) => notes::run(args),
        TopLevel::Gpus { json } => {
            // O Vulkan abre só no filho da sondagem; driver quebrado é lista vazia.
            let probe = fw_core::gpu_probe::ensure("auto").probe;
            if let Some(failure) = probe.failure()
                && !json
            {
                human_error!("{}", fw_core::gpu_probe::broken_detail(failure));
            }
            let names = probe.gpus();
            if json {
                serde_json::to_string(&names)
                    .map(|text| outln!("{text}"))
                    .map_err(Into::into)
            } else {
                for name in names {
                    outln!("{name}");
                }
                Ok(())
            }
        }
        TopLevel::Hub { page } => hub::run(page),
        TopLevel::Vram(args) => vram::run(args),
        TopLevel::Config { command } => config::run(command),
        TopLevel::Hotkeys { command } => hotkeys::run(command),
        TopLevel::Ollama(args) => ollama::run(args),
        TopLevel::Restart => restart::run(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let message = e
                .chain()
                .map(|cause| human(&cause.to_string()))
                .collect::<Vec<_>>()
                .join(": ");
            human_error!("fastwisper: {message}");
            ExitCode::from(EXIT_REJECTED)
        }
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn every_hub_action_is_accepted_by_the_cli() {
        let fields: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../ui/src/lib/settings/fields.json"))
                .expect("catálogo");
        let actions: Vec<_> = fields
            .iter()
            .filter(|field| field["kind"] == "action")
            .collect();
        assert!(!actions.is_empty());
        for action in actions {
            let mut args = vec!["fastwisper"];
            args.extend(
                action["cli"]
                    .as_str()
                    .expect("CLI da ação")
                    .split_whitespace(),
            );
            args.push("--yes");
            Cli::try_parse_from(args).expect("ação do hub existe na CLI");
        }
    }
}
