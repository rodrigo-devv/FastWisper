//! Captura do microfone pelo PipeWire.
//!
//! Seleção estrita: só nós de captura (`Audio/Source`, `Audio/Source/Virtual`),
//! nunca saídas ou monitores, e zero/várias correspondências é erro. O stream
//! pede `node.dont-fallback`/`dont-reconnect`: se o microfone sumir, a
//! gravação falha em vez de ir para outro dispositivo.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use thiserror::Error;

use crate::audio::{self, AudioError, Pcm16k};
use crate::pipeline::Failure;

/// Classes de nó aceitas como microfone.
pub const SOURCE_CLASSES: [&str; 2] = ["Audio/Source", "Audio/Source/Virtual"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: u32,
    pub node_name: String,
    pub description: String,
    pub media_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptureError {
    #[error("nenhum microfone corresponde a \"{query}\" (disponíveis: {available:?})")]
    Unavailable {
        query: String,
        available: Vec<String>,
    },
    #[error("mais de um microfone corresponde a \"{query}\": {matches:?}")]
    Ambiguous { query: String, matches: Vec<String> },
    #[error("o microfone desconectou durante a gravação")]
    Lost,
    #[error("a gravação passou do limite e foi descartada")]
    TooLong,
    #[error("nenhuma gravação desta sessão em andamento")]
    NotRecording,
    #[error("falha no PipeWire: {0}")]
    PipeWire(String),
}

impl CaptureError {
    pub fn failure(&self) -> Failure {
        match self {
            CaptureError::Unavailable { .. } => Failure::DeviceUnavailable,
            CaptureError::Ambiguous { .. } => Failure::DeviceAmbiguous,
            CaptureError::Lost => Failure::DeviceLost,
            CaptureError::TooLong => Failure::RecordingTooLong,
            CaptureError::NotRecording | CaptureError::PipeWire(_) => Failure::CaptureFailed,
        }
    }
}

/// Escolhe o microfone. Nome exato do nó vence; senão, substring sem
/// diferenciar maiúsculas no nome ou na descrição. Saídas e monitores nunca
/// são candidatos, mesmo que o nome case ("USB Headset Analog Stereo").
pub fn select_source<'a>(
    sources: &'a [SourceInfo],
    query: &str,
) -> Result<&'a SourceInfo, CaptureError> {
    let candidates: Vec<&SourceInfo> = sources
        .iter()
        .filter(|s| SOURCE_CLASSES.contains(&s.media_class.as_str()))
        .collect();
    if let Some(exact) = candidates.iter().find(|s| s.node_name == query) {
        return Ok(exact);
    }
    let needle = query.trim().to_lowercase();
    let matches: Vec<&SourceInfo> = candidates
        .iter()
        .copied()
        .filter(|s| {
            !needle.is_empty()
                && (s.node_name.to_lowercase().contains(&needle)
                    || s.description.to_lowercase().contains(&needle))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(CaptureError::Unavailable {
            query: query.to_owned(),
            available: candidates.iter().map(|s| s.description.clone()).collect(),
        }),
        many => Err(CaptureError::Ambiguous {
            query: query.to_owned(),
            matches: many.iter().map(|s| s.description.clone()).collect(),
        }),
    }
}

/// Áudio capturado, já no formato pedido ao PipeWire (mono 16 kHz f32).
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u32,
    /// Do pedido de início até o primeiro buffer.
    pub first_buffer: Option<Duration>,
}

impl Recording {
    pub fn into_pcm(self) -> Result<Pcm16k, AudioError> {
        if self.rate == audio::SAMPLE_RATE && self.channels == 1 {
            if self.samples.iter().any(|s| !s.is_finite()) {
                return Err(AudioError::Unsupported("amostras NaN ou infinitas".into()));
            }
            return Ok(Pcm16k::new(self.samples));
        }
        audio::to_mono_16k(&self.samples, self.channels as usize, self.rate)
    }
}

/// Piso da escala do medidor: abaixo disso, nível 0.
pub const METER_FLOOR_DBFS: f32 = -60.0;

