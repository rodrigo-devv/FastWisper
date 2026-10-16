//! `fastwisper status [--follow] [--bar]`: estado do daemon para humanos ou
//! para a barra do Omarchy (JSON estilo Waybar, uma linha por mudança).
//! Nunca imprime texto ditado: o daemon nem o manda no status.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use clap::Args;
use fw_core::config::{Config, UiLanguage};
use fw_core::daemon::DaemonPaths;
use fw_core::hotkeys::{Action, Hotkey};
use fw_core::ipc::{Request, Response};
use fw_core::pipeline::{OutcomeKind, Readiness};
use fw_core::session::{Command, StateName};
use fw_core::vram::{ModelState, Profile, VramMode, VramStatus};
use serde_json::json;

use crate::ctl;

const RECONNECT: Duration = Duration::from_secs(2);

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Continua aberto e imprime uma linha a cada mudança.
    #[arg(long)]
    follow: bool,
    /// JSON para a barra do Omarchy ({"text","class","tooltip"}).
    #[arg(long)]
    bar: bool,
    /// Traduz só a dica da barra; mantém o formato JSON de --bar.
    #[arg(long, requires = "bar")]
    localized: bool,
}

pub fn run(args: StatusArgs) -> ExitCode {
    if !args.follow {
        if !args.bar {
            return ctl::run(ctl::Action::Status, false);
        }
        let line = DaemonPaths::from_env()
            .ok()
            .and_then(|p| {
                fw_core::client::send(&p.socket, &Request::new("bar", Command::Status)).ok()
            })
            .map_or_else(
                || offline(args.localized),
                |r| render_bar(&r, args.localized),
            );
        crate::outln!("{line}");
        return ExitCode::SUCCESS;
    }
    let Ok(paths) = DaemonPaths::from_env() else {
        crate::human_error!("fastwisper: XDG_RUNTIME_DIR não definido");
        return ExitCode::from(crate::EXIT_UNREACHABLE);
    };
    let mut offline_shown = false;
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            offline_shown = false;
            let _ = follow(stream, args.bar, args.localized);
        }
        if !offline_shown {
            print_line(&if args.bar {
                offline(args.localized)
            } else {
                "state=offline".to_owned()
            });
            offline_shown = true;
        }
        crate::output::wait(RECONNECT);
    }
}

/// Uma assinatura: termina quando o daemon fecha a conexão.
fn follow(mut stream: UnixStream, bar: bool, localized: bool) -> std::io::Result<()> {
    stream.write_all(&Request::new("follow", Command::Subscribe).to_line())?;
    for line in crate::output::FollowLines::new(stream)? {
        let line = line?;
        let Ok(response) = serde_json::from_str::<Response>(&line) else {
            continue;
        };
        print_line(&if bar {
            render_bar(&response, localized)
        } else {
            human_line(&response)
        });
    }
    Ok(())
}

/// Pipe fechado (a barra saiu) encerra o comando e, com ele, a assinatura:
/// ver [`crate::output`].
fn print_line(line: &str) {
    crate::outln!("{line}");
}

fn human_line(r: &Response) -> String {
    let state = r.state.map_or("?".to_owned(), |s| s.to_string());
    let last = r.last.map(|l| l.outcome.as_str()).unwrap_or_default();
    format!(
        "state={state} last={last} seq={}",
        r.seq.unwrap_or_default()
    )
}

