//! `fastwisper audio`: lista os microfones do PipeWire e mostra qual a
//! config escolhe (ou por que nenhum), toca os sons de início/fim e testa o
//! microfone mostrando só o nível (nunca grava áudio em disco).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Subcommand;
use fw_core::capture::{Capture, select_source};
use fw_core::config::Config;
use fw_core::cues::{self, Cue};
use fw_core::daemon::DaemonPaths;
use fw_core::ipc::{MeterEvent, Request};
use fw_core::session::{Command, StateName};

use crate::ConfigArgs;

/// Cauda depois de cada som: o stream do PipeWire abre de forma assíncrona.
const CUE_MARGIN: Duration = Duration::from_millis(300);

#[derive(Debug, Subcommand)]
pub enum AudioCommand {
    /// Lista fontes de captura e a escolhida por `audio.device`.
    Sources {
        /// Uma lista JSON (nome do nó, descrição, se é a escolhida).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        config: ConfigArgs,
    },
    /// Toca os sons de início e fim no volume da config (ou no informado).
    TestSound {
        /// Volume de 0 a 1 (padrão: `audio.sound_volume`).
        #[arg(long)]
        volume: Option<f32>,
        #[command(flatten)]
        config: ConfigArgs,
    },
    /// Grava por alguns segundos pelo daemon, mostra o nível do microfone e
    /// cancela: nada é transcrito nem colado.
    Test {
        /// Duração do teste, em segundos (1 a 10).
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u64).range(1..=10))]
        secs: u64,
        /// Uma linha JSON por quadro (`{"level":0.42}`) e o pico no fim.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: AudioCommand) -> anyhow::Result<()> {
    match command {
        AudioCommand::Sources { json, config } => {
            sources(&Config::load(config.config.as_deref())?, json)
        }
        AudioCommand::TestSound { volume, config } => {
            let config = Config::load(config.config.as_deref())?;
            test_sound(volume.unwrap_or(config.audio.sound_volume))
        }
        AudioCommand::Test { secs, json } => test_mic(Duration::from_secs(secs), json),
    }
}

fn sources(config: &Config, json: bool) -> anyhow::Result<()> {
    let capture = Capture::spawn()?;
    let sources = capture.sources()?;
    let chosen = select_source(&sources, &config.audio.device);
    let chosen_id = chosen.as_ref().ok().map(|s| s.id);
    if json {
        // Só fontes de captura: saídas e monitores nunca são candidatas.
        let rows: Vec<serde_json::Value> = sources
            .iter()
            .filter(|s| fw_core::capture::SOURCE_CLASSES.contains(&s.media_class.as_str()))
            .map(|s| {
                serde_json::json!({
                    "node_name": s.node_name,
                    "description": s.description,
                    "media_class": s.media_class,
                    "chosen": Some(s.id) == chosen_id,
                })
            })
            .collect();
        crate::outln!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    for s in &sources {
        let mark = if Some(s.id) == chosen_id { "→" } else { " " };
        crate::outln!(
            "{mark} {:<28} {:<22} {}",
            s.description,
            s.media_class,
            s.node_name
        );
    }
    match chosen {
        Ok(s) => crate::human_print!(
            "audio.device = \"{}\" → {}",
            config.audio.device,
            s.description
        ),
        Err(e) => bail!("audio.device = \"{}\": {e}", config.audio.device),
    }
    Ok(())
}

fn test_sound(volume: f32) -> anyhow::Result<()> {
    if !(0.0..=1.0).contains(&volume) {
        bail!("volume {volume}: use de 0 a 1");
    }
    let capture = Capture::spawn()?;
    for cue in [Cue::Start, Cue::Stop] {
        capture.play(cues::render(cue, volume).into());
        std::thread::sleep(cues::duration(cue) + CUE_MARGIN);
    }
    crate::human_print!("Sons tocados (volume {volume}).");
    Ok(())
}

/// `press`, nível por `secs` e `cancel`. Cancelar descarta o áudio: nada é
/// transcrito, copiado ou colado.
fn test_mic(secs: Duration, json: bool) -> anyhow::Result<()> {
    let paths = DaemonPaths::from_env()?;
    let mut meter = UnixStream::connect(&paths.socket).context("o fastwisperd não está rodando")?;
    meter.write_all(&Request::new("audio-test-meter", Command::Meter).to_line())?;
    meter.set_read_timeout(Some(Duration::from_millis(200)))?;
    let press = fw_core::client::send(
        &paths.socket,
        &Request::new(crate::ctl::request_id(), Command::Press),
    )?;
    if let Some(error) = press.error {
        bail!("o daemon não começou a gravar: {}", error.message);
    }
    let session = press.session_id;
    // Cancela também quando quem lê a saída a fecha no meio.
    let (result, cancel) = crate::output::finally(
        || follow_levels(meter, secs, session, json),
        || {
            fw_core::client::send(
                &paths.socket,
                &Request::new(crate::ctl::request_id(), Command::Cancel),
            )
        },
    );
    let peak = result?;
    cancel.context("não foi possível cancelar a gravação de teste")?;
    if json {
        crate::outln!("{{\"peak\":{peak}}}");
    } else if peak < 0.05 {
        crate::human_print!("Pico {peak:.2}: o microfone quase não captou som (mudo ou longe?).");
    } else {
        crate::human_print!("Pico {peak:.2}: o microfone está captando.");
    }
    Ok(())
}

fn follow_levels(
    stream: UnixStream,
    secs: Duration,
    session: Option<fw_core::session::SessionId>,
    json: bool,
) -> anyhow::Result<f32> {
    let deadline = Instant::now() + secs;
    let mut reader = BufReader::new(stream);
    let mut peak: f32 = 0.0;
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => bail!("o daemon fechou o medidor"),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        }
        let Ok(event) = serde_json::from_str::<MeterEvent>(&line) else {
            continue;
        };
        // Só a gravação de teste conta; outra sessão não é deste teste.
        if event.state != StateName::Recording || (session.is_some() && event.session_id != session)
        {
            continue;
        }
        peak = peak.max(event.level);
        if json {
            crate::outln!("{{\"level\":{}}}", event.level);
        } else {
            let bars = (event.level * 30.0).round() as usize;
            crate::out!("\r  {:<30} {:.2}", "▮".repeat(bars), event.level);
        }
    }
    if !json {
        crate::outln!();
    }
    Ok(peak)
}