/// Nível do microfone em escala perceptual: dBFS do RMS de −60 a 0
/// mapeado linearmente para 0..1; abaixo de −60 dBFS (e silêncio
/// digital), 0.
pub fn level_from_rms(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    let dbfs = 20.0 * rms.log10();
    ((dbfs - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0)
}

/// RMS do último buffer do microfone, escrito pelo callback do PipeWire
/// (bits de um `f32` num atômico: sem lock nem alocação) e lido pelo
/// stream `meter` do daemon. É só um número, nunca áudio; volta a 0 quando
/// a gravação fecha.
#[derive(Debug, Default)]
pub struct MicLevel(AtomicU32);

impl MicLevel {
    pub fn set_rms(&self, rms: f32) {
        self.0.store(rms.to_bits(), Ordering::Relaxed);
    }

    pub fn rms(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    /// Nível perceptual (0..1), ver [`level_from_rms`].
    pub fn level(&self) -> f32 {
        level_from_rms(self.rms())
    }
}

#[cfg(feature = "pipewire")]
mod pw_backend;
#[cfg(feature = "pipewire")]
pub use pw_backend::Capture;

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: u32, name: &str, desc: &str, class: &str) -> SourceInfo {
        SourceInfo {
            id,
            node_name: name.into(),
            description: desc.into(),
            media_class: class.into(),
        }
    }

    /// O que o PipeWire mostra numa máquina com headset USB (nomes sintéticos).
    fn omarchy() -> Vec<SourceInfo> {
        vec![
            src(
                34,
                "alsa_output.usb-USB_Headset-00.analog-stereo",
                "USB Headset Analog Stereo",
                "Audio/Sink",
            ),
            src(
                47,
                "alsa_input.usb-USB_Headset-00.mono-fallback",
                "USB Headset Mono",
                "Audio/Source",
            ),
            src(
                58,
                "alsa_output.pci-0000_01_00.1.hdmi-stereo",
                "HDMI Audio",
                "Audio/Sink",
            ),
        ]
    }

    #[test]
    fn headset_mic_is_chosen_and_its_output_is_ignored() {
        let sources = omarchy();
        let chosen = select_source(&sources, "Headset").expect("Headset");
        assert_eq!(chosen.id, 47);
        assert_eq!(
            select_source(&sources, "hEaDsEt").expect("sem caixa").id,
            47
        );
    }

    #[test]
    fn exact_node_name_wins_over_substring() {
        let mut sources = omarchy();
        sources.push(src(
            90,
            "Headset-noise-filter",
            "Headset filtrado",
            "Audio/Source/Virtual",
        ));
        assert!(matches!(
            select_source(&sources, "Headset"),
            Err(CaptureError::Ambiguous { .. })
        ));
        let exact =
            select_source(&sources, "alsa_input.usb-USB_Headset-00.mono-fallback").expect("exato");
        assert_eq!(exact.id, 47);
    }

    #[test]
    fn missing_mic_never_falls_back() {
        let mut sources = omarchy();
        sources.retain(|s| s.id != 47);
        sources.push(src(
            70,
            "alsa_input.pci-onboard",
            "Onboard Analog",
            "Audio/Source",
        ));
        match select_source(&sources, "Headset") {
            Err(CaptureError::Unavailable { available, .. }) => {
                assert_eq!(
                    available,
                    ["Onboard Analog"],
                    "só fontes de captura são listadas"
                );
            }
            other => panic!("esperava Unavailable, veio {other:?}"),
        }
    }

    #[test]
    fn streams_and_empty_queries_are_not_matches() {
        let mut sources = omarchy();
        sources.push(src(
            80,
            "Headset recorder",
            "Headset recorder",
            "Stream/Input/Audio",
        ));
        assert_eq!(select_source(&sources, "Headset").expect("Headset").id, 47);
        assert!(select_source(&sources, "  ").is_err());
    }

    #[test]
    fn meter_maps_dbfs_linearly_from_minus_60_to_0() {
        assert_eq!(level_from_rms(0.0), 0.0, "silêncio digital");
        assert_eq!(level_from_rms(f32::NAN), 0.0);
        assert_eq!(level_from_rms(-1.0), 0.0);
        assert_eq!(level_from_rms(0.0005), 0.0, "abaixo de −60 dBFS");
        assert_eq!(level_from_rms(1.0), 1.0);
        assert_eq!(level_from_rms(2.0), 1.0, "acima de 0 dBFS satura");
        // −30 dBFS fica no meio; −6 dBFS em 0,9.
        assert!((level_from_rms(10f32.powf(-1.5)) - 0.5).abs() < 1e-4);
        assert!((level_from_rms(10f32.powf(-0.3)) - 0.9).abs() < 1e-4);
    }

    #[test]
    fn mic_level_round_trips_through_the_atomic() {
        let level = MicLevel::default();
        assert_eq!(level.level(), 0.0, "começa em silêncio");
        level.set_rms(0.1);
        assert_eq!(level.rms(), 0.1);
        assert!((level.level() - 2.0 / 3.0).abs() < 1e-4, "−20 dBFS");
        level.set_rms(0.0);
        assert_eq!(level.level(), 0.0);
    }

    #[test]
    fn errors_map_to_typed_failures() {
        assert_eq!(CaptureError::Lost.failure(), Failure::DeviceLost);
        assert_eq!(CaptureError::TooLong.failure(), Failure::RecordingTooLong);
    }
}
