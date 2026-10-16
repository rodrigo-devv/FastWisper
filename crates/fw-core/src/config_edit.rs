//! Edição da config que preserva comentários, ordem e o resto do TOML, e
//! gravação atômica. Usada pela CLI (`config`, `vram`, `ollama`) e pelo hub.
//!
//! Nada é gravado sem passar por [`Config::parse`]: valor inválido não
//! chega ao disco e a config anterior continua valendo.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;
use toml_edit::{DocumentMut, Item, Value};

use crate::config::{Config, ConfigError};

#[derive(Debug, Error)]
pub enum ConfigEditError {
    #[error("config TOML inválida: {0}")]
    Toml(String),
    #[error("chave desconhecida: {0} (veja `fastwisper config list`)")]
    UnknownKey(String),
    #[error("{0} deve ser uma tabela")]
    NotATable(String),
    #[error("valor inválido para {key}: {message}")]
    Value { key: String, message: String },
    #[error(transparent)]
    Invalid(#[from] ConfigError),
    #[error("falha ao acessar {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ConfigEditError + '_ {
    move |source| ConfigEditError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Texto atual da config; sem arquivo, parte do exemplo comentado.
pub fn read(path: &Path) -> Result<String, ConfigEditError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Ok(crate::setup::EXAMPLE_CONFIG.to_owned())
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

/// `key = value` no texto (`secao.chave`, ou `snippets`), mantendo o
/// comentário da linha e o resto do arquivo. O resultado precisa ser uma
/// config válida; senão, erro e nada muda.
pub fn set(text: &str, key: &str, value: Value, path: &Path) -> Result<String, ConfigEditError> {
    set_many(text, vec![(key.to_owned(), value)], path)
}

/// Várias chaves de uma vez, validadas juntas: para mudanças que só são
/// válidas em conjunto (ligar um provider remoto exige endpoint e
/// consentimento na mesma gravação).
pub fn set_many(
    text: &str,
    changes: Vec<(String, Value)>,
    path: &Path,
) -> Result<String, ConfigEditError> {
    let defaults = Config::default();
    if let Some((key, _)) = changes.iter().find(|(k, _)| defaults.get(k).is_none()) {
        return Err(ConfigEditError::UnknownKey(key.clone()));
    }
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Toml(e.message().to_owned()))?;
    for (key, value) in changes {
        match key.split_once('.') {
            Some((table, leaf)) => {
                if !doc.contains_key(table) {
                    doc[table] = toml_edit::table();
                }
                let entries = doc[table]
                    .as_table_like_mut()
                    .ok_or_else(|| ConfigEditError::NotATable(table.to_owned()))?;
                put(entries, leaf, value);
            }
            // `snippets`: a lista inteira, inclusive no lugar de `[[snippets]]`.
            None => put(doc.as_table_mut(), &key, value),
        }
    }
    let updated = doc.to_string();
    Config::parse(&updated, path)?;
    Ok(updated)
}

/// Lê, troca as chaves e grava atômico (ver [`save`]), segurando a trava
/// de escrita ([`lock`]) do começo ao fim: dois escritores ao mesmo tempo
/// (CLI e hub) não perdem a chave um do outro. O que a CLI (`config set`)
/// e o hub fazem antes de pedir `reload_config`.
pub fn update(path: &Path, changes: Vec<(String, Value)>) -> Result<(), ConfigEditError> {
    let _lock = lock(path)?;
    let updated = set_many(&read(path)?, changes, path)?;
    save(path, &updated)
}

/// Trava exclusiva entre escritores da config, solta ao sair do escopo.
pub struct WriteLock(#[allow(dead_code)] fs::File);

/// `flock` na pasta do arquivo (do destino, se for link): nada de arquivo
/// de trava extra, e o `rename` da gravação atômica não troca o que está
/// travado. Espera quem estiver gravando; a pasta é criada se faltar.
pub fn lock(path: &Path) -> Result<WriteLock, ConfigEditError> {
    let target = resolve(path)?;
    let parent = parent_of(&target);
    fs::create_dir_all(parent).map_err(io_err(parent))?;
    let dir = fs::File::open(parent).map_err(io_err(parent))?;
    rustix::fs::flock(&dir, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| io_err(parent)(e.into()))?;
    Ok(WriteLock(dir))
}

fn resolve(path: &Path) -> Result<PathBuf, ConfigEditError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path).map_err(io_err(path)),
        _ => Ok(path.to_path_buf()),
    }
}

