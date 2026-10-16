//! Contrato de captura contra um PipeWire real,
//! com microfones virtuais (nada toca na saída de som do usuário).
//!
//! `cargo test -p fw-core --features pipewire --test capture_contract -- --ignored --test-threads=1`
//! O teste ponta a ponta com o modelo também precisa de `--features vulkan`.

#![cfg(feature = "pipewire")]

mod common;

use std::f32::consts::PI;
use std::thread::sleep;
use std::time::Duration;

use common::virtual_mic::VirtualMic;
use fw_core::audio::SAMPLE_RATE;
use fw_core::capture::{Capture, CaptureError};

const SECOND: usize = SAMPLE_RATE as usize;

fn tone(secs: f32) -> Vec<f32> {
    (0..(secs * SAMPLE_RATE as f32) as usize)
        .map(|i| 0.5 * (2.0 * PI * 330.0 * i as f32 / SAMPLE_RATE as f32).sin())
        .collect()
}

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn records_mono_16k_from_the_strict_device() {
    let mic = VirtualMic::new("Mic", tone(1.0));
    let capture = Capture::spawn().expect("PipeWire");

    let chosen = capture
        .start(1, &mic.description, 10 * SECOND)
        .expect("start");
    assert_eq!(chosen.description, mic.description);
    sleep(Duration::from_millis(1500));
    let rec = capture.stop(1).expect("stop");

    assert_eq!((rec.rate, rec.channels), (SAMPLE_RATE, 1));
    let secs = rec.samples.len() as f32 / SAMPLE_RATE as f32;
    assert!((1.2..=1.8).contains(&secs), "gravou {secs:.2} s");
    assert!(rms(&rec.samples[..SECOND / 2]) > 0.2, "o tom chegou");
    let first = rec.first_buffer.expect("primeiro buffer");
    assert!(
        first < Duration::from_millis(300),
        "primeiro buffer em {first:?}"
    );
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn absent_device_fails_without_falling_back() {
    let capture = Capture::spawn().expect("PipeWire");
    let err = capture
        .start(1, "FW Test Inexistente", SECOND)
        .expect_err("não pode gravar de outro microfone");
    assert!(matches!(err, CaptureError::Unavailable { .. }), "{err:?}");
    assert!(matches!(capture.stop(1), Err(CaptureError::NotRecording)));
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn two_matching_devices_are_ambiguous() {
    let a = VirtualMic::new("Dup", vec![]);
    let b = VirtualMic::new("Dup", vec![]);
    let capture = Capture::spawn().expect("PipeWire");
    let prefix = "FW Test Dup";
    assert!(a.description.starts_with(prefix) && b.description.starts_with(prefix));
    let err = capture.start(1, prefix, SECOND).expect_err("ambíguo");
    assert!(matches!(err, CaptureError::Ambiguous { .. }), "{err:?}");
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn unplugging_mid_recording_is_device_lost_and_replug_needs_a_new_press() {
    let mic = VirtualMic::new("Plug", tone(5.0));
    let description = mic.description.clone();
    let capture = Capture::spawn().expect("PipeWire");

    capture.start(1, &description, 10 * SECOND).expect("start");
    sleep(Duration::from_millis(400));
    drop(mic);
    sleep(Duration::from_millis(200));
    assert_eq!(
        capture.stop(1),
        Err(CaptureError::Lost),
        "áudio parcial não vira sucesso"
    );

    // Sem o microfone, nada de gravar de outro dispositivo.
    assert!(matches!(
        capture.start(2, &description, SECOND),
        Err(CaptureError::Unavailable { .. })
    ));

    // Reconexão explícita: o mesmo nome volta e um novo start funciona.
    let replugged = VirtualMic::new("Plug2", tone(1.0));
    capture
        .start(3, &replugged.description, 10 * SECOND)
        .expect("novo start");
    sleep(Duration::from_millis(300));
    assert!(capture.stop(3).is_ok());
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn exceeding_the_buffer_aborts_instead_of_truncating() {
    let mic = VirtualMic::new("Long", tone(3.0));
    let capture = Capture::spawn().expect("PipeWire");
    capture
        .start(1, &mic.description, SECOND / 2)
        .expect("start");
    sleep(Duration::from_millis(1200));
    assert_eq!(capture.stop(1), Err(CaptureError::TooLong));
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn stop_and_abort_from_another_session_are_ignored() {
    let mic = VirtualMic::new("Tag", tone(2.0));
    let capture = Capture::spawn().expect("PipeWire");
    capture
        .start(7, &mic.description, 10 * SECOND)
        .expect("start");
    sleep(Duration::from_millis(300));

    capture.abort(6);
    assert_eq!(capture.stop(6), Err(CaptureError::NotRecording));
    let rec = capture.stop(7).expect("a sessão certa ainda grava");
    assert!(rec.samples.len() > SECOND / 10);
}

#[test]
#[ignore = "precisa de sessão PipeWire"]
fn level_follows_the_microphone_and_resets_when_closed() {
    let loud = VirtualMic::new("Nivel", tone(3.0));
    let mute = VirtualMic::new("NivelMudo", vec![]);
    let capture = Capture::spawn().expect("PipeWire");
    assert_eq!(capture.level(), 0.0, "sem gravação, nível 0");

    capture
        .start(1, &loud.description, 10 * SECOND)
        .expect("start");
    sleep(Duration::from_millis(500));
    // Tom de amplitude 0,5: RMS 0,35 ≈ −9 dBFS ≈ 0,85 na escala.
    let level = capture.level();
    assert!((0.75..=0.95).contains(&level), "nível com tom: {level}");
    capture.stop(1).expect("stop");
    assert_eq!(capture.level(), 0.0, "volta a 0 quando o microfone fecha");

    capture
        .start(2, &mute.description, 10 * SECOND)
        .expect("start");
    sleep(Duration::from_millis(500));
    let level = capture.level();
    assert!(level < 0.01, "nível com silêncio: {level}");
    capture.stop(2).expect("stop");
}

#[cfg(feature = "vulkan")]
mod end_to_end {
    //! Daemon completo: microfone virtual falando a fixture → Whisper.

    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use fw_core::audio;
    use fw_core::config::Config;
    use fw_core::dictation::{DictationConfig, DictationPipeline};
    use fw_core::pipeline::{OutcomeKind, Readiness};
    use fw_core::polish::Polisher;
    use fw_core::session::{Command, StateName};

    use super::common::TestDaemon;
    use super::common::virtual_mic::VirtualMic;

    async fn daemon_for(mic: &VirtualMic) -> (TestDaemon, DictationPipeline) {
        let mut config = Config::default();
        // Os sons de início/fim tocariam no fone de verdade.
        config.audio.sounds = false;
        config.audio.device = mic.description.clone();
        if let Ok(model) = std::env::var("FASTWISPER_TEST_MODEL") {
            config.asr.model = model;
        }
        let pipeline = DictationPipeline::new(
            DictationConfig::from_config(&config),
            Polisher::new(&config),
        )
        .expect("pipeline");
        let daemon = TestDaemon::start(Arc::new(pipeline.clone())).await;
        for _ in 0..600 {
            if daemon.ctl(Command::Status).await.asr == Some(Readiness::Ready) {
                return (daemon, pipeline);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("ASR não ficou pronto");
    }

    async fn dictate(daemon: &TestDaemon, speak_for: Duration) {
        let pressed = daemon.ctl(Command::Press).await;
        assert!(pressed.ok, "{pressed:?}");
        tokio::time::sleep(speak_for).await;
        daemon.ctl(Command::Release).await;
        for _ in 0..200 {
            if daemon.ctl(Command::Status).await.state == Some(StateName::Idle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("não voltou para idle");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo e GPU"]
    async fn portuguese_speech_from_the_microphone_becomes_text() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pt-casa-velha.wav");
        let speech = audio::load_wav(&fixture).expect("fixture");
        let mic = VirtualMic::new("Fala", speech.samples().to_vec());
        let (daemon, _pipeline) = daemon_for(&mic).await;

        dictate(&daemon, Duration::from_millis(10_500)).await;

        let status = daemon.ctl(Command::Status).await;
        let last = status.last.expect("resumo");
        assert_eq!(last.outcome, OutcomeKind::TRANSCRIBED, "{status:?}");
        assert_eq!(last.language.map(|l| l.code()), Some("pt"));
        assert!(
            last.metrics.llm.is_some(),
            "o texto passou por regras + LLM: {:?}",
            last.metrics
        );
        let line = String::from_utf8(status.to_line()).expect("utf8");
        assert!(
            !line.to_lowercase().contains("lalau"),
            "status sem texto: {line}"
        );

        let shown = daemon.ctl(Command::ShowLast).await;
        let text = shown.text.expect("texto").to_lowercase();
        assert!(text.contains("lalau") && text.contains("vestido"), "{text}");

        daemon.stop().await;
    }

    /// VRAM (MiB) deste processo segundo o `nvidia-smi`, se houver.
    fn gpu_mib() -> Option<u64> {
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-compute-apps=pid,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()?;
        let pid = std::process::id().to_string();
        Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split_once(','))
                .filter(|(p, _)| p.trim() == pid)
                .filter_map(|(_, mib)| mib.trim().parse::<u64>().ok())
                .sum(),
        )
    }

    /// M7: no perfil sob demanda o ASR aquece uma vez no início e sai após
    /// `unload_after`; parado, nada fica carregado; o press carrega ASR e LLM
    /// enquanto a pessoa fala e a descarga vem depois de `unload_after`. Usa o LLM de `FASTWISPER_TEST_LLM` (padrão: o da
    /// config), carregado e descarregado só por este teste.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo, GPU e Ollama"]
    async fn on_demand_loads_on_press_and_unloads_after_idle() {
        use fw_core::vram::{GameLlm, ModelState, Profile, VramMode, VramPolicy};

        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pt-casa-velha.wav");
        let _ = tracing_subscriber::fmt()
            .with_env_filter("fw_core=info")
            .with_test_writer()
            .try_init();
        let speech = audio::load_wav(&fixture).expect("fixture");
        let mut samples = speech.samples().to_vec();
        // Duas falas, uma por ditado (o microfone virtual só avança enquanto
        // alguém captura).
        samples.extend_from_slice(speech.samples());
        let mic = VirtualMic::new("Sob demanda", samples);
        let mut config = Config::default();
        // Os sons de início/fim tocariam no fone de verdade.
        config.audio.sounds = false;
        config.audio.device = mic.description.clone();
        if let Ok(model) = std::env::var("FASTWISPER_TEST_MODEL") {
            config.asr.model = model;
        }
        if let Ok(model) = std::env::var("FASTWISPER_TEST_LLM") {
            config.llm.model = model;
        }
        let pipeline = DictationPipeline::new(
            DictationConfig::from_config(&config),
            Polisher::new(&config),
        )
        .expect("pipeline");
        let unload_after = Duration::from_secs(4);
        let daemon = super::common::TestDaemon::start_configured(
            Arc::new(pipeline),
            Arc::new(super::common::FakeDesktop::default()),
            |d| {
                d.with_vram(VramPolicy::new(
                    VramMode::SobDemanda,
                    unload_after,
                    GameLlm::Off,
                ))
            },
        )
        .await;

        // Aquecimento único do ASR no início; depois, a ociosidade do perfil.
        let started = std::time::Instant::now();
        let mut warm_peak = None;
        loop {
            let vram = daemon.ctl(Command::Status).await.vram.expect("vram");
            if vram.asr == ModelState::Loaded && warm_peak.is_none() {
                warm_peak = gpu_mib();
                println!(
                    "aquecido em {:?}: {vram:?} · nvidia-smi deste processo: {warm_peak:?} MiB",
                    started.elapsed()
                );
            }
            if warm_peak.is_some() && vram.asr == ModelState::Unloaded {
                println!("aquecimento descarregado após {:?}", started.elapsed());
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(90),
                "aquecimento não terminou"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let idle = daemon.ctl(Command::Status).await.vram.expect("vram");
        assert_eq!(
            (idle.asr, idle.llm),
            (ModelState::Unloaded, ModelState::Unloaded)
        );
        let before = gpu_mib();
        println!("parado: {idle:?} · nvidia-smi deste processo: {before:?} MiB");

        dictate(&daemon, Duration::from_millis(10_500)).await;
        let status = daemon.ctl(Command::Status).await;
        let last = status.last.expect("resumo");
        let loaded = status.vram.expect("vram");
        let during = gpu_mib();
        println!("depois do ditado: {last:?}");
        println!("carregado: {loaded:?} · nvidia-smi deste processo: {during:?} MiB");
        assert_eq!(last.outcome, OutcomeKind::TRANSCRIBED);
        assert_eq!(last.metrics.profile, Some(Profile::SobDemanda));
        assert!(last.metrics.load_ms.is_some(), "carga a frio no press");
        assert_eq!(
            (loaded.asr, loaded.llm),
            (ModelState::Loaded, ModelState::Loaded)
        );
        assert!(loaded.unload_in_secs.is_some());
        if let (Some(before), Some(during)) = (before, during) {
            assert!(during >= before + 500, "o Whisper foi para a GPU");
        }

        tokio::time::sleep(unload_after + Duration::from_secs(3)).await;
        let after = daemon.ctl(Command::Status).await.vram.expect("vram");
        let freed = gpu_mib();
        println!("após unload_after: {after:?} · nvidia-smi deste processo: {freed:?} MiB");
        assert_eq!(
            (after.asr, after.llm),
            (ModelState::Unloaded, ModelState::Unloaded)
        );
        assert_eq!(after.vram_mb, Some(0));
        if let (Some(during), Some(freed)) = (during, freed) {
            assert!(freed + 500 <= during, "VRAM do Whisper liberada");
        }

        // Segunda carga a frio no mesmo processo (o caso do dia a dia).
        dictate(&daemon, Duration::from_millis(10_500)).await;
        let again = daemon.ctl(Command::Status).await.last.expect("resumo");
        println!("segundo ditado: {again:?}");
        assert_eq!(again.outcome, OutcomeKind::TRANSCRIBED);
        assert!(
            again.metrics.load_ms.is_some(),
            "recarregou depois da descarga"
        );
        daemon.stop().await;
    }

    /// Abre o stream `meter` e devolve os níveis até a gravação acabar.
    async fn meter_levels(daemon: &TestDaemon, record_for: Duration) -> Vec<f64> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut stream = tokio::net::UnixStream::connect(&daemon.paths.socket)
            .await
            .expect("connect");
        stream
            .write_all(b"{\"v\":1,\"request_id\":\"m\",\"cmd\":\"meter\"}\n")
            .await
            .expect("write");
        let mut lines = BufReader::new(stream).lines();
        let pressed = daemon.ctl(Command::Press).await;
        assert!(pressed.ok, "{pressed:?}");
        tokio::time::sleep(record_for).await;
        // `cancel`, não `release`: nada de transcrever nem entregar.
        daemon.ctl(Command::Cancel).await;
        let mut levels = Vec::new();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
                .await
                .expect("evento a tempo")
                .expect("leitura")
                .expect("conexão aberta");
            let event: serde_json::Value = serde_json::from_str(&line).expect("json");
            if event["state"] != "recording" {
                return levels;
            }
            levels.push(event["level"].as_f64().expect("nível"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo e GPU"]
    async fn meter_streams_the_microphone_level_while_recording() {
        let mic = VirtualMic::new("Medidor", super::tone(3.0));
        let (daemon, _pipeline) = daemon_for(&mic).await;
        let levels = meter_levels(&daemon, Duration::from_millis(1000)).await;
        assert!(levels.len() >= 15, "~25 Hz: {} eventos", levels.len());
        let peak = levels.iter().copied().fold(0.0, f64::max);
        assert!(peak > 0.7, "tom aparece no medidor: {levels:?}");
        daemon.stop().await;

        let mute = VirtualMic::new("MedidorMudo", vec![]);
        let (daemon, _pipeline) = daemon_for(&mute).await;
        let levels = meter_levels(&daemon, Duration::from_millis(600)).await;
        assert!(!levels.is_empty());
        assert!(levels.iter().all(|l| *l < 0.01), "silêncio ≈ 0: {levels:?}");
        daemon.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo e GPU"]
    async fn silent_microphone_never_reaches_whisper() {
        let mic = VirtualMic::new("Mudo", vec![]);
        let (daemon, _pipeline) = daemon_for(&mic).await;

        dictate(&daemon, Duration::from_millis(1500)).await;

        let last = daemon.ctl(Command::Status).await.last.expect("resumo");
        // Silêncio digital: microfone mudo/desligado no aparelho, com aviso.
        assert_eq!(
            last.outcome,
            OutcomeKind::Failed(fw_core::pipeline::Failure::MicSilent)
        );
        assert_eq!(last.metrics.asr_ms, None, "o Whisper não rodou");
        assert!(daemon.ctl(Command::ShowLast).await.text.is_none());
        daemon.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo e GPU"]
    async fn unconfigured_microphone_refuses_to_record() {
        let mut config = Config::default();
        // Os sons de início/fim tocariam no fone de verdade.
        config.audio.sounds = false;
        config.audio.device = String::new();
        let pipeline = DictationPipeline::new(
            DictationConfig::from_config(&config),
            Polisher::new(&config),
        )
        .expect("pipeline");
        let daemon = TestDaemon::start(Arc::new(pipeline)).await;
        let refused = daemon.ctl(Command::Press).await;
        assert_eq!(
            refused.error_code(),
            Some(fw_core::ipc::ErrorCode::DeviceNotConfigured),
            "sem microfone configurado não grava de nenhum"
        );
        assert_eq!(refused.state, Some(StateName::Idle));
        daemon.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de PipeWire, modelo e GPU"]
    async fn quick_tap_is_discarded_as_too_short() {
        let mic = VirtualMic::new("Toque", super::tone(1.0));
        let (daemon, _pipeline) = daemon_for(&mic).await;

        dictate(&daemon, Duration::from_millis(120)).await;
        let last = daemon.ctl(Command::Status).await.last.expect("resumo");
        assert_eq!(last.outcome, OutcomeKind::TOO_SHORT);
        daemon.stop().await;
    }
}