fn bar_language(localized: bool) -> UiLanguage {
    if localized {
        Config::load(None).unwrap_or_default().ui.language
    } else {
        UiLanguage::PtBr
    }
}
fn render_bar(response: &Response, localized: bool) -> String {
    let language = bar_language(localized);
    let dictate = dictate_label()
        .split(" + ")
        .map(|key| fw_core::i18n::translate(key, language))
        .collect::<Vec<_>>()
        .join(" + ");
    let line = bar_line_with_language(response, &dictate, language);
    if !localized {
        return line;
    }
    localized_line(response, line)
}
fn localized_line(response: &Response, line: String) -> String {
    let mut value: serde_json::Value = serde_json::from_str(&line).expect("JSON local da barra");
    let failure_key = if response.asr == Some(Readiness::Failed) {
        Some("asr_unavailable")
    } else {
        response.last.and_then(|last| match last.outcome {
            OutcomeKind::Failed(failure) => Some(failure.error_code().as_str()),
            OutcomeKind::Done(_) => None,
        })
    };
    value["failure_key"] = failure_key.into();
    value.to_string()
}
fn offline(localized: bool) -> String {
    let mut value: serde_json::Value = serde_json::from_str(&offline_bar()).expect("JSON local");
    value["tooltip"] = fw_core::i18n::translate(
        value["tooltip"].as_str().unwrap_or_default(),
        bar_language(localized),
    )
    .into();
    value.to_string()
}

fn offline_bar() -> String {
    json!({
        "text": "󰍭",
        "alt": "offline",
        "class": "offline",
        "tooltip": "FastWisper parado\nsystemctl --user start fastwisperd",
    })
    .to_string()
}

/// Atalho de ditar da config (`hotkey.dictate`), relido a cada linha: a
/// dica da barra acompanha a troca do atalho sem reiniciar o plugin.
/// Config ilegível ou inválida: o atalho padrão.
fn dictate_label() -> String {
    Config::load(None)
        .ok()
        .and_then(|c| c.hotkey.dictate.parse::<Hotkey>().ok())
        .or_else(|| Action::Dictate.default_text().parse().ok())
        .map_or_else(|| "o atalho de ditar".to_owned(), |h| h.label())
}

/// Ícone, classe e tooltip (pt-BR, só estado e métricas). `dictate`: o
/// atalho configurado, para a dica de quando está pronto.
#[cfg(test)]
fn bar_line(r: &Response, dictate: &str) -> String {
    bar_line_with_language(r, dictate, UiLanguage::PtBr)
}
fn bar_line_with_language(r: &Response, dictate: &str, language: UiLanguage) -> String {
    let tr = |text: &str| fw_core::i18n::translate(text, language);
    let failure = r.last.and_then(|l| match l.outcome {
        OutcomeKind::Failed(f) => Some(f.error_code()),
        OutcomeKind::Done(_) => None,
    });
    let (text, class, headline) = match (r.state, r.asr, failure) {
        (Some(StateName::Recording), _, _) => (
            "󰍬",
            "recording",
            tr("Gravando… (solte para transcrever, Esc cancela)"),
        ),
        (Some(StateName::Transcribing), _, _) => ("󰔟", "transcribing", tr("Transcrevendo…")),
        (_, Some(Readiness::Loading), _) => ("󰔟", "loading", tr("Carregando o modelo…")),
        (_, Some(Readiness::Failed), _) => (
            "󰍭",
            "error",
            tr("Modelo de transcrição indisponível (fastwisper doctor)"),
        ),
        (_, _, Some(code)) => (
            "󰍭",
            "error",
            tr(&format!(
                "Último ditado falhou: {}",
                code_message(code.as_str(), language)
            )),
        ),
        _ => (
            "󰍬",
            "idle",
            tr(&format!("Pronto: segure {dictate} para ditar")),
        ),
    };
    let mut tooltip = format!("FastWisper\n{headline}");
    if let Some(vram) = r.vram {
        tooltip.push_str(&format!("\n{}", vram_line(&vram, language)));
    }
    if let Some(last) = r.last
        && failure.is_none()
    {
        tooltip.push('\n');
        tooltip.push_str(&tr(&format!("Último: {}", summary(&last, language))));
    }
    let mut line = json!({ "text": text, "alt": class, "class": class, "tooltip": tooltip,
        "profile": r.vram.map(|v| v.profile), "vram_mode": r.vram.map(|v| v.mode)
    });
    // Contador de ditados que falharam (daemon novo): a pílula avisa quando
    // ele muda, não quando a linha se repete. Daemon antigo: campo ausente.
    if let Some(failures) = r.failures {
        line["failures"] = failures.into();
    }
    line.to_string()
}