fn parent_of(target: &Path) -> &Path {
    target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn put(entries: &mut dyn toml_edit::TableLike, key: &str, mut value: Value) {
    if let Some(decor) = entries
        .get(key)
        .and_then(Item::as_value)
        .map(|v| v.decor().clone())
    {
        *value.decor_mut() = decor;
    }
    match entries.get_mut(key) {
        Some(entry) => *entry = Item::Value(value),
        None => {
            entries.insert(key, Item::Value(value));
        }
    }
}

/// Valor digitado (CLI) → valor TOML do tipo da chave. Texto entra sem
/// aspas (`llm.model qwen3:8b`); o resto é literal TOML (`true`, `1500`,
/// `0.5`, `["*"]`, `{ "a" = "b" }`).
pub fn parse_value(key: &str, raw: &str) -> Result<Value, ConfigEditError> {
    let expected = Config::default()
        .get(key)
        .ok_or_else(|| ConfigEditError::UnknownKey(key.to_owned()))?;
    let trimmed = raw.trim();
    let quoted = trimmed.starts_with('"') || trimmed.starts_with('\'');
    if matches!(expected, toml::Value::String(_)) && !quoted {
        return Ok(Value::from(raw));
    }
    let invalid = |message: String| ConfigEditError::Value {
        key: key.to_owned(),
        message,
    };
    let value: Value = trimmed
        .parse()
        .map_err(|_| invalid(format!("esperado {}", expected.type_str())))?;
    check_type(key, &expected, value)
}

/// Confere que `value` tem o tipo da chave (inteiro vira float onde a
/// chave é float).
fn check_type(key: &str, expected: &toml::Value, value: Value) -> Result<Value, ConfigEditError> {
    let value = match (expected, value) {
        (toml::Value::Float(_), Value::Integer(n)) => Value::from(*n.value() as f64),
        (_, value) => value,
    };
    let received = match value.type_name() {
        "inline table" => "table",
        name => name,
    };
    if received != expected.type_str() {
        return Err(ConfigEditError::Value {
            key: key.to_owned(),
            message: format!("esperado {}, recebido {received}", expected.type_str()),
        });
    }
    Ok(value)
}

/// Valor vindo do hub (JSON) → valor TOML do tipo da chave. `null` não
/// existe em TOML e é recusado.
pub fn value_from_json(key: &str, json: &serde_json::Value) -> Result<Value, ConfigEditError> {
    let expected = Config::default()
        .get(key)
        .ok_or_else(|| ConfigEditError::UnknownKey(key.to_owned()))?;
    let value = json_to_toml(json).ok_or_else(|| ConfigEditError::Value {
        key: key.to_owned(),
        message: format!("esperado {}", expected.type_str()),
    })?;
    check_type(key, &expected, value)
}

fn json_to_toml(json: &serde_json::Value) -> Option<Value> {
    Some(match json {
        serde_json::Value::Null => return None,
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::from(i),
            None => Value::from(n.as_f64()?),
        },
        serde_json::Value::String(s) => Value::from(s.as_str()),
        serde_json::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            Value::Array(array)
        }
        serde_json::Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (k, v) in map {
                table.insert(k, json_to_toml(v)?);
            }
            Value::InlineTable(table)
        }
    })
}

/// Grava `text` como config em `path` só se ela for válida, de forma
/// atômica (ver [`prepare`]).
pub fn save(path: &Path, text: &str) -> Result<(), ConfigEditError> {
    Config::parse(text, path)?;
    prepare(path, text)?.commit()
}

/// Grava `text` em `path` de forma atômica, sem validar (arquivos que não
/// são a config, como o drop-in do Ollama).
pub fn write_atomic(path: &Path, text: &str) -> Result<(), ConfigEditError> {
    prepare(path, text)?.commit()
}

/// Temporário escrito e sincronizado no mesmo diretório, pronto para
/// substituir o destino com um `rename`.
pub struct Prepared {
    file: tempfile::NamedTempFile,
    target: PathBuf,
}

impl Prepared {
    pub fn commit(self) -> Result<(), ConfigEditError> {
        let target = self.target;
        self.file
            .persist(&target)
            .map(drop)
            .map_err(|e| io_err(&target)(e.error))
    }
}

