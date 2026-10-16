//! Config descartável, nada de daemon ou sessão real do dono.
use fw_core::config::{Config, UiLanguage};
use fw_core::config_edit;
use std::path::Path;
use std::process::{Command, Output};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fastwisper"))
        .args(args)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .output()
        .expect("fixture de idioma válida")
}
fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("fixture de idioma válida")
}
#[test]
fn language_defaults_validates_and_has_cli_catalog_parity_without_writing_defaults() {
    let root = tempfile::tempdir().expect("fixture de idioma válida");
    let config = root.path().join("config/fastwisper/config.toml");
    assert_eq!(
        stdout(cli(
            root.path(),
            &["config", "get", "ui.language", "--json"]
        )),
        "\"en\"\n"
    );
    assert!(!config.exists(), "consultar defaults nunca grava a config");
    let fields: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../ui/src/lib/settings/fields.json"))
            .expect("fixture de idioma válida");
    let field = fields
        .iter()
        .find(|f| f["key"] == "ui.language")
        .expect("fixture de idioma válida");
    for (text, language) in [("en", UiLanguage::En), ("pt-BR", UiLanguage::PtBr)] {
        assert!(
            field["options"]
                .as_array()
                .expect("fixture de idioma válida")
                .iter()
                .any(|o| o["value"] == text)
        );
        let updated = config_edit::set(
            "",
            "ui.language",
            config_edit::parse_value("ui.language", text).expect("fixture de idioma válida"),
            &config,
        )
        .expect("fixture de idioma válida");
        let parsed = Config::parse(&updated, &config).expect("fixture de idioma válida");
        assert_eq!(parsed.ui.language, language);
        assert!(Config::restart_required(&Config::default(), &parsed).is_empty());
        std::fs::create_dir_all(config.parent().expect("fixture de idioma válida"))
            .expect("fixture de idioma válida");
        std::fs::write(&config, updated).expect("fixture de idioma válida");
        assert_eq!(
            stdout(cli(
                root.path(),
                &["config", "get", "ui.language", "--json"]
            )),
            format!("\"{text}\"\n")
        );
        let help = stdout(cli(root.path(), &["--help"]));
        assert!(
            help.contains(if language == UiLanguage::En {
                "100% local voice dictation"
            } else {
                "Ditado por voz 100% local"
            }),
            "{help}"
        );
    }
    let before = std::fs::read(&config).expect("fixture de idioma válida");
    let bad = cli(root.path(), &["config", "set", "ui.language", "fr"]);
    assert!(!bad.status.success());
    assert_eq!(
        std::fs::read(&config).expect("fixture de idioma válida"),
        before
    );
}
#[test]
fn json_identifiers_bar_and_follow_contracts_are_independent_of_interface_language() {
    let root = tempfile::tempdir().expect("fixture de idioma válida");
    let config = root.path().join("config/fastwisper/config.toml");
    std::fs::create_dir_all(config.parent().expect("fixture de idioma válida"))
        .expect("fixture de idioma válida");
    let mut snapshots = Vec::new();
    for language in ["en", "pt-BR"] {
        std::fs::write(&config, format!("[ui]\nlanguage = \"{language}\"\n"))
            .expect("fixture de idioma válida");
        let rows: serde_json::Value =
            serde_json::from_str(&stdout(cli(root.path(), &["config", "list", "--json"])))
                .expect("fixture de idioma válida");
        let rows: Vec<_> = rows
            .as_array()
            .expect("fixture de idioma válida")
            .iter()
            .filter(|r| r["key"] != "ui.language")
            .cloned()
            .collect();
        snapshots.push((rows, stdout(cli(root.path(), &["status", "--bar"]))));
        let error = cli(root.path(), &["status"]);
        let stderr = String::from_utf8(error.stderr).expect("fixture de idioma válida");
        assert!(
            stderr.contains(if language == "en" {
                "fastwisperd is not running"
            } else {
                "fastwisperd não está rodando"
            }),
            "{stderr}"
        );
        let localized: serde_json::Value = serde_json::from_str(&stdout(cli(
            root.path(),
            &["status", "--bar", "--localized"],
        )))
        .expect("fixture de idioma válida");
        assert_eq!(localized["class"], "offline");
        assert!(
            localized["tooltip"]
                .as_str()
                .expect("fixture de idioma válida")
                .contains(if language == "en" {
                    "FastWisper stopped"
                } else {
                    "FastWisper parado"
                })
        );
    }
    assert_eq!(snapshots[0], snapshots[1]);
}
#[test]
fn subcommand_and_argument_help_follow_the_interface_language() {
    let root = tempfile::tempdir().expect("fixture de idioma válida");
    let help = stdout(cli(root.path(), &["--help"]));
    assert!(help.contains("Local voice notes (opt-in)"), "{help}");
    assert!(!help.contains("Notas por voz locais"), "{help}");
    let help = stdout(cli(root.path(), &["notes", "--help"]));
    assert!(help.contains("Lists notes with the newest first"), "{help}");
    let config = root.path().join("config/fastwisper/config.toml");
    std::fs::create_dir_all(config.parent().expect("fixture de idioma válida"))
        .expect("fixture de idioma válida");
    std::fs::write(&config, "[ui]\nlanguage = \"pt-BR\"\n").expect("fixture de idioma válida");
    let help = stdout(cli(root.path(), &["--help"]));
    assert!(help.contains("Notas por voz locais (opt-in)"), "{help}");
}

