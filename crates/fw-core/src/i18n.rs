//! Mensagens humanas locais. IPC/JSON e conteúdo ditado não passam por este módulo.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::config::UiLanguage;

struct Catalog {
    en: BTreeMap<String, String>,
    templates: Vec<(String, Vec<String>, String)>,
}
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let en: BTreeMap<String, String> =
            serde_json::from_str(include_str!("i18n/en.json")).expect("catálogo inglês versionado");
        let mut templates: Vec<_> = en
            .iter()
            .filter(|(key, _)| key.contains("{0}"))
            .map(|(key, value)| (key.clone(), segments(key), value.clone()))
            .collect();
        // Frases mais específicas têm prioridade sobre mensagens genéricas de erro.
        // Moldes ancorados no início vêm antes dos que começam por `{0}`, que
        // engoliriam um invólucro ("config inválida em {0}: {1}") como parâmetro.
        // Sufixos específicos que envolvem uma frase inteira do catálogo têm
        // prioridade sobre os ancorados: "nuvem {0}" engoliria
        // "nuvem ligada: o Ollama pode acessar a internet".
        templates.sort_by_key(|(key, parts, _)| {
            (
                !WRAPPERS.contains(&key.as_str()),
                parts[0].is_empty(),
                std::cmp::Reverse(parts.iter().map(String::len).sum::<usize>()),
            )
        });
        Catalog { en, templates }
    })
}
/// Invólucros com sufixo específico, conferidos antes de todos os moldes.
const WRAPPERS: [&str; 4] = [
    "{0} (serviço: {1})",
    "{0} (llm.ollama_cloud: {1})",
    "{0}, mas llm.ollama_cloud = true",
    "{0}: o Ollama pode acessar a internet",
];
fn segments(template: &str) -> Vec<String> {
    let mut rest = template;
    let mut parts = Vec::new();
    let mut index = 0;
    while let Some((head, tail)) = rest.split_once(&format!("{{{index}}}")) {
        parts.push(head.to_owned());
        rest = tail;
        index += 1;
    }
    parts.push(rest.to_owned());
    parts
}
fn parameters<'a>(text: &'a str, parts: &[String]) -> Option<Vec<&'a str>> {
    let mut rest = text.strip_prefix(&parts[0])?;
    let mut values = Vec::new();
    for (index, part) in parts.iter().enumerate().skip(1) {
        if index == parts.len() - 1 {
            values.push(rest.strip_suffix(part)?);
        } else {
            // Placeholders adjacentes não têm delimitação; preserva o resto.
            if part.is_empty() {
                return None;
            }
            let (value, tail) = rest.split_once(part)?;
            values.push(value);
            rest = tail;
        }
    }
    Some(values)
}
fn render(template: &str, values: &[String]) -> String {
    let parts = segments(template);
    let mut out = parts[0].clone();
    for (index, part) in parts.iter().enumerate().skip(1) {
        out.push_str(
            values
                .get(index - 1)
                .map(String::as_str)
                .unwrap_or_default(),
        );
        out.push_str(part);
    }
    out
}

fn nested_message(key: &str, index: usize) -> bool {
    match key {
        "{0} = {1}: {2}" => index == 2,
        "{0} já é o atalho de {1}; escolha outro" => index == 1,
        "config inválida em {0}: {1}"
        | "valor inválido para {0}: {1}"
        | "falha ao acessar {0}: {1}" => index == 1,
        "Texto copiado: cole com Ctrl+V ({0})."
        | "Texto colado, mas não enviado ({0})."
        | "{0} (serviço: {1})"
        | "Ollama: {0}"
        | "Ollama: {0}."
        | "Nada aplicado: {0}."
        | "llm.endpoint: {0}"
        | "falha de comunicação com o fastwisperd: {0}"
        | "config TOML inválida: {0}"
        | "{0} (llm.ollama_cloud: {1})"
        | "{0}, mas llm.ollama_cloud = true"
        | "{0}: o Ollama pode acessar a internet"
        | "não conectado agora ({0})" => index == 0,
        _ => false,
    }
}

