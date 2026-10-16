//! Páginas de Configurações. Cada campo do hub é uma chave de `fastwisper
//! config` (catálogo em `src/lib/settings/fields.json`): gravar passa por
//! `fw_core::config_edit` (mesma biblioteca e trava da CLI) e depois por
//! `reload_config`. O resto (microfones, sons, modelos, Ollama, reinício)
//! roda a própria CLI `fastwisper`, então toda opção do hub existe nela.
//!
//! A webview só recebe projeções: valores das chaves do catálogo, níveis do
//! microfone e estados. Nunca texto ditado, título de janela ou a config
//! inteira.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fw_core::client::{self, ClientError, Reload};
use fw_core::config::{Config, RESTART_KEYS};
use fw_core::config_edit;
use fw_core::daemon::DaemonPaths;
use fw_core::ipc::{MeterEvent, Request};
use fw_core::models::{self, MANIFEST};
use fw_core::session::{Command, StateName};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

/// Catálogo de campos, o mesmo que as páginas usam.
const CATALOG: &str = include_str!("../../src/lib/settings/fields.json");

#[derive(Deserialize)]
struct CatalogField {
    key: String,
    #[serde(default)]
    kind: String,
}

fn catalog_keys() -> Vec<String> {
    serde_json::from_str::<Vec<CatalogField>>(CATALOG)
        .map(|fields| {
            fields
                .into_iter()
                .filter(|f| f.kind != "action")
                .map(|f| f.key)
                .collect()
        })
        .unwrap_or_default()
}

fn in_catalog(key: &str) -> bool {
    catalog_keys().iter().any(|k| k == key)
}

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| "Falha interna do hub".to_owned())?
}

// ---------------------------------------------------------------- config

#[derive(Serialize)]
pub struct SettingsSnapshot {
    /// Valor em vigor de cada chave do catálogo.
    values: BTreeMap<String, serde_json::Value>,
    defaults: BTreeMap<String, serde_json::Value>,
    /// Chaves que o daemon só aplica ao reiniciar.
    restart_keys: Vec<&'static str>,
}

fn project(config: &Config) -> BTreeMap<String, serde_json::Value> {
    let keys = catalog_keys();
    config
        .entries()
        .into_iter()
        .filter(|(key, _)| keys.contains(key))
        .filter_map(|(key, value)| Some((key, serde_json::to_value(value).ok()?)))
        .collect()
}

fn snapshot(path: &Path) -> Result<SettingsSnapshot, String> {
    let config = Config::load_path(path, false).map_err(|e| e.to_string())?;
    Ok(SettingsSnapshot {
        values: project(&config),
        defaults: project(&Config::default()),
        restart_keys: RESTART_KEYS.to_vec(),
    })
}

fn config_path() -> Result<PathBuf, String> {
    Config::default_path().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn settings_get() -> Result<SettingsSnapshot, String> {
    blocking(|| snapshot(&config_path()?)).await
}

/// O que aconteceu depois de gravar.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SaveOutcome {
    /// O daemon aplicou (o que não exige reinício).
    Applied { restart_required: Vec<String> },
    /// Há um ditado em andamento: vale quando ele terminar.
    Pending { restart_required: Vec<String> },
    /// Daemon parado: vale no próximo início.
    NotRunning,
    /// Daemon antigo, sem `reload_config`: precisa reiniciar.
    Outdated,
    /// O daemon recusou a config (a gravada fica; a anterior segue valendo).
    Refused { message: String },
}