/// Palavras que só existem em português (artigos e preposições curtas que
/// também são inglês, como "a", "do" e "no", ficam de fora).
const PORTUGUESE: &[&str] = &[
    "não",
    "nao",
    "você",
    "está",
    "também",
    "então",
    "para",
    "com",
    "sem",
    "uma",
    "já",
    "ainda",
    "mais",
    "só",
    "pelo",
    "pela",
    "dos",
    "das",
    "nas",
    "aos",
    "ao",
    "que",
    "quando",
    "isso",
    "este",
    "esta",
    "esse",
    "essa",
    "aqui",
    "onde",
    "como",
    "mas",
    "ou",
    "em",
    "de",
    "da",
    "rode",
    "veja",
    "arquivo",
    "padrão",
    "padrões",
    "atalho",
    "atalhos",
    "ditado",
    "modelo",
    "microfone",
    "nuvem",
    "salvo",
    "grava",
    "mostra",
    "lista",
];

/// O que denuncia texto em português numa saída que deveria ser inglês:
/// letras acentuadas ou palavras de [`PORTUGUESE`]. Identificadores (URLs,
/// chaves, `ollama.com`) não se partem nos pontos.
fn portuguese_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in text.lines() {
        // Página do hub com nome em português é identificador estável.
        if line.contains("e.g. microfone") || line.contains("[possible values: inicio") {
            continue;
        }
        let accent = line.chars().any(|c| "áàâãéêíóôõúçÁÀÂÃÉÊÍÓÔÕÚÇ".contains(c));
        let word = line.split_whitespace().any(|token| {
            let token = token
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            PORTUGUESE.contains(&token.as_str())
        });
        if accent || word {
            found.push(line.trim().to_owned());
        }
    }
    found
}

#[test]
fn portuguese_detector_catches_accents_and_words() {
    assert_eq!(
        portuguese_in("Prints the last text\nuse https://ollama.com"),
        Vec::<String>::new()
    );
    assert_eq!(portuguese_in("Imprime o último texto").len(), 1);
    assert_eq!(portuguese_in("Copia o texto para o clipboard").len(), 1);
    assert_eq!(
        portuguese_in("<CHAVE> <VALOR>... `key value`").len(),
        0,
        "só maiúsculas: veja o teste abaixo"
    );
}

/// A-21: `--help` de todos os subcomandos (descobertos na própria ajuda,
/// recursivamente) com o idioma padrão, inglês: nenhuma frase, valor
/// possível ou nome de valor em português.
#[test]
fn every_subcommand_help_is_english_by_default() {
    let root = tempfile::tempdir().expect("fixture de idioma válida");
    let mut pending = vec![Vec::<String>::new()];
    let mut seen = 0;
    let mut problems = Vec::new();
    while let Some(path) = pending.pop() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let help = stdout(cli(root.path(), &args));
        seen += 1;
        for line in portuguese_in(&help) {
            problems.push(format!("fastwisper {}: {line}", path.join(" ")));
        }
        // Nomes de valor em maiúsculas (`<CHAVE>`) não passam pelo filtro de palavras.
        for name in ["<CHAVE>", "<VALOR>", "<AÇÃO>", "<ATALHO>", "<CAMINHO>"] {
            if help.contains(name) {
                problems.push(format!("fastwisper {}: {name}", path.join(" ")));
            }
        }
        if let Some((_, commands)) = help.split_once("\nCommands:\n") {
            for line in commands.lines().take_while(|l| l.starts_with("  ")) {
                let name = line.split_whitespace().next().unwrap_or_default();
                if name != "help" {
                    let mut next = path.clone();
                    next.push(name.to_owned());
                    pending.push(next);
                }
            }
        }
    }
    assert!(seen > 30, "subcomandos conferidos: {seen}");
    assert!(
        problems.is_empty(),
        "ajuda em português:\n{}",
        problems.join("\n")
    );
}