/// Traduz mensagens já formatadas, sem dependências ou rede. Parâmetros
/// desconhecidos (caminhos, nomes, códigos) permanecem byte a byte.
pub fn translate(text: &str, language: UiLanguage) -> String {
    translate_depth(text, language, 0)
}
fn translate_depth(text: &str, language: UiLanguage, depth: u8) -> String {
    if language == UiLanguage::PtBr || depth > 4 {
        return text.to_owned();
    }
    let catalog = catalog();
    if let Some(message) = catalog.en.get(text) {
        return message.clone();
    }
    if let Some((key, message)) = text.split_once(": ")
        && key.contains('.')
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte == b'.')
    {
        return format!("{key}: {}", translate_depth(message, language, depth + 1));
    }
    for (key, parts, message) in &catalog.templates {
        if let Some(values) = parameters(text, parts) {
            let values: Vec<_> = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    if nested_message(key, index) {
                        translate_depth(value, language, depth + 1)
                    } else {
                        value.to_owned()
                    }
                })
                .collect();
            return render(message, &values);
        }
    }
    if let Some(message) = text.strip_prefix("fastwisper: ") {
        return format!(
            "fastwisper: {}",
            translate_depth(message, language, depth + 1)
        );
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_have_identical_nonempty_keys_and_parameters() {
        let en: BTreeMap<String, String> =
            serde_json::from_str(include_str!("i18n/en.json")).expect("fixture de idioma válida");
        let pt: BTreeMap<String, String> = serde_json::from_str(include_str!("i18n/pt-BR.json"))
            .expect("fixture de idioma válida");
        assert_eq!(en.keys().collect::<Vec<_>>(), pt.keys().collect::<Vec<_>>());
        for (key, value) in en {
            assert!(!value.trim().is_empty(), "{key}");
            assert_eq!(segments(&key).len(), segments(&value).len(), "{key}");
        }
    }
    #[test]
    fn send_now_validation_and_doctor_hints_are_translated() {
        let path = std::path::Path::new("/tmp/fw.toml");
        for (phrases, english) in [
            (r#"[" "]"#, "inject.send_now_phrases.pt: empty phrase"),
            (
                r#"["envia já!"]"#,
                "inject.send_now_phrases.pt: \"envia já!\" must contain only letters, numbers and spaces",
            ),
        ] {
            let text = format!("[inject.send_now_phrases]\npt = {phrases}\n");
            let error = crate::config::Config::parse(&text, path)
                .expect_err("frase inválida")
                .to_string();
            assert_eq!(
                translate(&error, UiLanguage::En),
                format!("invalid config at /tmp/fw.toml: {english}")
            );
        }
        assert_eq!(
            translate(
                "fastwisper models pull base (ou `fastwisper models verify`)",
                UiLanguage::En
            ),
            "fastwisper models pull base (or `fastwisper models verify`)"
        );
    }

    #[test]
    fn setup_microphone_and_gpu_fallbacks_are_translated() {
        assert_eq!(
            translate(
                "não conectado agora (nenhum microfone corresponde a \"usb\" (disponíveis: [\"Microfone padrão\"]))",
                UiLanguage::En
            ),
            "not connected now (no microphone matches \"usb\" (available: [\"Microfone padrão\"]))"
        );
        assert_eq!(
            translate(
                "mais de uma GPU corresponde a \"nvidia\": [\"NVIDIA A\", \"NVIDIA B\"]",
                UiLanguage::En
            ),
            "more than one GPU matches \"nvidia\": [\"NVIDIA A\", \"NVIDIA B\"]"
        );
        assert_eq!(translate("erro", UiLanguage::En), "error");
        // Estado da nuvem no doctor: o molde "nuvem {0}" não engole o invólucro.
        for (pt, en) in [
            (
                "nuvem desligada (OLLAMA_NO_CLOUD) (llm.ollama_cloud: offline)",
                "cloud disabled (OLLAMA_NO_CLOUD) (llm.ollama_cloud: offline)",
            ),
            (
                "nuvem ligada: o Ollama pode acessar a internet",
                "cloud enabled: Ollama can access the internet",
            ),
            (
                "nuvem ligada (OLLAMA_NO_CLOUD), mas llm.ollama_cloud = true",
                "cloud enabled (OLLAMA_NO_CLOUD), but llm.ollama_cloud = true",
            ),
        ] {
            assert_eq!(translate(pt, UiLanguage::En), en);
        }
        // Instalação sem Ollama (`ollama apply` no install.sh): frase inteira traduzida.
        assert_eq!(
            translate("Nada aplicado: Ollama não respondeu.", UiLanguage::En),
            "Nothing applied: Ollama did not respond."
        );
        assert_eq!(
            translate("`sudo /bin/sh -c x` falhou", UiLanguage::En),
            "`sudo /bin/sh -c x` failed"
        );
        // Sem molde genérico "erro: {0}": frase desconhecida fica intacta.
        let text = "erro: algo que não está no catálogo";
        assert_eq!(translate(text, UiLanguage::En), text);
        let text = "nenhuma GPU corresponde a \"x\" (disponíveis: [])";
        assert_eq!(translate(text, UiLanguage::PtBr), text);
    }

    #[test]
    fn formats_human_messages_and_preserves_identifiers() {
        let text = "config inválida em /tmp/Português.toml: audio.sound_volume = 2; use de 0 a 1";
        assert_eq!(
            translate(text, UiLanguage::En),
            "invalid config at /tmp/Português.toml: audio.sound_volume = 2; use a value from 0 to 1"
        );
        assert_eq!(translate(text, UiLanguage::PtBr), text);
        assert_eq!(
            translate(
                "hotkey.dictate = \"SHIFT+SPACE\": use ao menos um modificador além de Shift (Super, Ctrl ou Alt): sem ele, o Hyprland engole a tecla durante a digitação",
                UiLanguage::En
            ),
            "hotkey.dictate = \"SHIFT+SPACE\": use at least one modifier other than Shift (Super, Ctrl or Alt): without it, Hyprland consumes the key while typing"
        );
        assert_eq!(
            translate(
                "modelo Desempenho não existe e não está no manifesto",
                UiLanguage::En
            ),
            "model Desempenho does not exist and is not in the manifest"
        );
        for value in [
            "asr.languages",
            "mic_silent",
            "{\"error\":\"mic_silent\"}",
            "state=offline",
            "pt-BR",
        ] {
            assert_eq!(translate(value, UiLanguage::En), value);
        }
    }
}