/// "Perfil: sob demanda (automático) · modelos descarregados".
fn vram_line(vram: &VramStatus, language: UiLanguage) -> String {
    let tr = |text: &str| fw_core::i18n::translate(text, language);
    let profile = match vram.profile {
        Profile::Desempenho => "desempenho",
        Profile::SobDemanda => "sob demanda",
        Profile::Jogo => "jogo",
    };
    let profile = if language == UiLanguage::En {
        match vram.profile {
            Profile::Desempenho => "performance",
            Profile::SobDemanda => "on demand",
            Profile::Jogo => "gaming",
        }
    } else {
        profile
    };
    let mut line = tr(&format!("Perfil: {profile}"));
    if vram.mode == VramMode::Automatico {
        line.push_str(&tr(" (automático)"));
    }
    let loaded = [vram.asr, vram.llm].contains(&ModelState::Loaded);
    let loading = [vram.asr, vram.llm].contains(&ModelState::Loading);
    line.push_str(&tr(match (loading, loaded) {
        (true, _) => " · carregando modelos",
        (false, true) => " · modelos na VRAM",
        (false, false) => " · modelos descarregados",
    }));
    if let Some(mb) = vram.vram_mb.filter(|mb| *mb > 0) {
        line.push_str(&format!(" ({mb} MiB)"));
    }
    line
}

fn code_message(code: &str, language: UiLanguage) -> String {
    fw_core::i18n::translate(
        match code {
            "device_not_configured" => "nenhum microfone configurado",
            "device_unavailable" => "microfone não conectado",
            "device_ambiguous" => "mais de um microfone possível",
            "device_lost" => "o microfone desconectou",
            "recording_too_long" => "gravação passou do limite",
            "mic_silent" => "microfone mudo (só silêncio)",
            "asr_unavailable" => "modelo indisponível",
            _ => "erro interno",
        },
        language,
    )
}

