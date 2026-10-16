//! `fastwisper config`: lê e muda a config preservando comentários. Depois
//! de gravar, o daemon relê e aplica na hora o que dá (`reload_config`).
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Subcommand;
use fw_core::client::{self, DaemonConfig};
use fw_core::config::{Config, RESTART_KEYS};
use fw_core::config_edit;
use fw_core::daemon::DaemonPaths;
use fw_core::ipc::ReloadReport;

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Cada chave com o valor em vigor (e o padrão, quando é outro).
    List {
        /// Uma lista JSON (chave, valor, padrão, exige reinício).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Valor em vigor de uma chave (ex.: `llm.model`).
    Get {
        key: String,
        /// Valor em JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Muda uma ou mais chaves (texto sem aspas; o resto em TOML: true, 1500,
    /// ["*"]), valida tudo junto, grava preservando comentários e pede ao
    /// daemon para recarregar. Ex.: `set llm.style formal llm.timeout_ms 2000`.
    Set {
        /// Pares `chave valor`.
        #[arg(num_args = 2.., value_names = ["CHAVE", "VALOR"], required = true)]
        pairs: Vec<String>,
        /// Grava atalho (`hotkey.*`) mesmo em conflito com outro bind do Hyprland.
        #[arg(long, visible_alias = "force")]
        yes: bool,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Pede ao daemon para reler a config (depois de editar o arquivo à mão)
    /// e regera os atalhos do Hyprland, se mudaram.
    Reload,
}

pub use fw_core::client::Reload;

/// Pede ao daemon para reler a config.
pub fn request_reload(paths: &DaemonPaths) -> anyhow::Result<Reload> {
    Ok(fw_core::client::reload_config(
        &paths.socket,
        &crate::ctl::request_id(),
    )?)
}

/// Resultado de [`reload_for`].
pub enum ReloadFor {
    Reload(Reload),
    /// `written` não é a config do daemon da sessão: nada foi sinalizado.
    /// `None`: daemon antigo, que não informa o caminho.
    Elsewhere(Option<PathBuf>),
}

/// Pede a recarga só se `written` for o arquivo que o daemon relê (ele
/// informa no `status`). Daemon antigo, que não
/// informa, só é sinalizado para a config padrão, como antes.
pub fn reload_for(paths: &DaemonPaths, written: &Path) -> anyhow::Result<ReloadFor> {
    let uses = match client::daemon_config(&paths.socket, &crate::ctl::request_id())? {
        DaemonConfig::NotRunning => return Ok(ReloadFor::Reload(Reload::NotRunning)),
        DaemonConfig::Uses(path) => Some(path),
        DaemonConfig::Unknown => None,
    };
    let same = match &uses {
        Some(path) => client::same_file(path, written),
        None => client::same_file(&Config::default_path()?, written),
    };
    if !same {
        return Ok(ReloadFor::Elsewhere(uses));
    }
    request_reload(paths).map(ReloadFor::Reload)
}

/// Aviso de [`ReloadFor::Elsewhere`].
pub fn print_elsewhere(written: &Path, daemon: Option<&Path>) {
    match daemon {
        Some(daemon) => crate::human_print!(
            "Salvo em {}; o daemon usa {}: nada aplicado.",
            written.display(),
            daemon.display()
        ),
        None => crate::human_print!(
            "Salvo em {}; o daemon em execução não informa qual config usa: nada aplicado (se for esta, rode `fastwisper config reload`).",
            written.display()
        ),
    }
}

/// Aviso das chaves que só valem reiniciando o daemon, se houver.
pub fn restart_notice(report: &ReloadReport) -> Option<String> {
    (!report.restart_required.is_empty()).then(|| {
        format!(
            "Só vale depois de reiniciar o daemon: {} (fastwisper restart).",
            report.restart_required.join(", ")
        )
    })
}

fn config_path(args: &crate::ConfigArgs) -> anyhow::Result<PathBuf> {
    Ok(args.config.clone().map_or_else(Config::default_path, Ok)?)
}

fn load(args: &crate::ConfigArgs) -> anyhow::Result<Config> {
    Ok(Config::load(args.config.as_deref())?)
}

pub fn run(command: ConfigCommand) -> anyhow::Result<()> {
    match command {
        ConfigCommand::List { json, config } => list(&load(&config)?, json),
        ConfigCommand::Get { key, json, config } => {
            let value = load(&config)?.get(&key).with_context(|| {
                format!("chave desconhecida: {key} (veja `fastwisper config list`)")
            })?;
            if json {
                crate::outln!("{}", serde_json::to_string(&value)?);
            } else {
                crate::outln!("{value}");
            }
            Ok(())
        }
        ConfigCommand::Set { pairs, yes, config } => set(&config_path(&config)?, &pairs, yes),
        ConfigCommand::Reload => reload(),
    }
}

fn list(config: &Config, json: bool) -> anyhow::Result<()> {
    let defaults = Config::default().entries();
    let entries = config.entries().into_iter().zip(defaults);
    if json {
        let rows: Vec<serde_json::Value> = entries
            .map(|((key, value), (_, default))| {
                serde_json::json!({
                    "key": key,
                    "value": value,
                    "default": default,
                    "restart": RESTART_KEYS.contains(&key.as_str()),
                })
            })
            .collect();
        crate::outln!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    for ((key, value), (_, default)) in entries {
        if value == default {
            crate::outln!("{key} = {value}");
        } else {
            crate::human_print!("{key} = {value}  # padrão: {default}");
        }
    }
    Ok(())
}

fn set(path: &Path, pairs: &[String], yes: bool) -> anyhow::Result<()> {
    if !pairs.len().is_multiple_of(2) {
        bail!(
            "use pares `chave valor` (faltou o valor de {})",
            pairs[pairs.len() - 1]
        );
    }
    let mut changes = Vec::new();
    for pair in pairs.chunks(2) {
        changes.push((
            pair[0].clone(),
            config_edit::parse_value(&pair[0], &pair[1])?,
        ));
    }
    let hotkeys = crate::hotkeys::canonicalize(&mut changes);
    if !hotkeys.is_empty() {
        // Inválido ou repetido entre os três é recusado antes de perguntar
        // sobre conflitos com outros programas.
        config_edit::set_many(&config_edit::read(path)?, changes.clone(), path)?;
        crate::hotkeys::confirm_conflicts(&hotkeys, yes)?;
    }
    let shown: Vec<String> = changes
        .iter()
        .map(|(key, value)| format!("{key} = {}", value.to_string().trim()))
        .collect();
    let keys: Vec<String> = changes.iter().map(|(key, _)| key.clone()).collect();
    config_edit::update(path, changes)?;
    crate::human_print!("Salvo: {}", shown.join(", "));
    if !hotkeys.is_empty() {
        crate::hotkeys::apply_after_save(path)?;
    }
    let paths = DaemonPaths::from_env()?;
    match reload_for(&paths, path)? {
        ReloadFor::Elsewhere(daemon) => print_elsewhere(path, daemon.as_deref()),
        ReloadFor::Reload(Reload::NotRunning) => {
            crate::human_print!("O daemon está parado; vale no próximo início.")
        }
        ReloadFor::Reload(Reload::Outdated) => crate::human_print!(
            "O daemon em execução é de uma versão que não recarrega a config: reinicie-o para valer."
        ),
        ReloadFor::Reload(Reload::Refused(message)) => bail!(
            "salvo em {}, mas o daemon recusou a recarga: {message}",
            path.display()
        ),
        ReloadFor::Reload(Reload::Done(report)) => print_report(&report, Some(&keys)),
    }
    Ok(())
}

fn reload() -> anyhow::Result<()> {
    // Atalhos editados à mão: regera o arquivo do Lua e recarrega o Hyprland.
    let hotkeys = Config::load(None)
        .map_err(anyhow::Error::from)
        .and_then(|c| Ok(fw_core::hotkeys::apply_default(&c.hotkey, true)?));
    match hotkeys {
        Ok(fw_core::hotkeys::Applied::Unchanged) => {}
        Ok(applied) => crate::outln!("{}", fw_core::hotkeys::applied_message(&applied)),
        Err(e) => crate::human_error!("Aviso: atalhos não aplicados ({e:#})."),
    }
    match request_reload(&DaemonPaths::from_env()?)? {
        Reload::NotRunning => bail!("o daemon não está rodando"),
        Reload::Outdated => {
            bail!("o daemon em execução não recarrega a config: reinicie-o para valer")
        }
        Reload::Refused(message) => bail!("o daemon recusou a config: {message}"),
        Reload::Done(report) => print_report(&report, None),
    }
    Ok(())
}

/// `keys`: as chaves que `set` mudou; sem elas (`reload`), lista o que mudou.
fn print_report(report: &ReloadReport, keys: Option<&[String]>) {
    if keys.is_none() {
        if report.changed.is_empty() {
            crate::human_print!("Config relida: nada mudou.");
        } else {
            crate::human_print!("Config relida. Mudou: {}.", report.changed.join(", "));
        }
    }
    if report.pending {
        crate::human_print!("Há um ditado em andamento: vale quando ele terminar.");
    } else if keys.is_some_and(|keys| keys.iter().any(|k| !report.restart_required.contains(k))) {
        crate::human_print!("Aplicado no daemon.");
    }
    if let Some(notice) = restart_notice(report) {
        crate::human_print!("{notice}");
    }
}
