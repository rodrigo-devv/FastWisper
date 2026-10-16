//! Driver Vulkan falso e quebrado para os testes (`fake_vulkan_icd.c`),
//! compilado com o `cc` do sistema (o mesmo que compila o whisper.cpp).
//! Compartilhado com os testes da CLI por `#[path]`.

use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A primeira chamada ao driver nunca volta.
    Hang,
    /// A primeira chamada ao driver aborta o processo (SIGABRT).
    Abort,
}

pub struct FakeIcd {
    pub manifest: PathBuf,
    _dir: tempfile::TempDir,
}

impl FakeIcd {
    pub fn build(mode: Mode) -> FakeIcd {
        let dir = tempfile::tempdir().expect("diretório do ICD falso");
        let source = dir.path().join("fake_vulkan_icd.c");
        std::fs::write(&source, include_str!("fake_vulkan_icd.c")).expect("fonte do ICD falso");
        let library = dir.path().join("libfake_vulkan_icd.so");
        let mut cc = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
        cc.args(["-shared", "-fPIC", "-O0", "-o"])
            .arg(&library)
            .arg(&source);
        if mode == Mode::Hang {
            cc.arg("-DFAKE_ICD_HANG");
        }
        let status = cc.status().expect("cc (compilador C) para o ICD falso");
        assert!(status.success(), "falha ao compilar {}", source.display());
        let manifest = dir.path().join("fake_icd.json");
        let json = serde_json::json!({
            "file_format_version": "1.0.0",
            "ICD": { "library_path": library, "api_version": "1.3.0" }
        });
        std::fs::write(&manifest, json.to_string()).expect("manifesto do ICD falso");
        FakeIcd {
            manifest,
            _dir: dir,
        }
    }

    /// Só este driver para o loader do Vulkan, sem camadas implícitas.
    pub fn apply(&self, command: &mut Command) {
        command
            .env("VK_DRIVER_FILES", &self.manifest)
            .env("VK_ICD_FILENAMES", &self.manifest)
            .env_remove("VK_ADD_DRIVER_FILES")
            .env("VK_LOADER_LAYERS_DISABLE", "~implicit~");
    }
}
