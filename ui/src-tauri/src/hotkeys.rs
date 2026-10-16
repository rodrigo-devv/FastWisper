//! Atalhos editáveis (Configurações › Geral › Ditado). A validação, os
//! conflitos (`hyprctl binds -j`, só leitura) e a aplicação no Hyprland são
//! os de `fw_core::hotkeys`, os mesmos de `fastwisper config set hotkey.*`.
//!
//! A webview recebe só os atalhos, os rótulos das teclas e a descrição dos
//! binds em conflito; nunca texto ditado nem título de janela.
use std::path::Path;

use fw_core::config::Config;
use fw_core::config_edit;
use fw_core::hotkeys::{self, Action, Applied, Hotkey, Hyprctl, KEYS, Key, Preview};
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct HotkeyRow {
    action: Action,
    key: &'static str,
    combo: String,
    labels: Vec<&'static str>,
    default: &'static str,
    default_labels: Vec<&'static str>,
}

/// Atalhos em vigor e as teclas que a captura aceita.
#[derive(Debug, Serialize)]
pub struct HotkeysView {
    rows: Vec<HotkeyRow>,
    keys: &'static [Key],
}

fn labels(text: &str) -> Vec<&'static str> {
    text.parse::<Hotkey>()
        .map(|h| h.labels())
        .unwrap_or_default()
}

fn view(config: &Config) -> HotkeysView {
    HotkeysView {
        rows: Action::ALL
            .into_iter()
            .map(|action| {
                let combo = action.get(&config.hotkey).to_owned();
                HotkeyRow {
                    action,
                    key: action.config_key(),
                    labels: labels(&combo),
                    combo,
                    default: action.default_text(),
                    default_labels: labels(action.default_text()),
                }
            })
            .collect(),
        keys: KEYS,
    }
}

fn load(path: &Path) -> Result<Config, String> {
    Config::load_path(path, false).map_err(|e| e.to_string())
}

