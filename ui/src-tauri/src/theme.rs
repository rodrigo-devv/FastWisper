//! Cores do hub: `hub.theme` da config (`auto`, `dark`, `light`, `omarchy:<nome>`) e, no
//! automático, as cores do tema atual do Omarchy. Um observador avisa a
//! webview (evento `theme-changed`) quando a config ou o tema do Omarchy
//! mudam, para o hub trocar de cores sem reiniciar.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fw_core::config::{Config, HubTheme, valid_omarchy_theme_name};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const THEME_EVENT: &str = "theme-changed";

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ThemeColors {
    background: Option<String>,
    foreground: Option<String>,
    accent: Option<String>,
    surface: Option<String>,
}

fn theme_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("omarchy/current/theme/colors.toml"))
}

fn hex_color(value: Option<&toml::Value>) -> Option<String> {
    let color = value?.as_str()?;
    (color.len() == 7
        && color.starts_with('#')
        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
    .then(|| color.to_owned())
}

/// Um `colors.toml` tem poucas linhas: o limite e a exigência de arquivo
/// comum impedem que um arquivo enorme, um FIFO ou um dispositivo (ex.: link
/// para `/dev/zero`) travem ou esgotem a memória do hub.
const MAX_COLORS_BYTES: u64 = 64 * 1024;

fn read_small_file(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_COLORS_BYTES {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_COLORS_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_COLORS_BYTES).then_some(text)
}

/// Só o arquivo de cores; arquivos inválidos não entram na galeria.
fn read_colors(path: &Path) -> Option<ThemeColors> {
    let text = read_small_file(path)?;
    let table = text.parse::<toml::Table>().ok()?;
    Some(ThemeColors {
        background: Some(hex_color(table.get("background"))?),
        foreground: Some(hex_color(table.get("foreground"))?),
        accent: Some(hex_color(table.get("accent"))?),
        surface: hex_color(table.get("lighter_background")),
    })
}

fn user_themes_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|config| config.join("omarchy/themes"))
}

fn theme_roots() -> Vec<PathBuf> {
    [
        Some(PathBuf::from("/usr/share/omarchy/themes")),
        user_themes_path(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Caminhos internos apenas; o DTO não expõe caminhos nem importa CSS/scripts.
/// Os diretórios do usuário prevalecem, inclusive se suas cores forem inválidas.
/// Links simbólicos (do tema ou do `colors.toml`) ficam de fora: o caminho lido
/// nunca sai dos diretórios de temas.
fn catalog_paths(roots: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut paths = BTreeMap::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if valid_omarchy_theme_name(&name) && is_dir {
                paths.insert(name, entry.path().join("colors.toml"));
            }
        }
    }
    paths
}

fn catalog_colors(path: &Path) -> Option<ThemeColors> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.file_type().is_file().then(|| read_colors(path))?
}

#[derive(Serialize)]
pub struct OmarchyTheme {
    name: String,
    colors: ThemeColors,
}

