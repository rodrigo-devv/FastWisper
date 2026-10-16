use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};

use fw_core::{client, config::Config, daemon::DaemonPaths, ipc::Request, session::Command};
use serde::Serialize;

#[derive(Serialize)]
pub struct HubStatus {
    state: Option<fw_core::session::StateName>,
    available: bool,
}

#[tauri::command]
pub async fn hub_status() -> Result<HubStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let paths = DaemonPaths::from_env().map_err(|_| "Runtime do daemon indisponível")?;
        match client::send(&paths.socket, &Request::new("hub-status", Command::Status)) {
            Ok(response) if response.ok => Ok(HubStatus {
                state: response.state,
                available: true,
            }),
            Ok(_) => Err("O daemon recusou a consulta".to_owned()),
            Err(client::ClientError::NotRunning(_)) => Ok(HubStatus {
                state: None,
                available: false,
            }),
            Err(_) => Err("Falha de comunicação com o daemon".to_owned()),
        }
    })
    .await
    .map_err(|_| "Falha ao consultar o daemon".to_owned())?
}

#[derive(Serialize)]
pub struct ConfigSummary {
    sounds: bool,
    llm_enabled: bool,
    store_text: bool,
    store_audio: bool,
}

#[tauri::command]
pub async fn config_summary() -> Result<ConfigSummary, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let config = Config::load(None).map_err(|_| "Não foi possível ler a configuração")?;
        Ok(ConfigSummary {
            sounds: config.audio.sounds,
            llm_enabled: config.llm.enabled,
            store_text: config.privacy.store_text,
            store_audio: config.privacy.store_audio,
        })
    })
    .await
    .map_err(|_| "Falha ao ler a configuração".to_owned())?
}
/// Versão do FastWisper, para o rodapé do menu.
#[tauri::command]
pub fn hub_version() -> &'static str {
    fw_core::VERSION
}

/// Guia local que o `install.sh` instala junto do hub:
/// `$XDG_DATA_HOME/doc/fastwisper/README.md` (ou `~/.local/share/...`).
fn guide_path(data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match data_home.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".local/share"),
    };
    Some(base.join("doc/fastwisper/README.md"))
}

/// Item Ajuda: abre o guia local no app padrão do sistema (`xdg-open`).
/// Só esse arquivo; nada de URL nem caminho vindo da webview.
#[tauri::command]
pub async fn open_guide() -> Result<(), String> {
    let path = guide_path(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
        .filter(|path| path.is_file())
        .ok_or("O guia não está instalado. Reinstale com packaging/omarchy/install.sh.")?;
    let mut child = Process::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Não foi possível abrir o guia (xdg-open ausente?).".to_owned())?;
    // Recolhe o filho fora da thread da UI, inclusive se o abridor demorar.
    tauri::async_runtime::spawn_blocking(move || child.wait());
    Ok(())
}

/// URLs exatas da tela Sobre; a webview não pode abrir outro destino.
const ABOUT_URLS: [&str; 2] = [
    "https://github.com/rodrigo-devv/FastWisper",
    "https://github.com/rodrigo-devv",
];

fn about_link_command(url: &str) -> Result<Process, String> {
    if !ABOUT_URLS.contains(&url) {
        return Err("Link não permitido.".into());
    }
    let mut command = Process::new("xdg-open");
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

/// Só abre os dois links de Sobre, com argv sem shell. O filho é recolhido
/// fora da UI; o hub não faz nenhuma requisição ao destino.
#[tauri::command]
pub async fn open_about_link(url: String) -> Result<(), String> {
    let mut child = about_link_command(&url)?
        .spawn()
        .map_err(|_| "Não foi possível abrir o navegador (xdg-open ausente?).".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_links_are_exact_and_use_one_argument_without_a_shell() {
        for url in ABOUT_URLS {
            let command = about_link_command(url).expect("link permitido");
            assert_eq!(command.get_program(), "xdg-open");
            assert_eq!(command.get_args().collect::<Vec<_>>(), [url]);
        }
        for url in [
            "https://example.com",
            "https://github.com/rodrigo-devv/",
            "https://github.com/rodrigo-devv/FastWisper/issues",
            "https://github.com/rodrigo-devv?tab=repositories",
            "https://github.com/rodrigo-devv#profile",
            "https://github.com/rodrigo-devv.evil.test",
            "https://github.com/rodrigo-devv;touch /tmp/unwanted",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "--help",
            "",
        ] {
            assert!(about_link_command(url).is_err(), "{url}");
        }
    }

    #[test]
    fn guide_is_where_the_installer_puts_it() {
        assert_eq!(
            guide_path(Some("/d".into()), Some("/h".into())),
            Some(PathBuf::from("/d/doc/fastwisper/README.md"))
        );
        assert_eq!(
            guide_path(Some("".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.local/share/doc/fastwisper/README.md"))
        );
        assert_eq!(guide_path(None, None), None);
        let install = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packaging/omarchy/install.sh"),
        )
        .expect("install.sh");
        assert!(install.contains("\"$data_root/doc/fastwisper/README.md\""));
    }
}
