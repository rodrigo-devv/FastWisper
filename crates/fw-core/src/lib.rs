//! Núcleo do FastWisper. O daemon é o único dono da sessão; CLI, Waybar e
//! UIs futuras falam com ele pelo socket local (IPC JSONL versionado).

/// Versão do FastWisper (a do workspace do Cargo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod asr;
pub mod audio;
pub mod capture;
pub mod client;
pub mod config;
pub mod config_edit;
pub mod cues;
pub mod daemon;
#[cfg(all(feature = "whisper", feature = "pipewire"))]
pub mod dictation;
pub mod doctor;
pub mod game_detector;
pub mod gpu_probe;
pub mod history;
pub mod hotkeys;
pub mod hypr;
pub mod i18n;
pub mod inject;
pub mod ipc;
pub mod llm;
pub mod models;
pub mod notes;
pub mod ollama;
pub mod pipeline;
pub mod polish;
pub mod sandbox;
pub mod session;
pub mod setup;
pub mod terminal;
pub mod text;
pub mod vram;
pub mod vram_switch;