fn summary(last: &fw_core::pipeline::LastSummary, language: UiLanguage) -> String {
    let tr = |text: &str| fw_core::i18n::translate(text, language);
    let m = &last.metrics;
    let mut parts = vec![match last.outcome {
        OutcomeKind::Done(fw_core::pipeline::DoneKind::Transcribed) => tr("transcrito"),
        OutcomeKind::Done(fw_core::pipeline::DoneKind::NoSpeech) => tr("sem fala"),
        OutcomeKind::Done(fw_core::pipeline::DoneKind::TooShort) => tr("curto demais"),
        OutcomeKind::Failed(_) => tr("falhou"),
    }];
    if let Some(lang) = last.language {
        parts.push(lang.to_string());
    }
    if let Some(words) = m.words {
        parts.push(tr(&format!("{words} palavras")));
    }
    if let Some(asr) = m.asr_ms {
        parts.push(format!("ASR {asr} ms"));
    }
    if let Some(llm) = m.llm_ms {
        parts.push(format!("LLM {llm} ms"));
    }
    if let Some(e2e) = m.e2e_ms {
        parts.push(format!("total {e2e} ms"));
    }
    if let Some(delivery) = last.delivery {
        let value = serde_json::to_value(delivery).unwrap_or_default();
        parts.push(match value["delivery"].as_str() {
            Some("pasted") => tr("colado"),
            Some("submitted") => tr("colado e enviado"),
            Some("copied") => tr("copiado"),
            _ => tr("não entregue"),
        });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_key_is_language_independent_and_absent_from_legacy_bar() {
        let mut response: Response =
            serde_json::from_str(r#"{"v":1,"request_id":null,"ok":true,"state":"idle"}"#)
                .expect("estado");
        response.last = Some(fw_core::pipeline::LastSummary {
            outcome: OutcomeKind::Failed(fw_core::pipeline::Failure::MicSilent),
            language: None,
            metrics: fw_core::pipeline::Metrics::default(),
            delivery: None,
        });
        for language in [UiLanguage::En, UiLanguage::PtBr] {
            let legacy = bar_line_with_language(&response, "Alt + Space", language);
            let raw: serde_json::Value = serde_json::from_str(&legacy).expect("JSON legado");
            assert!(raw.get("failure_key").is_none());
            let line: serde_json::Value =
                serde_json::from_str(&localized_line(&response, legacy)).expect("JSON traduzido");
            assert_eq!(line["failure_key"], "mic_silent");
            assert_eq!(line["class"], "error");
        }
    }

    #[test]
    fn failure_counter_passes_through_and_is_absent_for_old_daemons() {
        let old: Response =
            serde_json::from_str(r#"{"v":1,"request_id":null,"ok":true,"state":"idle"}"#)
                .expect("daemon antigo");
        let new: Response = serde_json::from_str(
            r#"{"v":1,"request_id":null,"ok":true,"state":"idle","failures":7}"#,
        )
        .expect("daemon novo");
        for localized in [false, true] {
            let render = |response: &Response| {
                let line = bar_line(response, "Alt + Space");
                let line = if localized {
                    localized_line(response, line)
                } else {
                    line
                };
                serde_json::from_str::<serde_json::Value>(&line).expect("JSON")
            };
            assert!(render(&old).get("failures").is_none());
            assert_eq!(render(&new)["failures"], 7);
        }
    }

    #[test]
    fn localized_bar_preserves_machine_fields_and_follow_line() {
        let response: Response = serde_json::from_str(
            r#"{"v":1,"request_id":null,"ok":true,"state":"recording","seq":12}"#,
        )
        .expect("estado válido");
        let en: serde_json::Value = serde_json::from_str(&bar_line_with_language(
            &response,
            "Alt + Space",
            UiLanguage::En,
        ))
        .expect("barra");
        let pt: serde_json::Value = serde_json::from_str(&bar_line_with_language(
            &response,
            "Alt + Espaço",
            UiLanguage::PtBr,
        ))
        .expect("barra");
        assert_ne!(en["tooltip"], pt["tooltip"]);
        for key in ["text", "alt", "class", "profile", "vram_mode"] {
            assert_eq!(en[key], pt[key]);
        }
        assert_eq!(human_line(&response), "state=recording last= seq=12");
        assert_eq!(
            en["tooltip"],
            "FastWisper\nRecording… (release to transcribe, Esc cancels)"
        );
    }

    #[test]
    fn english_bar_translates_the_last_dictation_summary() {
        let mut response: Response =
            serde_json::from_str(r#"{"v":1,"request_id":null,"ok":true,"state":"idle"}"#)
                .expect("estado");
        response.last = Some(fw_core::pipeline::LastSummary {
            outcome: OutcomeKind::Done(fw_core::pipeline::DoneKind::Transcribed),
            language: None,
            metrics: fw_core::pipeline::Metrics {
                words: Some(3),
                ..Default::default()
            },
            delivery: None,
        });
        let en: serde_json::Value = serde_json::from_str(&bar_line_with_language(
            &response,
            "Alt + Space",
            UiLanguage::En,
        ))
        .expect("barra");
        let tooltip = en["tooltip"].as_str().expect("tooltip");
        assert!(
            tooltip.ends_with("\nLast: transcribed · 3 words"),
            "{tooltip}"
        );
        let pt: serde_json::Value =
            serde_json::from_str(&bar_line(&response, "Alt + Espaço")).expect("barra");
        let tooltip = pt["tooltip"].as_str().expect("tooltip");
        assert!(
            tooltip.ends_with("\nÚltimo: transcrito · 3 palavras"),
            "{tooltip}"
        );
    }

    #[test]
    fn idle_bar_tooltip_shows_the_configured_dictate_hotkey() {
        let idle: Response =
            serde_json::from_str(r#"{"v":1,"request_id":null,"ok":true,"state":"idle"}"#)
                .expect("resposta");
        let label = "SUPER+D".parse::<Hotkey>().expect("atalho").label();
        let line: serde_json::Value = serde_json::from_str(&bar_line(&idle, &label)).expect("JSON");
        let tooltip = line["tooltip"].as_str().expect("tooltip");
        assert!(
            tooltip.starts_with("FastWisper\nPronto: segure Super + D para ditar"),
            "{tooltip}"
        );
        assert!(!tooltip.contains("Alt"), "{tooltip}");
    }
}