fn config_path() -> Result<std::path::PathBuf, String> {
    Config::default_path().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn hotkeys_get() -> Result<HotkeysView, String> {
    crate::settings::blocking(|| Ok(view(&load(&config_path()?)?))).await
}

/// Prévia antes de gravar: canônico, rótulos e conflitos. Atalho inválido
/// ou já usado por outra ação do FastWisper volta como erro.
#[tauri::command]
pub async fn hotkey_preview(action: Action, combo: String) -> Result<Preview, String> {
    crate::settings::blocking(move || {
        let config = load(&config_path()?)?;
        hotkeys::preview(&config.hotkey, action, &combo, &Hyprctl::from_env())
    })
    .await
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct HotkeySaved {
    combo: String,
    labels: Vec<&'static str>,
    applied: Applied,
    message: String,
}

/// Grava `combo` para `action` e aplica no Hyprland (arquivo + recarga).
/// Conflito com bind de outro programa só passa com `confirm` (o usuário
/// viu o aviso na prévia).
fn save(
    path: &Path,
    file: &Path,
    hypr: &Hyprctl,
    action: Action,
    combo: &str,
    confirm: bool,
) -> Result<HotkeySaved, String> {
    let config = load(path)?;
    let preview = hotkeys::preview(&config.hotkey, action, combo, hypr)?;
    if !preview.conflicts.is_empty() && !confirm {
        return Err(format!(
            "{} já é usado por {}; confirme para gravar mesmo assim",
            preview.labels.join(" + "),
            preview
                .conflicts
                .iter()
                .map(|c| c.description.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let value = config_edit::value_from_json(action.config_key(), &preview.combo.clone().into())
        .map_err(|e| e.to_string())?;
    config_edit::update(path, vec![(action.config_key().to_owned(), value)])
        .map_err(|e| e.to_string())?;
    let saved = load(path)?;
    let applied = hotkeys::apply(&saved.hotkey, file, Some(hypr)).map_err(|e| e.to_string())?;
    Ok(HotkeySaved {
        message: hotkeys::applied_message(&applied),
        combo: preview.combo,
        labels: preview.labels,
        applied,
    })
}

#[tauri::command]
pub async fn hotkey_save(
    action: Action,
    combo: String,
    confirm: bool,
) -> Result<HotkeySaved, String> {
    crate::settings::blocking(move || {
        let file = hotkeys::file_path().ok_or("Pasta de dados indisponível (HOME)")?;
        save(
            &config_path()?,
            &file,
            &Hyprctl::from_env(),
            action,
            &combo,
            confirm,
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_hyprctl(dir: &Path) -> Hyprctl {
        let script = dir.join("hyprctl");
        crate::test_support::write_executable(
            &script,
            &format!(
                "#!/bin/bash\necho \"$*\" >> '{log}'\ncase \"$*\" in\n  'binds -j') echo '[{{\"modmask\":64,\"key\":\"SPACE\",\"description\":\"Omarchy menu\"}},{{\"modmask\":8,\"key\":\"SPACE\",\"description\":\"FastWisper: dictate (hold)\"}}]' ;;\n  reload) echo ok ;;\n  *) exit 3 ;;\nesac\n",
                log = dir.join("calls").display()
            ),
        );
        Hyprctl::with_program(script)
    }

    #[test]
    fn view_lists_the_three_hotkeys_with_labels_and_the_key_table() {
        let config = Config::parse("[hotkey]\ndictate = \"SUPER+D\"\n", Path::new("c.toml"))
            .expect("config");
        let view = view(&config);
        assert_eq!(view.rows.len(), 3);
        assert_eq!(view.rows[0].key, "hotkey.dictate");
        assert_eq!(view.rows[0].labels, ["Super", "D"]);
        assert_eq!(view.rows[0].default_labels, ["Alt", "Espaço"]);
        assert_eq!(view.rows[1].labels, ["Alt", "Shift", "Espaço"]);
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["rows"][2]["action"], "paste_last");
        assert!(
            json["keys"]
                .as_array()
                .expect("teclas")
                .iter()
                .any(|k| k["name"] == "SPACE" && k["code"] == "Space" && k["label"] == "Espaço")
        );
    }

    #[test]
    fn save_needs_confirmation_for_foreign_conflicts_and_refuses_duplicates() {
        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("config.toml");
        let file = dir.path().join("data/hotkeys.conf");
        std::fs::write(
            &path,
            "# minha config\n[hotkey]\nmax_recording_secs = 60 # meu\n",
        )
        .expect("config");
        let hypr = fake_hyprctl(dir.path());
        let before = std::fs::read_to_string(&path).expect("config");

        let err = save(&path, &file, &hypr, Action::Dictate, "super+space", false)
            .expect_err("conflito sem confirmação");
        assert!(err.contains("Omarchy menu"), "{err}");
        let err =
            save(&path, &file, &hypr, Action::PasteLast, "alt+space", true).expect_err("duplicado");
        assert!(err.contains("Atalho para ditar"), "{err}");
        let err = save(&path, &file, &hypr, Action::Dictate, "shift+space", true)
            .expect_err("sem modificador forte");
        assert!(err.contains("além de Shift"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).expect("config"), before);
        assert!(!file.exists());

        let saved =
            save(&path, &file, &hypr, Action::Dictate, "super+space", true).expect("confirmado");
        assert_eq!(saved.combo, "SUPER+SPACE");
        assert_eq!(saved.applied, Applied::Reloaded);
        let text = std::fs::read_to_string(&path).expect("config");
        assert!(text.starts_with("# minha config\n[hotkey]\nmax_recording_secs = 60 # meu\n"));
        assert!(text.contains("dictate = \"SUPER+SPACE\""), "{text}");
        assert!(
            std::fs::read_to_string(&file)
                .expect("arquivo")
                .contains("dictate SUPER+SPACE 65 133 134\n")
        );
        // Restaurar o padrão regrava o arquivo e recarrega de novo.
        let saved = save(&path, &file, &hypr, Action::Dictate, "ALT+SPACE", false).expect("padrão");
        assert_eq!(saved.labels, ["Alt", "Espaço"]);
        let calls = std::fs::read_to_string(dir.path().join("calls")).expect("log");
        assert_eq!(calls.matches("reload").count(), 2, "{calls}");
    }
}
