//! Contrato de áudio do M1: WAV → mono 16 kHz,
//! formatos, limites e nada de truncar em silêncio.

use std::f32::consts::PI;
use std::path::Path;

use fw_core::audio::{self, AudioError, MAX_DURATION, SAMPLE_RATE};
use hound::{SampleFormat, WavSpec, WavWriter};

fn write_wav(path: &Path, spec: WavSpec, frames: usize, sample: impl Fn(usize, u16) -> f32) {
    let mut w = WavWriter::create(path, spec).expect("create");
    for i in 0..frames {
        for ch in 0..spec.channels {
            let v = sample(i, ch);
            match (spec.sample_format, spec.bits_per_sample) {
                (SampleFormat::Float, _) => w.write_sample(v).expect("w"),
                (SampleFormat::Int, 8) => w.write_sample((v * 127.0) as i8).expect("w"),
                (SampleFormat::Int, 16) => w.write_sample((v * 32767.0) as i16).expect("w"),
                (SampleFormat::Int, bits) => w
                    .write_sample((v * ((1i64 << (bits - 1)) - 1) as f32) as i32)
                    .expect("w"),
            }
        }
    }
    w.finalize().expect("finalize");
}

fn spec(channels: u16, rate: u32, bits: u16, format: SampleFormat) -> WavSpec {
    WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: format,
    }
}

fn sine(freq: f32, rate: u32, amp: f32) -> impl Fn(usize, u16) -> f32 {
    move |i, _| amp * (2.0 * PI * freq * i as f32 / rate as f32).sin()
}

#[test]
fn integer_and_float_formats_become_mono_16k() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        spec(1, 16_000, 8, SampleFormat::Int),
        spec(1, 16_000, 16, SampleFormat::Int),
        spec(2, 16_000, 24, SampleFormat::Int),
        spec(1, 16_000, 32, SampleFormat::Int),
        spec(2, 16_000, 32, SampleFormat::Float),
    ];
    for (n, s) in cases.into_iter().enumerate() {
        let path = dir.path().join(format!("{n}.wav"));
        write_wav(&path, s, 16_000, sine(440.0, 16_000, 0.5));
        let pcm = audio::load_wav(&path).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        assert_eq!(pcm.samples().len(), 16_000, "{s:?}");
        let rms = pcm.rms();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.02, "{s:?}: rms {rms}");
    }
}

#[test]
fn stereo_44k1_is_downmixed_and_resampled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.wav");
    // Canal esquerdo com seno, direito em silêncio: a média reduz pela metade.
    write_wav(
        &path,
        spec(2, 44_100, 16, SampleFormat::Int),
        44_100 * 2,
        |i, ch| {
            if ch == 0 {
                sine(440.0, 44_100, 0.8)(i, ch)
            } else {
                0.0
            }
        },
    );
    let pcm = audio::load_wav(&path).expect("load");

    let expected = 2 * SAMPLE_RATE as usize;
    let len = pcm.samples().len();
    assert!(
        len.abs_diff(expected) <= 16,
        "duração preservada: {len} vs {expected}"
    );
    let rms = pcm.rms();
    assert!((rms - 0.4 / 2f32.sqrt()).abs() < 0.02, "rms {rms}");
}

#[test]
fn full_scale_input_never_clips_after_resampling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("loud.wav");
    // Onda quadrada em escala cheia: o filtro gera overshoot (Gibbs).
    write_wav(
        &path,
        spec(1, 48_000, 32, SampleFormat::Float),
        48_000,
        |i, _| {
            if (i / 50) % 2 == 0 { 1.0 } else { -1.0 }
        },
    );
    let pcm = audio::load_wav(&path).expect("load");
    assert!(pcm.samples().iter().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
fn digital_silence_is_kept_not_dropped() {
    // O ASR decide o que fazer com silêncio; a leitura não descarta áudio.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("silence.wav");
    write_wav(
        &path,
        spec(1, 16_000, 16, SampleFormat::Int),
        8_000,
        |_, _| 0.0,
    );
    let pcm = audio::load_wav(&path).expect("load");
    assert_eq!(pcm.samples().len(), 8_000);
    assert_eq!(pcm.rms(), 0.0);
}

#[test]
fn empty_wav_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("empty.wav");
    write_wav(&path, spec(1, 16_000, 16, SampleFormat::Int), 0, |_, _| 0.0);
    assert!(matches!(audio::load_wav(&path), Err(AudioError::Empty)));
}

#[test]
fn too_long_is_refused_instead_of_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("long.wav");
    let rate = 8_000;
    let frames = (MAX_DURATION.as_secs() as usize + 1) * rate as usize;
    write_wav(
        &path,
        spec(1, rate, 8, SampleFormat::Int),
        frames,
        |_, _| 0.0,
    );
    match audio::load_wav(&path) {
        Err(AudioError::TooLong { max, .. }) => assert_eq!(max, 300),
        other => panic!("esperava TooLong, veio {other:?}"),
    }
}

