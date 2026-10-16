//! Sons de início e fim da gravação, sintetizados aqui (sem arquivos de
//! áudio de terceiros). Curtos e suaves: senoides com ataque rápido e
//! decaimento exponencial, como um "pop" de duas notas.

use std::time::Duration;

/// Taxa dos sons tocados (a do PipeWire na maioria das placas).
pub const RATE: u32 = 48_000;

/// Silêncio no fim do som, para o último buffer sair inteiro antes de o
/// stream pausar.
const TAIL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// Microfone aberto: duas notas subindo.
    Start,
    /// Microfone fechado: duas notas descendo.
    Stop,
}

/// Notas (Hz, duração) de cada som.
fn notes(cue: Cue) -> [(f32, Duration); 2] {
    const A5: f32 = 880.0;
    const E6: f32 = 1318.5;
    match cue {
        Cue::Start => [
            (A5, Duration::from_millis(45)),
            (E6, Duration::from_millis(55)),
        ],
        Cue::Stop => [
            (E6, Duration::from_millis(45)),
            (A5, Duration::from_millis(65)),
        ],
    }
}

/// Duração audível (sem a cauda de silêncio).
pub fn duration(cue: Cue) -> Duration {
    notes(cue).iter().map(|(_, d)| *d).sum()
}

fn samples(d: Duration) -> usize {
    (d.as_secs_f64() * f64::from(RATE)).round() as usize
}

/// PCM F32 mono a [`RATE`], com pico `volume` (0–1) e cauda de silêncio.
pub fn render(cue: Cue, volume: f32) -> Vec<f32> {
    let volume = volume.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(samples(duration(cue) + TAIL));
    for (freq, length) in notes(cue) {
        let n = samples(length);
        let attack = samples(Duration::from_millis(4)).max(1);
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let rise = (i as f32 / attack as f32).min(1.0);
            // Decai ~40 dB ao longo da nota; os últimos 2 ms vão a zero
            // para não estalar na troca de nota.
            let decay = (-4.6 * i as f32 / n as f32).exp();
            let fade = ((n - i) as f32 / samples(Duration::from_millis(2)) as f32).min(1.0);
            let wave = (std::f32::consts::TAU * freq * t).sin()
                + 0.25 * (std::f32::consts::TAU * 2.0 * freq * t).sin();
            out.push(volume * 0.8 * rise * decay * fade * wave);
        }
    }
    out.resize(out.len() + samples(TAIL), 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_short_bounded_and_end_silent() {
        for cue in [Cue::Start, Cue::Stop] {
            assert!(duration(cue) <= Duration::from_millis(120), "{cue:?} longo");
            let pcm = render(cue, 1.0);
            assert_eq!(pcm.len(), samples(duration(cue) + TAIL));
            assert!(pcm.iter().all(|s| s.abs() <= 1.0), "{cue:?} satura");
            assert!(pcm.iter().any(|s| s.abs() > 0.3), "{cue:?} mudo");
            assert!(pcm[pcm.len() - samples(TAIL)..].iter().all(|s| *s == 0.0));
            assert!(pcm[0].abs() < 0.01, "{cue:?} começa com estalo");
        }
    }

    #[test]
    fn volume_scales_and_is_clamped() {
        let peak = |v: f32| {
            render(Cue::Start, v)
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()))
        };
        assert!((peak(0.5) * 2.0 - peak(1.0)).abs() < 1e-3);
        assert_eq!(peak(0.0), 0.0);
        assert!((peak(3.0) - peak(1.0)).abs() < 1e-6);
    }
}