/// Valida e grava `changes` juntas (com a trava de escrita) e pede ao
/// daemon de `socket` para reler.
fn save(
    path: &Path,
    socket: &Path,
    changes: Vec<(String, serde_json::Value)>,
) -> Result<SaveOutcome, String> {
    if changes.is_empty() {
        return Err("Nada para salvar".into());
    }
    let mut values = Vec::new();
    for (key, json) in changes {
        if !in_catalog(&key) {
            return Err(format!("{key} não é uma opção do hub"));
        }
        values.push((
            key.clone(),
            config_edit::value_from_json(&key, &json).map_err(|e| e.to_string())?,
        ));
    }
    let hotkeys_changed = values
        .iter()
        .any(|(key, _)| fw_core::hotkeys::Action::from_config_key(key).is_some());
    config_edit::update(path, values).map_err(|e| e.to_string())?;
    // Atalho mudado por aqui também chega ao Hyprland; só na config da
    // sessão (a padrão), nunca numa config de teste.
    if hotkeys_changed && Config::default_path().is_ok_and(|default| default == path) {
        let config = Config::load_path(path, false).map_err(|e| e.to_string())?;
        fw_core::hotkeys::apply_default(&config.hotkey, true).map_err(|e| e.to_string())?;
    }
    let id = format!("hub-{}", std::process::id());
    Ok(match client::reload_config(socket, &id) {
        Ok(Reload::Done(report)) if report.pending => SaveOutcome::Pending {
            restart_required: report.restart_required,
        },
        Ok(Reload::Done(report)) => SaveOutcome::Applied {
            restart_required: report.restart_required,
        },
        Ok(Reload::NotRunning) | Err(ClientError::NotRunning(_)) => SaveOutcome::NotRunning,
        Ok(Reload::Outdated) => SaveOutcome::Outdated,
        Ok(Reload::Refused(message)) => SaveOutcome::Refused { message },
        Err(e) => SaveOutcome::Refused {
            message: e.to_string(),
        },
    })
}

fn socket() -> Result<PathBuf, String> {
    DaemonPaths::from_env()
        .map(|p| p.socket)
        .map_err(|_| "Runtime do daemon indisponível".to_owned())
}

/// Uma ou mais chaves do catálogo, validadas juntas.
#[tauri::command]
pub async fn settings_set(
    changes: Vec<(String, serde_json::Value)>,
) -> Result<SaveOutcome, String> {
    blocking(move || save(&config_path()?, &socket()?, changes)).await
}

// ------------------------------------------------------------------- CLI

/// A CLI instalada junto do hub; em desenvolvimento, `FASTWISPER_CLI` ou
/// (só no build de depuração) a do `PATH`. O hub instalado nunca roda uma
/// `fastwisper` qualquer do `PATH`: sem a irmã, a chamada falha com
/// "reinstale".
fn cli() -> PathBuf {
    if let Some(path) = std::env::var_os("FASTWISPER_CLI").filter(|v| !v.is_empty()) {
        return PathBuf::from(path);
    }
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("fastwisper")));
    match sibling {
        Some(path) if path.is_file() => path,
        _ if cfg!(debug_assertions) => PathBuf::from("fastwisper"),
        // Caminho inexistente: a execução falha e o hub pede para reinstalar.
        sibling => sibling.unwrap_or_default(),
    }
}

/// Última linha útil do stderr da CLI, sem o prefixo `fastwisper: `.
fn cli_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches("fastwisper: ").to_owned())
        .unwrap_or_else(|| "a CLI do FastWisper falhou".to_owned())
}