#[test]
fn non_wav_and_missing_files_are_read_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("x.wav");
    std::fs::write(&path, b"isto nao e um wav").expect("write");
    assert!(matches!(
        audio::load_wav(&path),
        Err(AudioError::Read { .. })
    ));
    assert!(matches!(
        audio::load_wav(&dir.path().join("nao-existe.wav")),
        Err(AudioError::Read { .. })
    ));
}

#[test]
fn nan_samples_are_rejected() {
    let err = audio::to_mono_16k(&[0.1, f32::NAN, 0.2], 1, 16_000).expect_err("NaN");
    assert!(matches!(err, AudioError::Unsupported(_)));
}

#[test]
fn repository_fixtures_are_valid_16k_mono() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (name, secs) in [("en-jfk.wav", 11.0), ("pt-casa-velha.wav", 10.0)] {
        let pcm = audio::load_wav(&root.join(name)).expect(name);
        let d = pcm.duration().as_secs_f64();
        assert!((d - secs).abs() < 0.05, "{name}: {d} s");
        assert!(pcm.rms() > 0.01, "{name} tem fala");
    }
}

// --- Corte de silêncio por energia (M2) ---

fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
    // Ruído pseudoaleatório determinístico (LCG), sem dependência extra.
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amp * ((x >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

fn tone(len: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| amp * (2.0 * PI * 220.0 * i as f32 / SAMPLE_RATE as f32).sin())
        .collect()
}

#[test]
fn digital_silence_has_no_speech() {
    let pcm = audio::Pcm16k::new(vec![0.0; 3 * SAMPLE_RATE as usize]);
    assert_eq!(audio::speech_range(&pcm), None);
    assert!(audio::is_dead(&pcm), "silêncio digital = microfone mudo");
}

#[test]
fn steady_room_noise_has_no_speech() {
    // Ventoinha/ruído de fundo constante, mesmo alto, não vira fala.
    let pcm = audio::Pcm16k::new(noise(3 * SAMPLE_RATE as usize, 0.05, 7));
    assert_eq!(audio::speech_range(&pcm), None);
    assert!(!audio::is_dead(&pcm), "ruído é microfone ligado");
}

#[test]
fn speech_in_the_middle_is_kept_with_margins() {
    let sr = SAMPLE_RATE as usize;
    let mut samples = noise(3 * sr, 0.002, 3);
    for (i, s) in tone(sr, 0.3).into_iter().enumerate() {
        samples[sr + i] += s;
    }
    let range = audio::speech_range(&audio::Pcm16k::new(samples)).expect("fala");
    // Fala de 1,0 s a 2,0 s, margem de 250 ms antes e 350 ms depois.
    let to_ms = |n: usize| n * 1000 / sr;
    assert!(
        (700..=1000).contains(&to_ms(range.start)),
        "início {} ms",
        to_ms(range.start)
    );
    assert!(
        (2000..=2400).contains(&to_ms(range.end)),
        "fim {} ms",
        to_ms(range.end)
    );
}

#[test]
fn a_click_is_not_speech() {
    let sr = SAMPLE_RATE as usize;
    let mut samples = noise(2 * sr, 0.002, 5);
    for s in samples.iter_mut().skip(sr).take(sr / 20) {
        *s += 0.5;
    }
    assert_eq!(audio::speech_range(&audio::Pcm16k::new(samples)), None);
}

#[test]
fn whispered_fixture_still_has_speech() {
    // Sussurro (3% do volume) não pode ser descartado pelo corte de energia.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let loud = audio::load_wav(&root.join("pt-casa-velha.wav")).expect("fixture");
    for gain in [1.0, 0.03] {
        let pcm = audio::Pcm16k::new(loud.samples().iter().map(|s| s * gain).collect());
        let range = audio::speech_range(&pcm).unwrap_or_else(|| panic!("ganho {gain}: sem fala"));
        let kept = (range.end - range.start) as f64 / SAMPLE_RATE as f64;
        assert!(kept > 8.0, "ganho {gain}: manteve só {kept:.1} s");
    }
}

#[test]
fn quiet_live_microphone_is_not_dead() {
    // Fundo de um microfone ligado numa sala silenciosa (≈ −60 dBFS).
    let pcm = audio::Pcm16k::new(noise(2 * SAMPLE_RATE as usize, 0.002, 3));
    assert!(!audio::is_dead(&pcm));
    assert!(!audio::is_dead(&audio::Pcm16k::new(Vec::new())));
}

#[test]
fn skip_start_drops_only_the_guard() {
    let pcm = audio::Pcm16k::new((0..SAMPLE_RATE as usize).map(|i| i as f32).collect());
    let rest = pcm
        .clone()
        .skip_start(std::time::Duration::from_millis(250));
    assert_eq!(rest.samples().len(), SAMPLE_RATE as usize * 3 / 4);
    assert_eq!(rest.samples()[0], (SAMPLE_RATE / 4) as f32);
    assert!(
        pcm.skip_start(std::time::Duration::from_secs(5))
            .samples()
            .is_empty()
    );
}