/// Prepara a gravação de `text` sobre `path`. Link simbólico (dotfiles)
/// grava no arquivo apontado. Permissões: as do dono no arquivo atual, sem
/// grupo/outros (a config pode apontar chaves); arquivo novo fica `0600`.
pub fn prepare(path: &Path, text: &str) -> Result<Prepared, ConfigEditError> {
    let target = resolve(path)?;
    let mode = match fs::metadata(&target) {
        Ok(meta) => meta.permissions().mode() & 0o700,
        Err(_) => 0o600,
    };
    let parent = parent_of(&target);
    fs::create_dir_all(parent).map_err(io_err(parent))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_err(parent))?;
    let tmp = file.path().to_path_buf();
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io_err(&tmp))?;
    file.write_all(text.as_bytes()).map_err(io_err(&tmp))?;
    file.as_file().sync_all().map_err(io_err(&tmp))?;
    Ok(Prepared { file, target })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "config.toml";

    fn edit(text: &str, key: &str, raw: &str) -> Result<String, ConfigEditError> {
        set(text, key, parse_value(key, raw)?, Path::new(PATH))
    }

    #[test]
    fn harness_apps_round_trip_through_cli_and_hub_values() {
        let key = "inject.harness_apps";
        let raw = r#"["fixture.ai.desktop"]"#;
        let value = parse_value(key, raw).expect("lista CLI");
        let path = Path::new("/tmp/fixture-config.toml");
        let updated = set("# fixture\n", key, value, path).expect("config editada");
        let config = Config::parse(&updated, path).expect("config válida");
        assert_eq!(config.inject.harness_apps, ["fixture.ai.desktop"]);
        let json = serde_json::json!(["fixture.ai.desktop"]);
        let value = value_from_json(key, &json).expect("lista hub");
        let from_hub = set("", key, value, path).expect("config hub");
        assert_eq!(
            Config::parse(&from_hub, path).expect("config").inject,
            config.inject
        );
        assert!(parse_value(key, "true").is_err());
        assert!(set("", key, parse_value(key, "[42]").expect("array"), path).is_err());
    }

    #[test]
    fn comments_and_order_are_preserved() {
        let before = "# minha config\n[performance] # perfil\n# escolha pessoal\nvram = 'automatico' # manter\nunload_after_secs = 42\n\n[privacy]\nstore_text = false # privado\n";
        let after = edit(before, "performance.vram", "jogo").expect("válido");
        assert_eq!(after, before.replace("'automatico'", "\"jogo\""));
        let after = edit(&after, "privacy.store_audio", "false").expect("válido");
        assert!(after.starts_with(&before.replace("'automatico'", "\"jogo\"")));
    }

    #[test]
    fn missing_section_and_inline_table_are_supported() {
        for before in [
            "# config\n",
            "performance = { vram = 'automatico', unload_after_secs = 10 } # comentário\n",
        ] {
            let after = edit(before, "performance.vram", "sob_demanda").expect("válido");
            assert!(after.contains("sob_demanda"));
            assert!(after.contains('#'));
        }
        assert!(matches!(
            edit("performance = 42", "performance.vram", "jogo"),
            Err(ConfigEditError::NotATable(_))
        ));
    }

    #[test]
    fn invalid_result_is_refused() {
        // Valor fora do limite, tipo errado e config já inválida no arquivo.
        assert!(matches!(
            edit("", "hotkey.max_recording_secs", "0"),
            Err(ConfigEditError::Invalid(_))
        ));
        assert!(matches!(
            edit("", "audio.sounds", "talvez"),
            Err(ConfigEditError::Value { .. })
        ));
        assert!(matches!(
            edit("", "overlay.position", "top"),
            Err(ConfigEditError::Invalid(_))
        ));
        assert!(edit("[privacy]\nstore_txt = true", "performance.vram", "jogo").is_err());
        assert!(matches!(
            edit("", "llm.modelo", "x"),
            Err(ConfigEditError::UnknownKey(_))
        ));
        assert!(edit("[llm\n", "llm.model", "x").is_err());
    }

    #[test]
    fn values_follow_the_key_type() {
        assert_eq!(
            parse_value("llm.model", "qwen3:8b")
                .expect("texto")
                .as_str(),
            Some("qwen3:8b")
        );
        assert_eq!(
            parse_value("audio.device", "\"1234\"")
                .expect("texto")
                .as_str(),
            Some("1234")
        );
        assert_eq!(
            parse_value("audio.sound_volume", "1")
                .expect("número")
                .as_float(),
            Some(1.0)
        );
        assert!(parse_value("llm.timeout_ms", "rápido").is_err());
        let apps =
            parse_value("inject.copy_only_apps", r#"["org.keepassxc.KeePassXC"]"#).expect("lista");
        assert!(apps.is_array());
        let after = edit(
            "[[snippets]]\ntrigger = \"a\"\nexpansion = \"b\"\n",
            "snippets",
            r#"[{ trigger = "x", expansion = "y" }]"#,
        )
        .expect("snippets");
        let config = Config::parse(&after, Path::new(PATH)).expect("válido");
        assert_eq!(config.snippets.len(), 1);
        assert_eq!(config.snippets[0].trigger, "x");
    }

    #[test]
    fn many_keys_are_validated_together() {
        let remote = vec![
            ("llm.engine".to_owned(), Value::from("openai_compatible")),
            (
                "llm.endpoint".to_owned(),
                Value::from("https://api.example.com"),
            ),
            ("llm.remote_consent".to_owned(), Value::from(true)),
        ];
        // Endpoint remoto sozinho é recusado; com o consentimento junto, vale.
        assert!(
            set(
                "",
                "llm.endpoint",
                Value::from("https://api.example.com"),
                Path::new(PATH)
            )
            .is_err()
        );
        let after = set_many("# minha\n", remote, Path::new(PATH)).expect("válido em conjunto");
        assert!(after.contains("# minha\n"));
        let config = Config::parse(&after, Path::new(PATH)).expect("config");
        assert!(config.llm.remote_consent);
        assert!(matches!(
            set_many(
                "",
                vec![("llm.x".to_owned(), Value::from(1))],
                Path::new(PATH)
            ),
            Err(ConfigEditError::UnknownKey(_))
        ));
    }

    #[test]
    fn json_values_follow_the_key_type() {
        use serde_json::json;
        assert_eq!(
            value_from_json("audio.sound_volume", &json!(1))
                .expect("float")
                .as_float(),
            Some(1.0)
        );
        assert_eq!(
            value_from_json("llm.timeout_ms", &json!(2000))
                .expect("int")
                .as_integer(),
            Some(2000)
        );
        assert!(value_from_json("llm.timeout_ms", &json!("2000")).is_err());
        assert!(value_from_json("audio.sounds", &json!(null)).is_err());
        let replace = value_from_json("dictionary.replace", &json!({"fast whisper": "FastWisper"}))
            .expect("tabela");
        let after = set("", "dictionary.replace", replace, Path::new(PATH)).expect("válido");
        let config = Config::parse(&after, Path::new(PATH)).expect("config");
        assert_eq!(config.dictionary.replace["fast whisper"], "FastWisper");
        let snippets = value_from_json(
            "snippets",
            &json!([{ "trigger": "meu email", "expansion": "eu@example.com" }]),
        )
        .expect("lista");
        let after = set("", "snippets", snippets, Path::new(PATH)).expect("válido");
        assert_eq!(
            Config::parse(&after, Path::new(PATH))
                .expect("config")
                .snippets
                .len(),
            1
        );
        // Lista com item de outro tipo passa no tipo da chave, mas não na config.
        let mixed = value_from_json("inject.paste_apps", &json!(["*", 1])).expect("array");
        assert!(set("", "inject.paste_apps", mixed, Path::new(PATH)).is_err());
    }

    #[test]
    fn save_is_atomic_private_and_refuses_invalid_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "# antes\n").expect("config");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(save(&path, "[hotkey]\nmax_recording_secs = 0\n").is_err());
        assert_eq!(fs::read_to_string(&path).expect("config"), "# antes\n");
        save(&path, "# depois\n").expect("válida");
        assert_eq!(fs::read_to_string(&path).expect("config"), "# depois\n");
        assert_eq!(
            fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
            0o600
        );
        let entries = fs::read_dir(dir.path()).expect("dir").count();
        assert_eq!(entries, 1, "sem temporário sobrando");
    }

    #[test]
    fn concurrent_updates_keep_every_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "# antes\n").expect("config");
        // Cada thread muda uma chave diferente; sem a trava, a leitura de
        // uma e a gravação da outra se cruzam e uma chave se perde.
        let keys = [
            ("llm.timeout_ms", Value::from(2000)),
            ("audio.sounds", Value::from(false)),
            ("llm.style", Value::from("formal")),
            ("performance.unload_after_secs", Value::from(60)),
            ("overlay.enabled", Value::from(false)),
            ("hotkey.max_recording_secs", Value::from(120)),
        ];
        std::thread::scope(|scope| {
            for (key, value) in keys.clone() {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..10 {
                        update(path, vec![(key.to_owned(), value.clone())]).expect("grava");
                    }
                });
            }
        });
        let config = Config::load_path(&path, true).expect("válida");
        assert_eq!(config.llm.timeout_ms, 2000);
        assert!(!config.audio.sounds);
        assert_eq!(config.llm.style, crate::config::Style::Formal);
        assert_eq!(config.performance.unload_after_secs, 60);
        assert!(!config.overlay.enabled);
        assert_eq!(config.hotkey.max_recording_secs, 120);
        assert!(
            fs::read_to_string(&path)
                .expect("config")
                .contains("# antes")
        );
        assert_eq!(
            fs::read_dir(dir.path()).expect("dir").count(),
            1,
            "sem arquivo de trava"
        );
    }

    #[test]
    fn symlinked_config_is_written_through_the_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("dotfiles.toml");
        let link = dir.path().join("config.toml");
        fs::write(&real, "").expect("config");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        save(&link, "# via link\n").expect("válida");
        assert!(
            fs::symlink_metadata(&link)
                .expect("meta")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).expect("real"), "# via link\n");
    }
}
