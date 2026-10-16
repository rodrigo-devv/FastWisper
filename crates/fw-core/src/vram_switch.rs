//! Troca do modo de VRAM pedida pelo usuário (`fastwisper vram <modo>` e o
//! hub): aplica no daemon na hora e salva `[performance] vram`, preservando
//! o resto do TOML.
//!
//! A gravação é preparada e validada antes de falar com o daemon. Se ele
//! recusar, o temporário some e a config anterior fica intacta; daemon
//! parado só salva (vale no próximo início).

use std::path::{Path, PathBuf};

use thiserror::Error;
use toml_edit::Value;

use crate::client::{self, ClientError};
use crate::config_edit::{self, ConfigEditError};
use crate::ipc::{ReloadReport, Request};
use crate::session::Command;
use crate::vram::{VramMode, VramStatus};

#[derive(Debug)]
pub enum ModeSwitch {
    /// Em vigor no daemon e salvo. `reload`: a recarga que alinha o resto
    /// do arquivo, quando o daemon a fez (falha nela não desfaz o perfil).
    Applied {
        status: VramStatus,
        reload: Option<ReloadReport>,
    },
    /// Daemon parado: salvo, vale no próximo início.
    Saved,
}

#[derive(Debug, Error)]
pub enum SwitchError {
    #[error(transparent)]
    Edit(#[from] ConfigEditError),
    #[error(transparent)]
    Daemon(#[from] ClientError),
    #[error("daemon recusou o perfil: {0}")]
    Refused(String),
    #[error("não foi possível salvar o perfil")]
    SaveStopped(#[source] ConfigEditError),
    #[error("perfil aplicado no daemon, mas não foi possível salvar {}", .path.display())]
    SaveApplied {
        path: PathBuf,
        #[source]
        source: ConfigEditError,
    },
    #[error("daemon não informou o perfil de VRAM")]
    MissingStatus,
}

/// `[performance] vram = "<modo>"` no texto, mantendo comentários.
pub fn edit(text: &str, mode: VramMode, path: &Path) -> Result<String, ConfigEditError> {
    config_edit::set(text, "performance.vram", Value::from(mode.as_str()), path)
}

/// Aplica `mode` no daemon de `socket` e salva em `config`.
pub fn switch_mode(
    socket: &Path,
    config: &Path,
    mode: VramMode,
    request_id: &str,
) -> Result<ModeSwitch, SwitchError> {
    // Trava de escrita da config do começo ao `commit` (como em
    // `config_edit::update`): outro escritor (CLI ou hub) espera.
    let _lock = config_edit::lock(config)?;
    let updated = edit(&config_edit::read(config)?, mode, config)?;
    let prepared = config_edit::prepare(config, &updated)?;
    let response = match client::send(socket, &Request::set_vram(request_id, mode)) {
        Ok(response) => response,
        Err(ClientError::NotRunning(_)) => {
            prepared.commit().map_err(SwitchError::SaveStopped)?;
            return Ok(ModeSwitch::Saved);
        }
        Err(error) => return Err(error.into()),
    };
    if !response.ok {
        return Err(SwitchError::Refused(
            response
                .error
                .map_or_else(|| "erro desconhecido".into(), |e| e.message),
        ));
    }
    prepared
        .commit()
        .map_err(|source| SwitchError::SaveApplied {
            path: config.to_path_buf(),
            source,
        })?;
    let status = response.vram.ok_or(SwitchError::MissingStatus)?;
    // O perfil já vale (`set_vram`); a recarga alinha o resto do que foi
    // editado no arquivo.
    let reload_id = format!("{request_id}-reload");
    let reload = client::send(socket, &Request::new(reload_id, Command::ReloadConfig))
        .ok()
        .filter(|r| r.error.is_none())
        .map(|r| r.reload.unwrap_or_default());
    Ok(ModeSwitch::Applied { status, reload })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn preserves_comments_and_other_settings() {
        let before = "# minha config\n[performance] # perfil\n# escolha pessoal\nvram = 'automatico' # manter\nunload_after_secs = 42\n\n[privacy]\nstore_text = false # privado\n";
        let after = edit(before, VramMode::Jogo, Path::new("config.toml")).expect("válido");
        assert_eq!(after, before.replace("'automatico'", "\"jogo\""));
        assert_eq!(
            Config::parse(&after, Path::new("config.toml"))
                .expect("config")
                .performance
                .vram,
            VramMode::Jogo
        );
    }

    #[test]
    fn missing_section_and_inline_table_are_supported() {
        for before in [
            "# config\n",
            "performance = { vram = 'automatico', unload_after_secs = 10 } # comentário\n",
        ] {
            let after = edit(before, VramMode::SobDemanda, Path::new("c.toml")).expect("válido");
            assert!(after.contains("sob_demanda"));
            assert!(after.contains('#'));
        }
        assert!(edit("performance = 42", VramMode::Jogo, Path::new("c")).is_err());
        assert!(
            edit(
                "[privacy]\nstore_txt = true",
                VramMode::Jogo,
                Path::new("c")
            )
            .is_err()
        );
    }

    #[test]
    fn stopped_daemon_only_saves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "# minha\n[performance]\nvram = \"automatico\"\n").expect("config");
        let socket = dir.path().join("sem-daemon.sock");
        let result = switch_mode(&socket, &config, VramMode::Desempenho, "t-1").expect("salva");
        assert!(matches!(result, ModeSwitch::Saved));
        let text = std::fs::read_to_string(&config).expect("lida");
        assert!(text.starts_with("# minha\n") && text.contains("\"desempenho\""));
    }

    #[test]
    fn invalid_config_is_not_touched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "[privacy]\nstore_txt = true\n").expect("config");
        let socket = dir.path().join("sem-daemon.sock");
        assert!(matches!(
            switch_mode(&socket, &config, VramMode::Jogo, "t-2"),
            Err(SwitchError::Edit(_))
        ));
        assert_eq!(
            std::fs::read_to_string(&config).expect("lida"),
            "[privacy]\nstore_txt = true\n"
        );
    }
}
