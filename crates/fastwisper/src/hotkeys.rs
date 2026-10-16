//! `fastwisper hotkeys`: mostra os atalhos, confere um novo antes de gravar
//! e regera o arquivo que o Lua do Hyprland lê. Mudar um atalho é
//! `fastwisper config set hotkey.dictate SUPER+D` (ver `config.rs`).
use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::{Context, bail};
use clap::Subcommand;
use fw_core::config::Config;
use fw_core::hotkeys::{self, Action, Hotkey, Hyprctl, KEYS, Modifier};

#[derive(Debug, Subcommand)]
pub enum HotkeysCommand {
    /// Os três atalhos em vigor (e o padrão, quando é outro).
    List {
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Confere um atalho novo sem gravar: formato, os outros dois atalhos e
    /// os binds do Hyprland (`hyprctl binds -j`, só leitura).
    Check {
        /// dictate, hands_free ou paste_last.
        #[arg(value_parser = parse_action)]
        action: Action,
        /// Modificadores + uma tecla, ex.: SUPER+D.
        hotkey: String,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Regera o arquivo de atalhos a partir da config e recarrega o Hyprland.
    Apply {
        /// Só grava o arquivo (o instalador recarrega depois).
        #[arg(long)]
        no_reload: bool,
    },
    /// Teclas aceitas e os modificadores.
    Keys,
}

fn parse_action(name: &str) -> Result<Action, String> {
    Action::from_name(name)
        .ok_or_else(|| crate::human("ação inválida: use dictate, hands_free ou paste_last"))
}

pub fn run(command: HotkeysCommand) -> anyhow::Result<()> {
    match command {
        HotkeysCommand::List { json, config } => {
            list(&Config::load(config.config.as_deref())?, json)
        }
        HotkeysCommand::Check {
            action,
            hotkey,
            json,
            config,
        } => check(
            &Config::load(config.config.as_deref())?,
            action,
            &hotkey,
            json,
        ),
        HotkeysCommand::Apply { no_reload } => {
            let config = Config::load(None)?;
            let applied = hotkeys::apply_default(&config.hotkey, !no_reload)?;
            crate::outln!("{}", crate::human(&hotkeys::applied_message(&applied)));
            Ok(())
        }
        HotkeysCommand::Keys => {
            let mods: Vec<&str> = Modifier::ALL.iter().map(|m| m.name()).collect();
            crate::human_print!(
                "Modificadores: {} (ao menos um além de SHIFT).",
                mods.join(", ")
            );
            let keys: Vec<&str> = KEYS.iter().map(|k| k.name).collect();
            crate::human_print!("Teclas: {}.", keys.join(" "));
            crate::human_print!("Esc é fixo para cancelar.");
            Ok(())
        }
    }
}

fn list(config: &Config, json: bool) -> anyhow::Result<()> {
    let rows: Vec<serde_json::Value> = Action::ALL
        .into_iter()
        .map(|action| {
            let value = action.get(&config.hotkey);
            let label = value
                .parse::<Hotkey>()
                .map(|h| h.label())
                .unwrap_or_else(|_| value.to_owned());
            serde_json::json!({
                "action": action,
                "key": action.config_key(),
                "hotkey": value,
                "label": label,
                "default": action.default_text(),
            })
        })
        .collect();
    if json {
        crate::outln!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    for (action, row) in Action::ALL.into_iter().zip(&rows) {
        let mut line = format!(
            "{}: {} ({})",
            crate::human(action.label()),
            row["label"]
                .as_str()
                .unwrap_or_default()
                .split(" + ")
                .map(crate::human)
                .collect::<Vec<_>>()
                .join(" + "),
            action.config_key()
        );
        if action.get(&config.hotkey) != action.default_text() {
            line.push_str(&crate::human(&format!(
                "  # padrão: {}",
                action.default_text()
            )));
        }
        crate::outln!("{}", crate::human(&line));
    }
    crate::human_print!("Esc cancela a gravação (fixo).");
    Ok(())
}

fn check(config: &Config, action: Action, text: &str, json: bool) -> anyhow::Result<()> {
    let preview = hotkeys::preview(&config.hotkey, action, text, &Hyprctl::from_env())
        .map_err(anyhow::Error::msg)?;
    if json {
        crate::outln!("{}", serde_json::to_string(&preview)?);
        return Ok(());
    }
    crate::outln!(
        "{}: {} ({})",
        crate::human(action.label()),
        preview
            .labels
            .iter()
            .map(|label| crate::human(label))
            .collect::<Vec<_>>()
            .join(" + "),
        preview.combo
    );
    for conflict in &preview.conflicts {
        crate::human_print!(
            "Conflito: {} já usa {}.",
            conflict.description,
            conflict.combo
        );
    }
    if let Some(note) = &preview.note {
        crate::outln!("{}", crate::human(note));
    } else if preview.conflicts.is_empty() {
        crate::human_print!("Sem conflito com os atalhos do Hyprland.");
    }
    Ok(())
}

/// Atalhos mudados em `config set`: troca o texto digitado pelo canônico
/// (`super + d` → `SUPER+D`) e devolve os que mudaram.
pub fn canonicalize(changes: &mut [(String, toml_edit::Value)]) -> Vec<(Action, Hotkey)> {
    let mut changed = Vec::new();
    for (key, value) in changes.iter_mut() {
        let Some(action) = Action::from_config_key(key) else {
            continue;
        };
        // Inválido segue como está: a validação da config explica o porquê.
        if let Some(hotkey) = value.as_str().and_then(|v| v.parse::<Hotkey>().ok()) {
            *value = toml_edit::Value::from(hotkey.to_string());
            changed.push((action, hotkey));
        }
    }
    changed
}

/// Confere os binds de outros programas antes de gravar. Conflito pede
/// confirmação (ou `--yes`); sem Hyprland, nada a conferir.
pub fn confirm_conflicts(changed: &[(Action, Hotkey)], yes: bool) -> anyhow::Result<()> {
    let binds = match Hyprctl::from_env().binds() {
        Ok(binds) => binds,
        Err(hotkeys::HyprError::NotRunning) => return Ok(()),
        Err(e) => {
            crate::human_error!("Aviso: conflitos com o Hyprland não conferidos ({e}).");
            return Ok(());
        }
    };
    let mut warned = false;
    for (action, hotkey) in changed {
        for bind in hotkeys::conflicts(hotkey, &binds) {
            warned = true;
            crate::human_error!(
                "Conflito: {} ({}) já é usado por \"{}\" no Hyprland; os dois disparam juntos.",
                hotkey.label(),
                crate::human(action.label()),
                bind.summary()
            );
        }
    }
    if warned && !yes {
        if !io::stdin().is_terminal() {
            bail!("atalho em conflito não gravado; use --yes para gravar mesmo assim");
        }
        crate::out!("{}", crate::human("Gravar mesmo assim? [s/N] "));
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(
            answer.trim().to_lowercase().as_str(),
            "s" | "sim" | "y" | "yes"
        ) {
            bail!("nada foi gravado");
        }
    }
    Ok(())
}

/// Depois de gravar atalhos: regera o arquivo e recarrega o Hyprland, mas
/// só para a config padrão (a que o daemon e o Lua da sessão usam).
pub fn apply_after_save(path: &Path) -> anyhow::Result<()> {
    if !is_default_config(path)? {
        crate::human_print!("Config fora do caminho padrão: os atalhos do Hyprland não mudaram.");
        return Ok(());
    }
    let config = Config::load_path(path, false)?;
    let applied = hotkeys::apply_default(&config.hotkey, true).context("aplicar atalhos")?;
    crate::outln!("{}", crate::human(&hotkeys::applied_message(&applied)));
    Ok(())
}

fn is_default_config(path: &Path) -> anyhow::Result<bool> {
    Ok(fw_core::client::same_file(path, &Config::default_path()?))
}