fn list_themes(roots: &[PathBuf]) -> Vec<OmarchyTheme> {
    catalog_paths(roots)
        .into_iter()
        .filter_map(|(name, path)| {
            Some(OmarchyTheme {
                name,
                colors: catalog_colors(&path)?,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn omarchy_themes() -> Result<Vec<OmarchyTheme>, String> {
    tauri::async_runtime::spawn_blocking(|| list_themes(&theme_roots()))
        .await
        .map_err(|_| "Falha ao listar os temas".to_owned())
}

/// `None` indica tema escolhido ausente/inválido: a webview volta ao Automático.
/// O nome é validado antes de consultar o catálogo, sem caminhos vindos da webview.
#[tauri::command]
pub async fn theme_colors(name: Option<String>) -> Result<Option<ThemeColors>, String> {
    if name
        .as_ref()
        .is_some_and(|name| !valid_omarchy_theme_name(name))
    {
        return Err("Nome de tema inválido".into());
    }
    tauri::async_runtime::spawn_blocking(move || match name {
        Some(name) => catalog_colors(&catalog_paths(&theme_roots()).remove(&name)?),
        None => read_colors(&theme_path()?),
    })
    .await
    .map_err(|_| "Falha ao ler o tema".to_owned())
}

#[tauri::command]
pub async fn ui_language() -> Result<fw_core::config::UiLanguage, String> {
    fw_core::config::Config::load(None)
        .map(|config| config.ui.language)
        .map_err(|error| error.to_string())
}

/// `hub.theme` em vigor. Config inválida é erro: o hub mantém as cores que
/// já mostra em vez de piscar para o padrão.
fn read_mode(path: &Path) -> Result<HubTheme, String> {
    Config::load_path(path, false)
        .map(|config| config.hub.theme)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn hub_theme() -> Result<HubTheme, String> {
    tauri::async_runtime::spawn_blocking(|| {
        read_mode(&Config::default_path().map_err(|e| e.to_string())?)
    })
    .await
    .map_err(|_| "Falha ao ler o tema".to_owned())?
}

/// O que identifica uma versão de um arquivo: o destino real (o tema do
/// Omarchy troca por link simbólico), o tamanho e a data.
type Stamp = Option<(PathBuf, u64, SystemTime)>;

fn stamp(path: &Path) -> Stamp {
    let real = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&real).ok()?;
    Some((real, meta.len(), meta.modified().ok()?))
}

fn stamps(paths: &[PathBuf], roots: &[PathBuf]) -> Vec<(PathBuf, Stamp)> {
    // Inclui o catálogo dinâmico: criação, remoção e edição de temas também avisam.
    paths
        .iter()
        .cloned()
        .chain(catalog_paths(roots).into_values())
        .map(|path| {
            let version = stamp(&path);
            (path, version)
        })
        .collect()
}

/// Confere a config e o tema do Omarchy a cada segundo (só metadados) e
/// avisa a webview quando algum muda.
pub fn start_watcher(app: AppHandle) {
    let paths: Vec<PathBuf> = [Config::default_path().ok(), theme_path()]
        .into_iter()
        .flatten()
        .collect();
    let roots = theme_roots();
    let _ = std::thread::Builder::new()
        .name("hub-theme".into())
        .spawn(move || {
            let mut last = stamps(&paths, &roots);
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now = stamps(&paths, &roots);
                if now != last {
                    last = now;
                    let _ = app.emit(THEME_EVENT, ());
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS: &str = "background = \"#101010\"\nforeground = \"#eeeeee\"\naccent = \"#123abc\"\nlighter_background = \"#202020\"\nunknown = \"ignored\"\n";

    #[test]
    fn colors_toml_is_tolerant_and_missing_or_invalid_files_are_skipped() {
        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("colors.toml");
        assert!(read_colors(&path).is_none());
        std::fs::write(&path, COLORS).expect("cores");
        let colors = read_colors(&path).expect("válido");
        assert_eq!(colors.background.as_deref(), Some("#101010"));
        assert_eq!(colors.surface.as_deref(), Some("#202020"));
        // Surface opcional; texto de baixo contraste não é corrigido.
        std::fs::write(
            &path,
            COLORS
                .replace("#eeeeee", "#101010")
                .replace("lighter_background", "unused"),
        )
        .expect("cores");
        let colors = read_colors(&path).expect("válido");
        assert_eq!(colors.background, colors.foreground);
        assert_eq!(colors.surface, None);
        for text in [
            "not toml [",
            "background = 3",
            &COLORS.replace("#123abc", "red"),
        ] {
            std::fs::write(&path, text).expect("inválido");
            assert!(read_colors(&path).is_none());
        }
    }

    #[test]
    fn catalog_is_sorted_user_overrides_factory_and_watcher_sees_removal() {
        let dir = tempfile::tempdir().expect("temporário");
        let roots = [dir.path().join("factory"), dir.path().join("user")];
        assert!(list_themes(&roots).is_empty(), "sem Omarchy");
        for (root, name, colors) in [
            (0, "zeta", COLORS.to_owned()),
            (0, "alpha", COLORS.to_owned()),
            (1, "alpha", COLORS.replace("#101010", "#ffffff")),
            (1, "bad..name", COLORS.to_owned()),
            (1, "invalid", "broken [".into()),
        ] {
            let path = roots[root].join(name);
            std::fs::create_dir_all(&path).expect("tema");
            std::fs::write(path.join("colors.toml"), colors).expect("cores");
        }
        let themes = list_themes(&roots);
        assert_eq!(
            themes
                .iter()
                .map(|theme| theme.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert_eq!(themes[0].colors.background.as_deref(), Some("#ffffff"));
        let before = stamps(&[], &roots);
        std::fs::remove_file(roots[1].join("alpha/colors.toml")).expect("remover");
        assert_ne!(stamps(&[], &roots), before);
        assert_eq!(
            list_themes(&roots).len(),
            1,
            "inválido do usuário não revela tema de fábrica"
        );
        std::fs::remove_dir(roots[1].join("alpha")).expect("remover diretório");
        assert_eq!(
            list_themes(&roots).len(),
            2,
            "fábrica volta após remoção do diretório"
        );
    }

    #[test]
    fn huge_files_fifos_devices_and_symlinks_are_skipped_without_blocking() {
        let dir = tempfile::tempdir().expect("temporário");
        let roots = [dir.path().join("factory"), dir.path().join("user")];
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("fora");
        std::fs::write(outside.join("colors.toml"), COLORS).expect("cores");
        for root in &roots {
            std::fs::create_dir_all(root).expect("raiz");
        }
        let theme = |root: usize, name: &str| {
            let path = roots[root].join(name);
            std::fs::create_dir_all(&path).expect("tema");
            path.join("colors.toml")
        };
        std::fs::write(theme(0, "ok"), COLORS).expect("cores");
        let huge = format!("{COLORS}#{}\n", "x".repeat(MAX_COLORS_BYTES as usize));
        std::fs::write(theme(0, "huge"), huge).expect("enorme");
        let fifo = theme(0, "fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        std::os::unix::fs::symlink("/dev/zero", theme(0, "device")).expect("link");
        std::os::unix::fs::symlink(outside.join("colors.toml"), theme(1, "linked-file"))
            .expect("link");
        std::os::unix::fs::symlink(&outside, roots[1].join("linked-dir")).expect("link");
        let names: Vec<_> = list_themes(&roots)
            .into_iter()
            .map(|theme| theme.name)
            .collect();
        assert_eq!(names, ["ok"]);
        assert!(read_colors(&fifo).is_none(), "FIFO não bloqueia");
        assert!(read_colors(Path::new("/dev/zero")).is_none());
        assert!(stamps(&[], &roots).len() >= 5, "observador só lê metadados");
    }

    #[test]
    fn theme_accepts_only_hex_colors() {
        assert_eq!(
            hex_color(Some(&toml::Value::String("#aB1234".into()))),
            Some("#aB1234".into())
        );
        for value in ["red", "#123", "#１２３４５６", "url(https://example.com)"] {
            assert!(hex_color(Some(&toml::Value::String(value.into()))).is_none());
        }
    }

    #[test]
    fn mode_comes_from_the_config_and_defaults_to_auto() {
        let dir = tempfile::tempdir().expect("temporário");
        let path = dir.path().join("config.toml");
        assert_eq!(read_mode(&path), Ok(HubTheme::Auto), "sem arquivo");
        std::fs::write(&path, "[hub]\ntheme = \"light\"\n").expect("config");
        assert_eq!(read_mode(&path), Ok(HubTheme::Light));
        assert_eq!(
            serde_json::to_value(HubTheme::Light).expect("json"),
            serde_json::json!("light")
        );
        std::fs::write(&path, "[hub]\ntheme = \"sepia\"\n").expect("config");
        assert!(read_mode(&path).is_err(), "inválida: o hub mantém as cores");
    }

    #[test]
    fn a_new_file_or_a_new_theme_link_changes_the_stamp() {
        let dir = tempfile::tempdir().expect("temporário");
        let current = dir.path().join("current");
        let path = current.join("colors.toml");
        assert_eq!(stamp(&path), None);
        for (name, text) in [
            ("a", "background = \"#000000\"\n"),
            ("b", "background = \"#ffffff\"\n"),
        ] {
            std::fs::create_dir(dir.path().join(name)).expect("tema");
            std::fs::write(dir.path().join(name).join("colors.toml"), text).expect("cores");
        }
        std::os::unix::fs::symlink(dir.path().join("a"), &current).expect("link");
        let first = stamp(&path);
        assert!(first.is_some());
        std::fs::remove_file(&current).expect("rm link");
        std::os::unix::fs::symlink(dir.path().join("b"), &current).expect("link");
        assert_ne!(stamp(&path), first, "outro tema, mesmo tamanho");
    }
}
