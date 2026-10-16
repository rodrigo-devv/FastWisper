//! `fastwisper ctl …`: comandos de sessão enviados ao daemon. Os binds do
//! Hyprland chamam `ctl press|release|toggle|cancel`.

use std::process::{self, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use fw_core::client::{self, ClientError};
use fw_core::daemon::DaemonPaths;
use fw_core::inject::Delivery;
use fw_core::ipc::{Request, Response};
use fw_core::pipeline::Readiness;
use fw_core::session::Command;
use fw_core::vram::ModelState;

use crate::{EXIT_REJECTED, EXIT_UNREACHABLE};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Action {
    Status,
    Press,
    Release,
    Toggle,
    Cancel,
    /// Imprime o último texto (fica 2 minutos em RAM).
    ShowLast,
    /// Copia o último texto para o clipboard, sem colar.
    CopyLast,
    /// Cola o último texto na janela com foco agora (nunca envia Enter).
    PasteLast,
    /// Libera a VRAM agora (espera o ditado em andamento terminar).
    Unload,
}

impl From<Action> for Command {
    fn from(action: Action) -> Self {
        match action {
            Action::Status => Command::Status,
            Action::Press => Command::Press,
            Action::Release => Command::Release,
            Action::Toggle => Command::Toggle,
            Action::Cancel => Command::Cancel,
            Action::ShowLast => Command::ShowLast,
            Action::CopyLast => Command::CopyLast,
            Action::PasteLast => Command::PasteLast,
            Action::Unload => Command::Unload,
        }
    }
}

pub fn run(action: Action, json: bool) -> ExitCode {
    let paths = match DaemonPaths::from_env() {
        Ok(paths) => paths,
        Err(e) => {
            crate::human_error!("fastwisper: {e}");
            return ExitCode::from(EXIT_UNREACHABLE);
        }
    };
    let request = Request::new(request_id(), action.into());
    let response = match client::send(&paths.socket, &request) {
        Ok(response) => response,
        Err(e @ ClientError::NotRunning(_)) => {
            crate::human_error!(
                "fastwisper: {}. {}",
                crate::human(&e.to_string()),
                crate::human("Inicie com `systemctl --user start fastwisperd` ou `fastwisperd`.")
            );
            return ExitCode::from(EXIT_UNREACHABLE);
        }
        Err(e) => {
            crate::human_error!("fastwisper: {e}");
            return ExitCode::from(EXIT_UNREACHABLE);
        }
    };

    if json {
        crate::out!("{}", String::from_utf8_lossy(&response.to_line()));
    } else if let Some(text) = &response.text {
        // show-last: só o texto no stdout, para poder encadear.
        crate::outln!("{text}");
    } else {
        print_human(&response);
    }
    if response.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_REJECTED)
    }
}

fn print_human(response: &Response) {
    let mut fields = Vec::new();
    if let Some(state) = response.state {
        fields.push(format!("state={state}"));
    }
    if let Some(mode) = response.mode {
        fields.push(format!("mode={mode}"));
    }
    if let Some(session) = response.session_id {
        fields.push(format!("session={session}"));
    }
    if let Some(generation) = response.generation {
        fields.push(format!("generation={generation}"));
    }
    if let Some(asr) = response.asr {
        let asr = match asr {
            Readiness::Loading => "loading",
            Readiness::Ready => "ready",
            Readiness::Failed => "failed",
        };
        fields.push(format!("asr={asr}"));
    }
    if let Some(last) = response.last {
        fields.push(format!("last={}", last.outcome.as_str()));
        if let Some(lang) = last.language {
            fields.push(format!("lang={lang}"));
        }
        let m = last.metrics;
        fields.push(format!("audio_ms={}", m.audio_ms));
        if let Some(delivery) = last.delivery {
            fields.push(delivery_field(delivery));
        }
        if let Some(llm) = m.llm {
            let llm = serde_json::to_value(llm).unwrap_or_default();
            fields.push(format!("llm={}", llm.as_str().unwrap_or("?")));
        }
        for (name, value) in [
            ("capture_start_ms", m.capture_start_ms),
            ("speech_ms", m.speech_ms),
            ("load_ms", m.load_ms),
            ("asr_ms", m.asr_ms),
            ("llm_load_ms", m.llm_load_ms),
            ("llm_ms", m.llm_ms),
            ("e2e_ms", m.e2e_ms),
            ("words", m.words),
        ] {
            if let Some(v) = value {
                fields.push(format!("{name}={v}"));
            }
        }
    }
    if let Some(delivery) = response.delivery {
        fields.push(delivery_field(delivery));
    }
    if let Some(vram) = response.vram {
        fields.push(format!("vram={}", vram.mode));
        fields.push(format!("profile={}", vram.profile));
        if vram.game {
            fields.push("game=yes".to_owned());
        }
        fields.push(format!("asr_model={}", model_state(vram.asr)));
        fields.push(format!("llm_model={}", model_state(vram.llm)));
        if let Some(mb) = vram.vram_mb {
            fields.push(format!("vram_mb={mb}"));
        }
        if let Some(secs) = vram.unload_in_secs {
            fields.push(format!("unload_in={secs}s"));
        }
        if vram.unload_pending {
            fields.push("unload=pending".to_owned());
        }
    }
    if !fields.is_empty() {
        crate::outln!("{}", fields.join(" "));
    }
    if let Some(error) = &response.error {
        crate::human_error!(
            "fastwisper: {} ({})",
            crate::human(&error.message),
            error.code
        );
    }
}

fn model_state(state: ModelState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn delivery_field(delivery: Delivery) -> String {
    let value = serde_json::to_value(delivery).unwrap_or_default();
    let kind = value["delivery"].as_str().unwrap_or("?");
    match value["reason"].as_str() {
        Some(reason) => format!("delivery={kind}:{reason}"),
        None => format!("delivery={kind}"),
    }
}

pub(crate) fn request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("cli-{}-{nanos}", process::id())
}
