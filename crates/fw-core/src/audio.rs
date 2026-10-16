//! Áudio para o ASR: PCM mono 16 kHz em `f32`.
//!
//! M1 lê WAV; a captura do microfone (M2) entra neste módulo depois.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hound::{SampleFormat, WavReader};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use thiserror::Error;

/// Taxa esperada pelo Whisper.
pub const SAMPLE_RATE: u32 = 16_000;
/// Limite do MVP: acima disso, erro visível em vez de truncar.
pub const MAX_DURATION: Duration = Duration::from_secs(300);

/// Áudio pronto para o ASR.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm16k {
    samples: Vec<f32>,
}

impl Pcm16k {
    pub fn new(samples: Vec<f32>) -> Self {
        Self { samples }
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(SAMPLE_RATE))
    }

    /// Sem os primeiros `skip` (o trecho inteiro, se for mais curto).
    pub fn skip_start(mut self, skip: Duration) -> Self {
        let n = (skip.as_secs_f64() * f64::from(SAMPLE_RATE)).round() as usize;
        self.samples.drain(..n.min(self.samples.len()));
        self
    }

    /// RMS do sinal inteiro (0.0 = silêncio digital).
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .samples
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        (sum / self.samples.len() as f64).sqrt() as f32
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("não foi possível ler o WAV {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    #[error("formato não suportado: {0}")]
    Unsupported(String),
    #[error("o áudio está vazio")]
    Empty,
    #[error("áudio de {actual:.1} s passa do limite de {max} s")]
    TooLong { actual: f64, max: u64 },
    #[error("falha ao reamostrar para 16 kHz: {0}")]
    Resample(String),
}

/// Lê um WAV PCM (8/16/24/32 bits inteiro ou 32 bits float), qualquer taxa
/// e número de canais, e converte para mono 16 kHz.
pub fn load_wav(path: &Path) -> Result<Pcm16k, AudioError> {
    let read_err = |source| AudioError::Read {
        path: path.to_path_buf(),
        source,
    };
    let reader = WavReader::open(path).map_err(read_err)?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err(AudioError::Unsupported(
            "cabeçalho sem canais ou taxa".into(),
        ));
    }

    // Recusar antes de decodificar um arquivo longo demais.
    let frames = u64::from(reader.duration());
    check_duration(frames as f64 / f64::from(spec.sample_rate))?;

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(read_err)?,
        (SampleFormat::Int, bits @ (8 | 16 | 24 | 32)) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(read_err)?
        }
        (format, bits) => {
            return Err(AudioError::Unsupported(format!("{format:?}, {bits} bits")));
        }
    };

    to_mono_16k(&interleaved, usize::from(spec.channels), spec.sample_rate)
}

/// Média dos canais e reamostragem para 16 kHz, sem clipping.
pub fn to_mono_16k(interleaved: &[f32], channels: usize, rate: u32) -> Result<Pcm16k, AudioError> {
    if channels == 0 {
        return Err(AudioError::Unsupported("zero canais".into()));
    }
    if interleaved.len() < channels {
        return Err(AudioError::Empty);
    }
    check_duration(interleaved.len() as f64 / channels as f64 / f64::from(rate))?;
    if interleaved.iter().any(|s| !s.is_finite()) {
        return Err(AudioError::Unsupported("amostras NaN ou infinitas".into()));
    }

    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    let samples = if rate == SAMPLE_RATE {
        mono
    } else {
        resample(&mono, rate)?
    };
    Ok(Pcm16k::new(
        samples.into_iter().map(|s| s.clamp(-1.0, 1.0)).collect(),
    ))
}

fn resample(mono: &[f32], rate: u32) -> Result<Vec<f32>, AudioError> {
    let err = |e: &dyn std::fmt::Display| AudioError::Resample(e.to_string());
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .map_err(|e| err(&e))?;
    let input = InterleavedSlice::new(mono, 1, mono.len()).map_err(|e| err(&e))?;
    let output = resampler
        .process_all(&input, mono.len(), None)
        .map_err(|e| err(&e))?;
    Ok(output.take_data())
}

fn check_duration(secs: f64) -> Result<(), AudioError> {
    if secs > MAX_DURATION.as_secs_f64() {
        return Err(AudioError::TooLong {
            actual: secs,
            max: MAX_DURATION.as_secs(),
        });
    }
    Ok(())
}

/// Janela de análise de energia (20 ms a 16 kHz).
const FRAME: usize = 320;
/// Abaixo disso o microfone está mudo ou desconectado (≈ −70 dBFS).
const DEAD_MIC_DB: f32 = -70.0;
/// Faixa mínima entre fundo e pico para haver fala, e não só ruído constante.
const MIN_DYNAMIC_DB: f32 = 8.0;
/// Fala detectada mais curta que isso é clique/respiração.
const MIN_SPEECH: Duration = Duration::from_millis(150);
const PAD_BEFORE: Duration = Duration::from_millis(250);
const PAD_AFTER: Duration = Duration::from_millis(350);

/// Microfone mudo ou desligado no próprio aparelho: a gravação inteira
/// fica abaixo de [`DEAD_MIC_DB`] (silêncio digital, sem nem o ruído de
/// fundo de um microfone ligado).
pub fn is_dead(pcm: &Pcm16k) -> bool {
    let mut energies: Vec<f32> = pcm.samples().chunks(FRAME).map(frame_db).collect();
    if energies.is_empty() {
        return false;
    }
    energies.sort_by(f32::total_cmp);
    percentile(&energies, 0.95) < DEAD_MIC_DB
}

/// Corte de silêncio por energia.
///
/// O limiar é relativo ao próprio áudio (fundo = percentil 10, pico =
/// percentil 95), então fala baixa (sussurro) passa e ruído constante não.
/// Devolve o trecho com fala, com margem, ou `None` se não há fala.
pub fn speech_range(pcm: &Pcm16k) -> Option<std::ops::Range<usize>> {
    let samples = pcm.samples();
    let energies: Vec<f32> = samples.chunks(FRAME).map(frame_db).collect();
    if energies.is_empty() {
        return None;
    }

    let mut sorted = energies.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = percentile(&sorted, 0.10);
    let peak = percentile(&sorted, 0.95);
    if peak < DEAD_MIC_DB || peak - floor < MIN_DYNAMIC_DB {
        return None;
    }

    let threshold = floor + (0.3 * (peak - floor)).max(6.0);
    let first = energies.iter().position(|&e| e >= threshold)?;
    let last = energies.iter().rposition(|&e| e >= threshold)?;
    let speech =
        Duration::from_secs_f64(((last - first + 1) * FRAME) as f64 / f64::from(SAMPLE_RATE));
    if speech < MIN_SPEECH {
        return None;
    }

    let pad = |d: Duration| (d.as_secs_f64() * f64::from(SAMPLE_RATE)) as usize;
    let start = (first * FRAME).saturating_sub(pad(PAD_BEFORE));
    let end = ((last + 1) * FRAME + pad(PAD_AFTER)).min(samples.len());
    Some(start..end)
}

fn frame_db(frame: &[f32]) -> f32 {
    let mean_sq = frame
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / frame.len() as f64;
    (10.0 * (mean_sq + 1e-12).log10()) as f32
}

fn percentile(sorted: &[f32], q: f64) -> f32 {
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx]
}
