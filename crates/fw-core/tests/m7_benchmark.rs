//! Coletor opt-in do M7: PipeWire virtual, daemon isolado e desktop falso.
#![cfg(all(feature = "pipewire", feature = "vulkan"))]
mod common;

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{FakeDesktop, TestDaemon, virtual_mic::VirtualMic};
use fw_core::audio;
use fw_core::config::Config;
use fw_core::dictation::{DictationConfig, DictationPipeline};
use fw_core::pipeline::OutcomeKind;
use fw_core::polish::Polisher;
use fw_core::session::{Command, StateName};
use fw_core::vram::{ModelState, VramMode, VramPolicy};

fn asr_mib() -> u64 {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .expect("nvidia-smi");
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .expect("CSV")
        .lines()
        .filter_map(|l| l.split_once(','))
        .filter(|(pid, _)| pid.trim() == std::process::id().to_string())
        .map(|(_, mib)| mib.trim().parse::<u64>().expect("MiB"))
        .sum()
}
async fn llm_mib(client: &reqwest::Client, model: &str) -> u64 {
    let ps: serde_json::Value = client
        .get("http://127.0.0.1:11434/api/ps")
        .send()
        .await
        .expect("ps")
        .error_for_status()
        .expect("HTTP")
        .json()
        .await
        .expect("JSON");
    ps["models"]
        .as_array()
        .expect("models")
        .iter()
        .filter(|m| m["name"] == model)
        .map(|m| m["size_vram"].as_u64().expect("size_vram") / (1 << 20))
        .sum()
}
fn llm_nvidia_mib(blob: &str) -> u64 {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .expect("nvidia-smi");
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .expect("CSV")
        .lines()
        .filter_map(|l| l.split_once(','))
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{}/cmdline", pid.trim())).unwrap_or_default();
            let args: Vec<_> = cmd.split(|b| *b == 0).collect();
            args.windows(2)
                .any(|a| a[0] == b"--model" && a[1] == blob.as_bytes())
        })
        .map(|(_, mib)| mib.trim().parse::<u64>().expect("MiB"))
        .sum()
}
async fn unloaded(daemon: &TestDaemon, timeout: Duration) {
    let start = Instant::now();
    loop {
        let v = daemon.ctl(Command::Status).await.vram.expect("VRAM");
        if v.asr == ModelState::Unloaded && v.llm == ModelState::Unloaded {
            return;
        }
        assert!(start.elapsed() < timeout, "não descarregou: {v:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "medição real: requer GPU livre, PipeWire, Ollama e M7_BENCH_CSV"]
async fn collect_m7() {
    let output = std::env::var("M7_BENCH_CSV").expect("destino explícito");
    let model = std::env::var("FASTWISPER_TEST_LLM").expect("modelo de teste explícito");
    assert_ne!(model, "qwen3:4b-instruct", "não usar o modelo do dono");
    let client = reqwest::Client::new();
    let show: serde_json::Value = client
        .post("http://127.0.0.1:11434/api/show")
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .expect("show")
        .error_for_status()
        .expect("HTTP")
        .json()
        .await
        .expect("JSON");
    let blob = show["modelfile"]
        .as_str()
        .expect("modelfile")
        .lines()
        .find_map(|line| line.strip_prefix("FROM /").map(|path| format!("/{path}")))
        .expect("caminho local do GGUF");
    let version: serde_json::Value = client
        .get("http://127.0.0.1:11434/api/version")
        .send()
        .await
        .expect("versão")
        .error_for_status()
        .expect("HTTP")
        .json()
        .await
        .expect("JSON");
    let tags: serde_json::Value = client
        .get("http://127.0.0.1:11434/api/tags")
        .send()
        .await
        .expect("tags")
        .error_for_status()
        .expect("HTTP")
        .json()
        .await
        .expect("JSON");
    let digest = tags["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|m| m["name"] == model)
        .expect("modelo instalado")["digest"]
        .as_str()
        .expect("digest");
    let gpu = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
        .output()
        .expect("GPU/driver");
    assert!(gpu.status.success());
    let runtime = format!(
        "Ollama {}; whisper.cpp 1.8.3 Vulkan (whisper-rs-sys 0.15.0); {}; LLM {} digest {}; ASR large-v3-turbo q5_0; release",
        version["version"].as_str().expect("versão"),
        String::from_utf8(gpu.stdout)
            .expect("GPU")
            .trim()
            .replace(',', ";"),
        model,
        digest
    );
    assert_eq!(
        llm_mib(&client, &model).await,
        0,
        "modelo já pertence a outro cliente"
    );
    let mut csv = std::fs::File::create(output).expect("CSV");
    writeln!(csv, "modo,unload_after_secs,execucao,asr_parado_mib,llm_parado_mib,asr_pico_amostrado_mib,llm_pico_amostrado_mib,asr_apos_mib,llm_apos_mib,load_ms,llm_load_ms,e2e_ms,press_entrega_ms,espera_descarga_ms,llm_resultado,llm_pico_api_ps_mib,runtime,metodo").expect("medição");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pt-casa-velha.wav");
    let speech = audio::load_wav(&fixture).expect("fixture");
    for (mode, secs) in [
        (VramMode::SobDemanda, 0u32),
        (VramMode::SobDemanda, 300),
        (VramMode::Desempenho, 300),
        (VramMode::Jogo, 0),
        (VramMode::Automatico, 0),
    ] {
        let mut samples = speech.samples().to_vec();
        samples.resize(168_000, 0.0);
        samples.extend_from_within(..);
        let mic = VirtualMic::new("M7", samples);
        let dir = common::runtime_dir();
        let path = dir.path().join("config.toml");
        let text = fw_core::setup::EXAMPLE_CONFIG
            .replace("device = \"\"", &format!("device = {:?}", mic.description))
            .replace(
                "model = \"qwen3:4b-instruct\"",
                &format!("model = {model:?}"),
            )
            .replace("vram = \"automatico\"", &format!("vram = \"{mode}\""))
            .replace(
                "unload_after_secs = 300",
                &format!("unload_after_secs = {secs}"),
            );
        std::fs::write(&path, text).expect("medição");
        let config = Config::load(Some(&path)).expect("medição");
        let pipeline = DictationPipeline::new(
            DictationConfig::from_config(&config),
            Polisher::new(&config),
        )
        .expect("medição");
        let daemon = TestDaemon::start_configured(
            Arc::new(pipeline),
            Arc::new(FakeDesktop::default()),
            |d| d.with_vram(VramPolicy::from_config(&config.performance)),
        )
        .await;
        let start = Instant::now();
        loop {
            let v = daemon.ctl(Command::Status).await.vram.expect("medição");
            if v.asr == ModelState::Loaded
                || (secs == 0
                    && v.asr == ModelState::Unloaded
                    && start.elapsed() > Duration::from_secs(2))
            {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(90));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if mode != VramMode::Desempenho {
            daemon.ctl(Command::Unload).await;
            unloaded(&daemon, Duration::from_secs(90)).await;
        } else {
            while daemon.ctl(Command::Status).await.vram.expect("medição").llm != ModelState::Loaded
            {
                assert!(start.elapsed() < Duration::from_secs(90));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        for run in 1..=2 {
            let idle_asr = asr_mib();
            let idle_llm = llm_nvidia_mib(&blob);
            let press = Instant::now();
            assert!(daemon.ctl(Command::Press).await.ok);
            let mut peak_asr = idle_asr;
            let mut peak_llm = idle_llm;
            let mut peak_api = llm_mib(&client, &model).await;
            while press.elapsed() < Duration::from_millis(10_500) {
                peak_asr = peak_asr.max(asr_mib());
                peak_llm = peak_llm.max(llm_nvidia_mib(&blob));
                peak_api = peak_api.max(llm_mib(&client, &model).await);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(daemon.ctl(Command::Release).await.ok);
            let last = loop {
                let status = daemon.ctl(Command::Status).await;
                peak_asr = peak_asr.max(asr_mib());
                peak_llm = peak_llm.max(llm_nvidia_mib(&blob));
                peak_api = peak_api.max(llm_mib(&client, &model).await);
                if status.state == Some(StateName::Idle) {
                    break status.last.expect("resumo");
                }
                assert!(press.elapsed() < Duration::from_secs(40));
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            let press_ms = press.elapsed().as_millis();
            assert_eq!(last.outcome, OutcomeKind::TRANSCRIBED);
            let wait = Instant::now();
            // A primeira fala de 300 s conserva os modelos para a segunda (quente).
            if mode != VramMode::Desempenho && (secs == 0 || run == 2) {
                unloaded(&daemon, Duration::from_secs(u64::from(secs) + 30)).await;
            }
            let m = last.metrics;
            writeln!(csv, "{mode},{secs},{run},{idle_asr},{idle_llm},{peak_asr},{peak_llm},{},{},{},{},{},{press_ms},{},{:?},{peak_api},{runtime},ASR por PID nvidia-smi; LLM nvidia-smi por PID --model igual ao blob api/show; api/ps separado; amostragem >=100 ms; fixture pt-casa-velha 10 s; captura 10.5 s; entrega fake; cache shader quente; daemon/config temporários 0700; automatico sem detector=fora do jogo", asr_mib(), llm_nvidia_mib(&blob), m.load_ms.map_or(String::new(), |v| v.to_string()), m.llm_load_ms.map_or(String::new(), |v| v.to_string()), m.e2e_ms.expect("medição"), wait.elapsed().as_millis(), m.llm).expect("medição");
            csv.flush().expect("medição");
            println!("{mode}/{secs} execução {run}: {m:?}");
        }
        daemon.stop().await;
        assert_eq!(llm_mib(&client, &model).await, 0);
    }
}