fn run_cli(cli: &Path, args: &[&str]) -> Result<String, String> {
    let output = Process::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "CLI `fastwisper` não encontrada; reinstale o FastWisper".to_owned())?;
    if !output.status.success() {
        return Err(cli_error(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[tauri::command]
pub async fn daemon_restart() -> Result<(), String> {
    blocking(|| run_cli(&cli(), &["restart"]).map(drop)).await
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Microphone {
    node_name: String,
    description: String,
    chosen: bool,
}

/// `fastwisper audio sources --json`. Com `audio.device` vazio ou que não
/// casa, a CLI ainda imprime a lista e sai com erro: a lista vale.
fn microphones_from(cli: &Path) -> Result<Vec<Microphone>, String> {
    let output = Process::new(cli)
        .args(["audio", "sources", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "CLI `fastwisper` não encontrada; reinstale o FastWisper".to_owned())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|l| l.starts_with('['));
    match line {
        Some(line) => serde_json::from_str(line).map_err(|_| "Lista de microfones ilegível".into()),
        None => Err(cli_error(&output.stderr)),
    }
}

#[tauri::command]
pub async fn microphones() -> Result<Vec<Microphone>, String> {
    blocking(|| microphones_from(&cli())).await
}

#[tauri::command]
pub async fn sound_test(volume: f32) -> Result<(), String> {
    if !(0.0..=1.0).contains(&volume) {
        return Err("Volume fora de 0 a 100%".into());
    }
    blocking(move || {
        run_cli(
            &cli(),
            &["audio", "test-sound", "--volume", &volume.to_string()],
        )
        .map(drop)
    })
    .await
}

/// O teste faz `press` + `cancel` de verdade: fora do repouso, o cancel
/// descartaria o ditado em andamento (mãos livres gravando, transcrição).
pub(crate) const MIC_TEST_BUSY: &str =
    "Há um ditado em andamento; termine-o antes de testar o microfone";

/// Só em repouso. Daemon parado ou inacessível: a CLI dá o próprio erro.
fn mic_test_allowed(socket: &Path) -> Result<(), String> {
    match client::send(socket, &Request::new("hub-mic-test", Command::Status)) {
        Ok(response) if response.state.is_some_and(|state| state != StateName::Idle) => {
            Err(MIC_TEST_BUSY.to_owned())
        }
        _ => Ok(()),
    }
}

/// Grava 3 s pelo daemon e cancela (`fastwisper audio test`); o medidor
/// ao vivo mostra o nível enquanto isso. Devolve o pico.
#[tauri::command]
pub async fn mic_test() -> Result<f32, String> {
    blocking(|| {
        mic_test_allowed(&socket()?)?;
        let out = run_cli(&cli(), &["audio", "test", "--secs", "3", "--json"])?;
        out.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find_map(|v| v.get("peak")?.as_f64())
            .map(|p| p as f32)
            .ok_or_else(|| "O teste não informou o nível".into())
    })
    .await
}

// ------------------------------------------------------------- medidor

/// Geração do medidor: abrir de novo ou fechar invalida a thread anterior.
#[derive(Default)]
pub struct MeterState {
    generation: AtomicU64,
    stream: Mutex<Option<UnixStream>>,
}

#[derive(Clone, Serialize)]
struct MeterFrame {
    level: f32,
    recording: bool,
}

fn stop_stream(state: &MeterState) {
    if let Some(stream) = state
        .stream
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Assina o stream `meter` do daemon e repassa só o nível (evento
/// `meter`), reconectando enquanto a página estiver aberta.
#[tauri::command]
pub fn meter_start(app: AppHandle, state: State<'_, Arc<MeterState>>) -> Result<(), String> {
    let state = Arc::clone(&state);
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    stop_stream(&state);
    let socket = socket()?;
    std::thread::Builder::new()
        .name("hub-meter".into())
        .spawn(move || {
            while state.generation.load(Ordering::SeqCst) == generation {
                if let Ok(mut stream) = UnixStream::connect(&socket)
                    && stream
                        .write_all(&Request::new("hub-meter", Command::Meter).to_line())
                        .is_ok()
                    && let Ok(clone) = stream.try_clone()
                {
                    {
                        // Sob a trava: um `meter_stop` que chegou antes
                        // desta conexão não deixa a thread para trás.
                        let mut slot = state.stream.lock().unwrap_or_else(|e| e.into_inner());
                        if state.generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        *slot = Some(clone);
                    }
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { break };
                        if state.generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Ok(event) = serde_json::from_str::<MeterEvent>(&line) {
                            let _ = app.emit(
                                "meter",
                                MeterFrame {
                                    level: event.level,
                                    recording: event.state == StateName::Recording,
                                },
                            );
                        }
                    }
                }
                let _ = app.emit(
                    "meter",
                    MeterFrame {
                        level: 0.0,
                        recording: false,
                    },
                );
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .map_err(|_| "Não foi possível abrir o medidor".to_owned())?;
    Ok(())
}

#[tauri::command]
pub fn meter_stop(state: State<'_, Arc<MeterState>>) {
    state.generation.fetch_add(1, Ordering::SeqCst);
    stop_stream(&state);
}

// ---------------------------------------------------------- transcrição

#[derive(Serialize)]
pub struct AsrModel {
    id: &'static str,
    size_mb: u64,
    note: &'static str,
    license: &'static str,
    /// Caminho que `asr.model` recebe ao usar este modelo.
    path: String,
    installed: bool,
    active: bool,
}

/// Manifesto e o que já está no store (sem rede, sem hash: só tamanho).
#[tauri::command]
pub async fn asr_models() -> Result<Vec<AsrModel>, String> {
    blocking(|| {
        let config = Config::load_path(&config_path()?, false).map_err(|e| e.to_string())?;
        let dir = models::models_dir().ok_or("Pasta de dados indisponível (HOME)")?;
        let configured = config.asr.model_path();
        Ok(MANIFEST
            .iter()
            .map(|m| {
                let path = dir.join(m.file_name);
                AsrModel {
                    id: m.id,
                    size_mb: m.size / 1_000_000,
                    note: m.note,
                    license: m.license,
                    installed: std::fs::metadata(&path).is_ok_and(|meta| meta.len() == m.size),
                    active: configured == path,
                    path: path.display().to_string(),
                }
            })
            .collect())
    })
    .await
}

#[derive(Clone, Serialize)]
struct PullProgress {
    id: String,
    progress: u64,
}

/// `fastwisper models pull <id> --json`, com o progresso no evento
/// `model-progress`.
#[tauri::command]
pub async fn asr_model_pull(app: AppHandle, id: String) -> Result<(), String> {
    if models::find(&id).is_none() {
        return Err("Modelo fora do manifesto".into());
    }
    blocking(move || {
        let mut child = Process::new(cli())
            .args(["models", "pull", &id, "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "CLI `fastwisper` não encontrada; reinstale o FastWisper".to_owned())?;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let progress = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("progress")?.as_u64());
                if let Some(progress) = progress {
                    let _ = app.emit(
                        "model-progress",
                        PullProgress {
                            id: id.clone(),
                            progress,
                        },
                    );
                }
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|_| "O download foi interrompido".to_owned())?;
        if !output.status.success() {
            return Err(cli_error(&output.stderr));
        }
        Ok(())
    })
    .await
}

/// GPUs do mesmo backend Vulkan usado pelo daemon, sem carregar modelos.
#[tauri::command]
pub async fn asr_gpus() -> Result<Vec<String>, String> {
    blocking(|| {
        let out = run_cli(&cli(), &["gpus", "--json"])?;
        serde_json::from_str(out.trim()).map_err(|_| "Lista de GPUs ilegível".into())
    })
    .await
}

// ------------------------------------------------------------- IA local

/// Modelos do Ollama **local** (`fastwisper ollama models --json`: a CLI
/// recusa endpoint fora do loopback e esconde modelos de nuvem).
#[tauri::command]
pub async fn ollama_models() -> Result<Vec<String>, String> {
    blocking(|| {
        let out = run_cli(&cli(), &["ollama", "models", "--json"])?;
        serde_json::from_str(out.trim()).map_err(|_| "Lista do Ollama ilegível".into())
    })
    .await
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudState {
    /// Escolha na config (`llm.ollama_cloud`).
    cloud: bool,
    /// O que o Ollama local informa; `None` com o motivo em `error`.
    disabled: Option<bool>,
    error: Option<String>,
}

fn cloud_state_from(cli: &Path) -> Result<CloudState, String> {
    let out = run_cli(cli, &["ollama", "--json"])?;
    let json: serde_json::Value =
        serde_json::from_str(out.trim()).map_err(|_| "Estado do Ollama ilegível".to_owned())?;
    Ok(CloudState {
        cloud: json["cloud"].as_bool().unwrap_or(false),
        disabled: json["ollama"]["disabled"].as_bool(),
        error: json["ollama"]["error"].as_str().map(str::to_owned),
    })
}

#[tauri::command]
pub async fn ollama_cloud() -> Result<CloudState, String> {
    blocking(|| cloud_state_from(&cli())).await
}

/// Argumentos de `fastwisper ollama online|offline`: sem terminal no hub,
/// os passos de administrador vão pelo `pkexec` (janela do polkit), nunca
/// por `sudo`.
fn cloud_args(online: bool) -> [&'static str; 3] {
    [
        "ollama",
        if online { "online" } else { "offline" },
        "--pkexec",
    ]
}

#[tauri::command]
pub async fn ollama_cloud_set(online: bool) -> Result<CloudState, String> {
    blocking(move || {
        let cli = cli();
        run_cli(&cli, &cloud_args(online))?;
        cloud_state_from(&cli)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes() -> Vec<(PathBuf, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
        let mut files = Vec::new();
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("pasta do hub") {
                let path = entry.expect("entrada").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "svelte" || e == "ts") {
                    let text = std::fs::read_to_string(&path).expect("fonte");
                    files.push((path, text));
                }
            }
        }
        files
    }

    /// Chaves citadas nas páginas: `key="x"`, `saveKey('x'`, `fieldDef('x'`
    /// e `['x',`.
    fn keys_in(text: &str) -> Vec<String> {
        let mut keys = Vec::new();
        for marker in ["key=\"", "saveKey('", "fieldDef('", "['"] {
            for (at, _) in text.match_indices(marker) {
                let rest = &text[at + marker.len()..];
                let Some(end) = rest.find(['"', '\'']) else {
                    continue;
                };
                let key = &rest[..end];
                let looks_like_key = key == "snippets"
                    || key.split_once('.').is_some_and(|(a, b)| {
                        !a.is_empty()
                            && !b.is_empty()
                            && key
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c == '_' || c == '.')
                    });
                if looks_like_key {
                    keys.push(key.to_owned());
                }
            }
        }
        keys
    }

    #[test]
    fn every_hub_field_is_a_fastwisper_config_key() {
        let defaults = Config::default();
        let keys = catalog_keys();
        assert!(keys.len() > 20, "catálogo lido: {keys:?}");
        for key in &keys {
            // A mesma chave funciona em `fastwisper config get/set`.
            let value = defaults
                .get(key)
                .unwrap_or_else(|| panic!("{key} não existe em `fastwisper config list`"));
            let json = serde_json::to_value(&value).expect("json");
            config_edit::value_from_json(key, &json)
                .unwrap_or_else(|e| panic!("{key}: o padrão não volta pelo hub: {e}"));
        }
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "chave repetida no catálogo");
    }

    /// C-05: a dica "Padrão: X" dos campos de valor único vem do
    /// `Config::default()` que o `settings_get` projeta, nunca de texto
    /// escrito por campo; as opções do catálogo cobrem o padrão.
    #[test]
    fn single_value_fields_have_a_real_default_for_the_hint() {
        let fields: Vec<serde_json::Value> = serde_json::from_str(CATALOG).expect("catálogo");
        let defaults = project(&Config::default());
        let mut hinted = 0;
        for field in &fields {
            let key = field["key"].as_str().expect("key");
            if !["toggle", "slider", "select", "number", "text"]
                .contains(&field["kind"].as_str().expect("kind"))
            {
                continue;
            }
            let value = defaults
                .get(key)
                .unwrap_or_else(|| panic!("{key}: sem padrão no snapshot"));
            assert!(
                value.is_boolean()
                    || value.is_number()
                    || value.as_str().is_some_and(|s| !s.is_empty()),
                "{key}: padrão {value} não vira dica"
            );
            if let Some(options) = field["options"].as_array() {
                assert!(
                    options.iter().any(|option| option["value"] == *value),
                    "{key}: padrão {value} fora das opções"
                );
            }
            hinted += 1;
        }
        assert!(hinted >= 15, "campos com dica: {hinted}");
    }

    #[test]
    fn theme_catalog_documents_dynamic_values_accepted_by_the_cli_and_bridge() {
        let fields: Vec<serde_json::Value> = serde_json::from_str(CATALOG).expect("catálogo");
        let field = fields
            .iter()
            .find(|field| field["key"] == "hub.theme")
            .expect("tema");
        assert_eq!(field["dynamic_options"], "omarchy_themes");
        assert_eq!(
            field["value_pattern"],
            "^(auto|dark|light|omarchy:[a-zA-Z0-9_-]{1,128})$"
        );
        for value in field["options"]
            .as_array()
            .expect("paletas")
            .iter()
            .map(|option| option["value"].as_str().expect("valor"))
            .chain(["omarchy:tokyo-night", "omarchy:Custom_82"])
        {
            let changes = vec![(
                "hub.theme".to_owned(),
                config_edit::value_from_json("hub.theme", &serde_json::json!(value))
                    .expect("bridge"),
            )];
            let path = std::path::Path::new("fixture.toml");
            let text = config_edit::set_many("", changes, path).expect("mesma validação da CLI");
            let config = Config::parse(&text, path).expect("config");
            assert_eq!(config.get("hub.theme"), Some(value.into()));
        }
    }

    #[test]
    fn every_key_used_by_a_page_is_in_the_catalog_and_every_field_is_shown() {
        let keys = catalog_keys();
        let mut used = Vec::new();
        for (path, text) in routes() {
            for key in keys_in(&text) {
                assert!(
                    keys.contains(&key),
                    "{} usa {key}, fora do catálogo",
                    path.display()
                );
                used.push(key);
            }
        }
        let fields: Vec<serde_json::Value> = serde_json::from_str(CATALOG).expect("catálogo");
        for field in fields {
            // H2 implementa estas duas abas usando os componentes da H1.
            if matches!(field["page"].as_str(), Some("vibe-coding" | "experimental"))
                || field["kind"] == "action"
            {
                continue;
            }
            let key = field["key"].as_str().expect("chave");
            assert!(
                used.iter().any(|used| used == key),
                "{key} não aparece em nenhuma página"
            );
        }
    }

    #[test]
    fn modal_tabs_map_every_spec_field_to_config() {
        let fields: Vec<serde_json::Value> = serde_json::from_str(CATALOG).expect("catálogo");
        for (page, expected) in [
            (
                "geral",
                vec![
                    "hub.theme",
                    "ui.language",
                    "hotkey.dictate",
                    "hotkey.hands_free",
                    "hotkey.paste_last",
                    "audio.device",
                    "asr.languages",
                    "hotkey.max_recording_secs",
                    "audio.sounds",
                    "audio.sound_volume",
                    "overlay.enabled",
                ],
            ),
            (
                "sistema",
                vec![
                    "asr.model",
                    "asr.gpu",
                    "llm.enabled",
                    "llm.model",
                    "llm.style",
                    "llm.timeout_ms",
                    "llm.skip_below_words",
                    "performance.vram",
                    "performance.unload_after_secs",
                    "performance.game_llm",
                ],
            ),
            (
                "privacidade",
                vec![
                    "llm.ollama_cloud",
                    "inject.clipboard_history",
                    "llm.engine",
                    "llm.endpoint",
                    "llm.remote_consent",
                    "llm.api_key_file",
                ],
            ),
        ] {
            let mut actual: Vec<_> = fields
                .iter()
                .filter(|field| field["page"] == page && field["kind"] != "action")
                .map(|field| field["key"].as_str().expect("key"))
                .collect();
            let mut expected = expected;
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "campos esperados na aba {page}");
            for key in actual {
                assert!(Config::default().get(key).is_some(), "{key} existe na CLI");
            }
        }
        let action = fields
            .iter()
            .find(|field| field["key"] == "history.clear")
            .expect("ação no catálogo");
        assert_eq!(action["kind"], "action");
        assert_eq!(action["cli"], "history clear");
        assert_eq!(action["command"], "history_clear");
        assert!(!in_catalog("history.clear"), "ação não é config gravável");
    }

    #[test]
    fn save_writes_through_config_edit_and_refuses_unknown_keys() {
        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("config.toml");
        let socket = dir.path().join("sem-daemon.sock");
        std::fs::write(&path, "# minha config\n[llm]\nstyle = \"casual\" # meu\n").expect("config");
        let outcome = save(
            &path,
            &socket,
            vec![
                ("llm.style".into(), serde_json::json!("formal")),
                ("audio.sound_volume".into(), serde_json::json!(1)),
            ],
        )
        .expect("salva");
        assert_eq!(outcome, SaveOutcome::NotRunning);
        let text = std::fs::read_to_string(&path).expect("config");
        assert!(
            text.starts_with("# minha config\n[llm]\nstyle = \"formal\" # meu\n"),
            "{text}"
        );
        let config = Config::load_path(&path, true).expect("válida");
        assert_eq!(config.audio.sound_volume, 1.0);

        // Fora do catálogo (ex.: chave de API por variável), inválido e
        // remoto sem consentimento: nada é gravado.
        for (key, value) in [
            ("llm.api_key_env", serde_json::json!("X")),
            ("hotkey.max_recording_secs", serde_json::json!(0)),
            ("llm.endpoint", serde_json::json!("https://api.example.com")),
        ] {
            assert!(
                save(&path, &socket, vec![(key.into(), value)]).is_err(),
                "{key}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).expect("config"), text);
    }

    #[test]
    fn send_now_switch_and_phrases_save_like_the_cli() {
        let fields: Vec<serde_json::Value> = serde_json::from_str(CATALOG).expect("catálogo");
        let kind = |key: &str| {
            fields
                .iter()
                .find(|field| field["key"] == key)
                .map(|field| field["kind"].clone())
        };
        assert_eq!(kind("inject.send_now"), Some("toggle".into()));
        assert_eq!(kind("inject.send_now_phrases"), Some("phrases".into()));

        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("config.toml");
        let socket = dir.path().join("sem-daemon.sock");
        std::fs::write(&path, "# minha config\n").expect("config");
        // O que o `PhrasesEditor` grava: a tabela inteira, um idioma mudado.
        let phrases = serde_json::json!({"pt": ["enviar agora", "manda ver"], "en": ["send now"]});
        save(
            &path,
            &socket,
            vec![
                ("inject.send_now".into(), serde_json::json!(true)),
                ("inject.send_now_phrases".into(), phrases.clone()),
            ],
        )
        .expect("salva");
        let config = Config::load_path(&path, true).expect("válida");
        assert!(config.inject.send_now);
        assert_eq!(
            config.inject.send_now_phrases.pt,
            ["enviar agora", "manda ver"]
        );
        assert_eq!(
            serde_json::to_value(config.get("inject.send_now_phrases")).expect("json"),
            phrases,
            "o hub lê de volta o mesmo valor que a CLI mostra"
        );

        let text = std::fs::read_to_string(&path).expect("config");
        for bad in [
            serde_json::json!({"pt": [""]}),
            serde_json::json!({"pt": ["enviar, agora"]}),
            serde_json::json!({"fr": ["envoyer"]}),
            serde_json::json!(["enviar agora"]),
        ] {
            assert!(
                save(
                    &path,
                    &socket,
                    vec![("inject.send_now_phrases".into(), bad.clone())]
                )
                .is_err(),
                "{bad}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).expect("config"), text);
    }

    #[test]
    fn snapshot_projects_only_catalog_keys() {
        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\napi_key_env = \"SEGREDO\"\n").expect("config");
        let snap = snapshot(&path).expect("snapshot");
        assert!(!snap.values.contains_key("llm.api_key_env"));
        assert!(!snap.values.contains_key("privacy.store_text"));
        assert_eq!(snap.values["llm.style"], "casual");
        assert_eq!(snap.values.len(), catalog_keys().len());
    }

    /// CLI falsa que anota os argumentos; nada toca o Ollama nem o sistema.
    fn fake_cli(dir: &Path, stdout: &str) -> PathBuf {
        let path = dir.join("fastwisper");
        let log = dir.join("args");
        crate::test_support::write_executable(
            &path,
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncat <<'EOF'\n{stdout}\nEOF\n",
                log.display()
            ),
        );
        path
    }

    #[test]
    fn ollama_cloud_goes_through_the_cli_with_pkexec_never_sudo() {
        let dir = tempfile::tempdir().expect("temporário");
        let cli = fake_cli(
            dir.path(),
            r#"{"cloud":false,"ollama":{"disabled":true,"source":"env"}}"#,
        );
        for online in [false, true] {
            run_cli(&cli, &cloud_args(online)).expect("cli");
        }
        let state = cloud_state_from(&cli).expect("estado");
        assert_eq!(
            state,
            CloudState {
                cloud: false,
                disabled: Some(true),
                error: None
            }
        );
        let args = std::fs::read_to_string(dir.path().join("args")).expect("args");
        assert_eq!(
            args,
            "ollama offline --pkexec\nollama online --pkexec\nollama --json\n"
        );
        assert!(!args.contains("sudo"));
    }

    #[test]
    fn microphones_keep_the_list_even_when_the_choice_fails() {
        let dir = tempfile::tempdir().expect("temporário");
        let cli = fake_cli(
            dir.path(),
            r#"[{"node_name":"alsa_input.usb-mic","description":"USB Mic","media_class":"Audio/Source","chosen":false}]"#,
        );
        let mics = microphones_from(&cli).expect("lista");
        assert_eq!(
            mics,
            [Microphone {
                node_name: "alsa_input.usb-mic".into(),
                description: "USB Mic".into(),
                chosen: false
            }]
        );
        assert_eq!(
            cli_error(b"fastwisper: audio.device = \"\": nenhum\n"),
            "audio.device = \"\": nenhum"
        );
    }

    #[test]
    fn page_scan_finds_keys_only() {
        assert_eq!(
            keys_in(
                "<Field key=\"audio.sounds\" /> saveKey('llm.model', m) [['llm.engine', x]] ['*'] key=\"a\""
            ),
            ["audio.sounds", "llm.model", "llm.engine"]
        );
    }
}

#[cfg(test)]
mod mic_test_tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use fw_core::daemon::Daemon;
    use fw_core::inject::NoDesktop;
    use fw_core::pipeline::NullPipeline;

    use super::*;

    /// "Testar microfone" só roda em repouso: com o mãos livres gravando, o
    /// `cancel` do teste descartaria o ditado.
    #[test]
    fn mic_test_is_refused_outside_idle() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        let paths = DaemonPaths::in_dir(dir.path());
        // Daemon parado: a CLI explica.
        assert_eq!(mic_test_allowed(&paths.socket), Ok(()));
        let _context = runtime.enter();
        let daemon = Daemon::bind(paths.clone()).expect("bind");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server =
            runtime.spawn(
                daemon.run(Arc::new(NullPipeline), Arc::new(NoDesktop), async {
                    let _ = stopped.await;
                }),
            );
        let send = |cmd| client::send(&paths.socket, &Request::new("t", cmd)).expect("daemon");
        assert_eq!(mic_test_allowed(&paths.socket), Ok(()));
        assert!(send(Command::Toggle).ok);
        assert_eq!(
            mic_test_allowed(&paths.socket),
            Err(MIC_TEST_BUSY.to_owned())
        );
        assert_eq!(send(Command::Status).state, Some(StateName::Recording));
        assert!(send(Command::Cancel).ok);
        assert_eq!(mic_test_allowed(&paths.socket), Ok(()));
        stop.send(()).expect("parar");
        runtime.block_on(server).expect("daemon");
    }
}
