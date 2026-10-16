//! `fastwisper doctor`: confere cada peça do ditado e diz como consertar.
//! Só lê: não muda config, não baixa nada, não fala com hosts remotos.
//! As checagens ficam em `fw_core::doctor` (o hub mostra as mesmas); aqui
//! só o microfone (PipeWire), a sondagem das GPUs (num processo filho) e a
//! impressão.

use std::process::ExitCode;

use clap::Args;
use fw_core::capture::{Capture, select_source};
use fw_core::config::Config;
use fw_core::doctor::{self, Check, Level, Report};
use fw_core::gpu_probe;

use crate::ConfigArgs;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[command(flatten)]
    config: ConfigArgs,
}

fn print(check: &Check) {
    let mark = match check.level {
        Level::Ok => '✓',
        Level::Warn => '!',
        Level::Fail => '✗',
    };
    crate::outln!(
        "{mark} {:<12} {}",
        crate::human(&check.item),
        crate::human(&check.detail)
    );
    if let Some(hint) = &check.hint {
        crate::outln!("  → {}", crate::human(hint));
    }
}

pub fn run(args: DoctorArgs) -> ExitCode {
    let mut r = Report::streaming(print);
    let config = doctor::check_config(&mut r, args.config.config.as_deref());
    let status = doctor::check_daemon(&mut r);
    doctor::check_network(&mut r, &config, status.as_ref());
    check_microphone(&mut r, &config);
    doctor::check_gpu(&mut r, &config, &gpu_probe::ensure(&config.asr.gpu).probe);
    doctor::check_model(&mut r, &config);
    doctor::check_llm(&mut r, &config);
    doctor::check_desktop(&mut r);

    crate::outln!();
    crate::outln!("{}", crate::human(&r.verdict()));
    if r.failures() == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(crate::EXIT_REJECTED)
    }
}

fn check_microphone(r: &mut Report, config: &Config) {
    if !doctor::check_microphone_configured(r, config) {
        return;
    }
    let sources = Capture::spawn().and_then(|c| c.sources());
    match sources {
        Ok(sources) => match select_source(&sources, &config.audio.device) {
            Ok(s) => r.ok("microfone", format!("{} [{}]", s.description, s.node_name)),
            Err(e) => r.fail(
                "microfone",
                e.to_string(),
                "conecte o microfone ou ajuste audio.device (fastwisper audio sources)",
            ),
        },
        Err(e) => r.fail(
            "microfone",
            e.to_string(),
            "o PipeWire está rodando? (systemctl --user status pipewire)",
        ),
    }
}
