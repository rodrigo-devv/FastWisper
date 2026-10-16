//! Regras determinísticas de texto: snippets,
//! dicionário e limpeza leve. Conservadoras de propósito: na dúvida, o texto
//! do ASR fica como está. O que exige entender a frase (autocorreção falada,
//! listas, pontuação) é com o LLM, que tem estas regras como fallback.

use std::collections::BTreeMap;

use crate::config::{DictionaryConfig, Snippet};

/// Hesitações sem outro significado em pt/en. "um", "é", "tipo", "né" ficam
/// de fora: são palavras legítimas ("um carro", "é isso", "tipo de dado").
// i18n: dado (palavras ditadas, não interface)
const HESITATIONS: &[&str] = &[
    "hã", "hãã", "hum", "humm", "hmm", "hmmm", "uh", "uhm", "uhh", "ahn", "erm",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesOutput {
    pub text: String,
    /// Algum snippet expandiu: o texto expandido não passa pelo LLM.
    pub snippet: bool,
}

impl RulesOutput {
    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextRules {
    /// (gatilho, expansão), do maior para o menor gatilho.
    snippets: Vec<(String, String)>,
    /// (de, para) do dicionário, do maior para o menor.
    replace: Vec<(String, String)>,
    /// Grafia canônica (ex.: "Omarchy").
    words: Vec<String>,
}

impl TextRules {
    pub fn new(dictionary: &DictionaryConfig, snippets: &[Snippet]) -> TextRules {
        let mut snippets: Vec<(String, String)> = snippets
            .iter()
            .filter(|s| !s.trigger.trim().is_empty())
            .map(|s| (s.trigger.trim().to_owned(), s.expansion.clone()))
            .collect();
        snippets.sort_by_key(|(t, _)| std::cmp::Reverse(t.chars().count()));
        let mut replace: Vec<(String, String)> = dictionary
            .replace
            .iter()
            .filter(|(from, _)| !from.trim().is_empty())
            .map(|(from, to)| (from.trim().to_owned(), to.clone()))
            .collect();
        replace.sort_by_key(|(f, _)| std::cmp::Reverse(f.chars().count()));
        TextRules {
            snippets,
            replace,
            words: dictionary
                .words
                .iter()
                .filter(|w| !w.trim().is_empty())
                .cloned()
                .collect(),
        }
    }

    /// Termos do dicionário para o prompt do LLM.
    pub fn vocabulary(&self) -> Vec<&str> {
        let mut terms: BTreeMap<&str, ()> = BTreeMap::new();
        for w in &self.words {
            terms.insert(w, ());
        }
        for (_, to) in &self.replace {
            terms.insert(to, ());
        }
        terms.into_keys().collect()
    }

    pub fn apply(&self, input: &str) -> RulesOutput {
        let mut text = collapse_whitespace(input);
        let mut snippet = false;
        for (trigger, expansion) in &self.snippets {
            if let Some(out) = replace_all_words(&text, trigger, expansion) {
                text = out;
                snippet = true;
            }
        }
        for (from, to) in &self.replace {
            if let Some(out) = replace_all_words(&text, from, to) {
                text = out;
            }
        }
        for word in &self.words {
            if let Some(out) = replace_all_words(&text, word, word) {
                text = out;
            }
        }
        RulesOutput {
            text: remove_hesitations(&text),
            snippet,
        }
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Troca todas as ocorrências de `needle` como palavra(s) inteira(s), sem
/// diferenciar maiúsculas (Unicode). `None` se não houve troca.
fn replace_all_words(haystack: &str, needle: &str, with: &str) -> Option<String> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return None;
    }
    let chars: Vec<(usize, char)> = haystack.char_indices().collect();
    let mut out = String::with_capacity(haystack.len());
    let mut changed = false;
    let mut i = 0;
    let mut copied_until = 0;
    while i < chars.len() {
        let at_boundary = i == 0 || !is_word_char(chars[i - 1].1);
        if at_boundary && matches_at(&chars, i, &needle) {
            let end = i + needle.len();
            let end_boundary = end == chars.len() || !is_word_char(chars[end].1);
            if end_boundary {
                let start_byte = chars[i].0;
                let end_byte = chars.get(end).map_or(haystack.len(), |c| c.0);
                if &haystack[start_byte..end_byte] != with {
                    changed = true;
                }
                out.push_str(&haystack[copied_until..start_byte]);
                out.push_str(with);
                copied_until = end_byte;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&haystack[copied_until..]);
    changed.then_some(out)
}

fn matches_at(chars: &[(usize, char)], start: usize, needle: &[char]) -> bool {
    start + needle.len() <= chars.len()
        && needle
            .iter()
            .zip(&chars[start..])
            .all(|(n, (_, c))| n.to_lowercase().eq(c.to_lowercase()))
}

fn is_hesitation(token: &str) -> bool {
    let bare = token
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    !bare.is_empty() && HESITATIONS.contains(&bare.as_str())
}

/// Remove hesitações isoladas (com a vírgula colada nelas). Pontuação final
/// da hesitação passa para a palavra anterior; se a frase começava por ela,
/// a próxima palavra herda a maiúscula.
fn remove_hesitations(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut capitalize_next = false;
    for (i, token) in text.split(' ').enumerate() {
        if is_hesitation(token) {
            let last = token
                .chars()
                .last()
                .filter(|c| matches!(c, '.' | '?' | '!'));
            if let (Some(mark), Some(prev)) = (last, out.last_mut()) {
                let trimmed = prev.trim_end_matches([',', ';']).len();
                prev.truncate(trimmed);
                prev.push(mark);
            }
            if i == 0 && token.chars().next().is_some_and(char::is_uppercase) {
                capitalize_next = true;
            }
            continue;
        }
        if capitalize_next {
            capitalize_next = false;
            let mut chars = token.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_uppercase().chain(chars).collect());
                continue;
            }
        }
        out.push(token.to_owned());
    }
    out.join(" ")
}

/// Negações que, logo antes da frase do "enviar agora", desfazem o comando
/// ("não vou enviar agora", "don't send now"). Lista fechada, normalizada
/// como em [`words`] (sem acento; "don't" vira `don` `t`). Valem para frases
/// de qualquer idioma: quem programa mistura pt e en.
const NEGATIONS: &[&[&str]] = &[
    &["nao"],
    &["nunca"],
    &["jamais"],
    &["nem"],
    &["not"],
    &["never"],
    &["dont"],
    &["don", "t"],
    &["won", "t"],
    &["cannot"],
    &["can", "t"],
    &["shouldn", "t"],
];

/// O que pode ficar entre a negação e a frase ("não **vou** enviar agora",
/// "don't **want to** send now"). Também fechado.
const NEGATION_LINKS: &[&[&str]] = &[
    &[],
    &["vou"],
    &["vamos"],
    &["quero"],
    &["precisa"],
    &["pode"],
    &["deve"],
    &["e", "para"],
    &["e", "pra"],
    &["want", "to"],
    &["need", "to"],
    &["gonna"],
];

/// "Enviar agora" (opt-in): frases já quebradas em
/// palavras normalizadas (minúsculas, sem acento), da mais longa para a
/// mais curta.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendNowMatcher {
    phrases: Vec<Vec<String>>,
}

impl SendNowMatcher {
    pub fn new<'a>(phrases: impl IntoIterator<Item = &'a str>) -> SendNowMatcher {
        let mut phrases: Vec<Vec<String>> = phrases
            .into_iter()
            .map(|p| words(p).into_iter().map(|(_, _, w)| w).collect::<Vec<_>>())
            .filter(|p| !p.is_empty())
            .collect();
        phrases.sort_by_key(|p| std::cmp::Reverse(p.len()));
        phrases.dedup();
        SendNowMatcher { phrases }
    }

    /// Se `text` **termina** com uma das frases (caixa, acentos e pontuação
    /// final não importam; entre as palavras da frase, só espaço), devolve o
    /// texto sem ela e sem a pontuação de ligação que sobrar (`,` `;` `:`
    /// e travessões). Sem palavra nenhuma antes da frase, devolve `""`.
    /// `None`: a frase não está no fim ou vem logo depois de uma negação
    /// ([`NEGATIONS`]), e o texto segue intacto.
    pub fn strip(&self, text: &str) -> Option<String> {
        let found = words(text);
        let phrase = self.phrases.iter().find(|phrase| {
            let Some(tail) = found.len().checked_sub(phrase.len()).map(|at| &found[at..]) else {
                return false;
            };
            tail.iter().map(|(_, _, w)| w).eq(phrase.iter())
                && tail
                    .windows(2)
                    .all(|pair| text[pair[0].1..pair[1].0].chars().all(char::is_whitespace))
        })?;
        let at = found.len() - phrase.len();
        if negated(text, &found[..=at]) {
            return None;
        }
        let start = found[at].0;
        let rest = text[..start].trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '–' | '—')
        });
        Some(if rest.chars().any(is_word_char) {
            rest.to_owned()
        } else {
            String::new()
        })
    }
}

/// As palavras de `before` terminam com uma negação (e talvez um elo de
/// [`NEGATION_LINKS`]) colada à primeira palavra da frase, que é o último
/// item de `before`. Colada: só espaço entre elas (ou o apóstrofo de
/// "don't"); "Não, enviar agora" responde a outra coisa e envia.
fn negated(text: &str, before: &[(usize, usize, String)]) -> bool {
    let joined = |pair: &[(usize, usize, String)]| {
        text[pair[0].1..pair[1].0]
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '\'' | '’'))
    };
    NEGATIONS.iter().any(|negation| {
        NEGATION_LINKS.iter().any(|link| {
            let len = negation.len() + link.len() + 1;
            let Some(window) = before.len().checked_sub(len).map(|at| &before[at..]) else {
                return false;
            };
            window
                .iter()
                .take(len - 1)
                .map(|(_, _, w)| w.as_str())
                .eq(negation.iter().chain(link.iter()).copied())
                && window.windows(2).all(joined)
        })
    })
}

/// Palavras de `text` como (início, fim, forma normalizada).
fn words(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (i, c) in text.char_indices() {
        if is_word_char(c) {
            let (_, word) = current.get_or_insert_with(|| (i, String::new()));
            word.extend(c.to_lowercase().map(fold_accent));
        } else if let Some((start, word)) = current.take() {
            out.push((start, i, word));
        }
    }
    if let Some((start, word)) = current {
        out.push((start, text.len(), word));
    }
    out
}

/// Tira o acento das letras latinas de pt/en ("Já" e "ja" casam).
fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> TextRules {
        let dictionary = DictionaryConfig {
            words: vec!["Omarchy".into(), "Hyprland".into(), "FastWisper".into()],
            replace: [("fast whisper".to_owned(), "FastWisper".to_owned())].into(),
        };
        let snippets = vec![
            Snippet {
                trigger: "meu email".into(),
                expansion: "voce@exemplo.com".into(),
            },
            Snippet {
                trigger: "meu email de trabalho".into(),
                expansion: "voce@empresa.com".into(),
            },
        ];
        TextRules::new(&dictionary, &snippets)
    }

    #[test]
    fn snippet_inside_a_sentence_with_longest_match() {
        let out = rules().apply("Manda para o meu email de trabalho, por favor.");
        assert_eq!(out.text, "Manda para o voce@empresa.com, por favor.");
        assert!(out.snippet);
        let out = rules().apply("Meu email.");
        assert_eq!(out.text, "voce@exemplo.com.");
    }

    #[test]
    fn snippet_needs_word_boundaries() {
        let out = rules().apply("O meu emailing semanal saiu.");
        assert_eq!(out.text, "O meu emailing semanal saiu.");
        assert!(!out.snippet);
    }

    #[test]
    fn dictionary_fixes_spelling_and_case() {
        let out = rules().apply("instalei o fast whisper no omarchy com hyprland");
        assert_eq!(out.text, "instalei o FastWisper no Omarchy com Hyprland");
        assert!(!out.snippet);
    }

    #[test]
    fn legitimate_words_are_never_removed() {
        for s in [
            "Comprei um carro e é isso.",
            "I like Rust, um, maybe.", // "um" pode ser pt; não é removido
            "Esse é o tipo de dado certo, né?",
            "Tem 3 itens: 1, 2 e 3.",
            "rm -rf build && cargo test",
        ] {
            assert_eq!(rules().apply(s).text, s, "{s}");
        }
    }

    #[test]
    fn unambiguous_hesitations_go_away() {
        assert_eq!(
            rules().apply("Hã, manda o relatório.").text,
            "Manda o relatório."
        );
        assert_eq!(rules().apply("Pronto, hmm.").text, "Pronto.");
        assert_eq!(
            rules().apply("Manda o relatório, hmm, amanhã.").text,
            "Manda o relatório, amanhã."
        );
        assert_eq!(rules().apply("  muito   espaço  ").text, "muito espaço");
    }

    #[test]
    fn unicode_case_insensitive_match() {
        let dictionary = DictionaryConfig {
            words: vec![],
            replace: [("ação rápida".to_owned(), "AçãoRápida".to_owned())].into(),
        };
        let r = TextRules::new(&dictionary, &[]);
        assert_eq!(
            r.apply("Use a AÇÃO RÁPIDA agora.").text,
            "Use a AçãoRápida agora."
        );
    }

    fn send_now() -> SendNowMatcher {
        SendNowMatcher::new(["enviar agora", "send now", "Enviar já"])
    }

    #[test]
    fn send_now_only_at_the_end() {
        let m = send_now();
        assert_eq!(
            m.strip("Rode os testes e corrija o erro, enviar agora.")
                .as_deref(),
            Some("Rode os testes e corrija o erro")
        );
        assert_eq!(
            m.strip("Fix the failing test. Send now!").as_deref(),
            Some("Fix the failing test.")
        );
        assert_eq!(
            m.strip("rode cargo test enviar agora").as_deref(),
            Some("rode cargo test")
        );
        // No meio, a frase é texto comum.
        for s in [
            "Vou enviar agora o relatório.",
            "Enviar agora ou depois?  Depois",
            "send now and then wait",
        ] {
            assert_eq!(m.strip(s), None, "{s}");
        }
    }

    #[test]
    fn send_now_ignores_case_accents_and_final_punctuation() {
        let m = send_now();
        for s in [
            "Atualize o README. ENVIAR AGORA",
            "Atualize o README. Enviar agora...",
            "Atualize o README. enviar  agora ?!",
            "Atualize o README. Envíar Agóra.",
            "Atualize o README. enviar ja",
        ] {
            assert_eq!(m.strip(s).as_deref(), Some("Atualize o README."), "{s}");
        }
        // Pontuação entre as palavras da frase muda o sentido: não casa.
        assert_eq!(m.strip("Devo enviar? Agora."), None);
        assert_eq!(m.strip("Devo enviar, agora"), None);
        // Palavra maior que contém a frase não casa.
        assert_eq!(m.strip("vou reenviar agora"), None);
        assert_eq!(m.strip("check the godsend now"), None);
    }

    #[test]
    fn send_now_phrase_alone_or_empty_text_leaves_nothing() {
        let m = send_now();
        assert_eq!(m.strip("Enviar agora.").as_deref(), Some(""));
        assert_eq!(m.strip("  , send now").as_deref(), Some(""));
        assert_eq!(m.strip(""), None);
        assert_eq!(m.strip("..."), None);
        assert_eq!(SendNowMatcher::new([]).strip("enviar agora"), None);
        assert_eq!(SendNowMatcher::new(["", " , "]).strip("enviar agora"), None);
    }

    #[test]
    fn send_now_phrases_only_from_the_given_languages() {
        // O polisher passa só as frases dos idiomas de `asr.languages`.
        let pt_only = SendNowMatcher::new(["enviar agora"]);
        assert_eq!(pt_only.strip("fix it, send now"), None);
        assert_eq!(
            pt_only.strip("fix it, enviar agora").as_deref(),
            Some("fix it")
        );
        // A frase mais longa vence: "agora" sozinha não corta "enviar".
        let both = SendNowMatcher::new(["agora", "enviar agora"]);
        assert_eq!(both.strip("teste enviar agora").as_deref(), Some("teste"));
    }

    #[test]
    fn send_now_after_a_negation_is_plain_text() {
        let m = SendNowMatcher::new(["enviar agora", "envia agora", "send now", "Enviar já"]);
        for s in [
            "Não vou enviar agora.",
            "Revisa o diff, mas não envia agora",
            "Ajusta o teste e não enviar agora",
            "nao vou enviar agora",
            "Nunca enviar agora",
            "Por enquanto não quero enviar agora!",
            "Ajusta o teste, não é pra enviar agora.",
            "não enviar já",
            "Don't send now.",
            "Fix the test but do not send now",
            "fix it, never send now",
            "Fix it and dont send now",
            "I don’t want to send now",
            "We won't send now.",
        ] {
            assert_eq!(m.strip(s), None, "{s}");
        }
        // Negação separada por pontuação, longe da frase ou fora da lista
        // não conta: a frase envia.
        for (s, rest) in [
            ("Não, enviar agora", "Não"),
            ("No. Send now", "No."),
            ("Not that one. Send now", "Not that one."),
            ("Não mexe no README e enviar agora", "Não mexe no README e"),
            ("Isso não é bug, enviar agora", "Isso não é bug"),
            ("Corrige o knot send now", "Corrige o knot"),
            ("Pode enviar agora", "Pode"),
        ] {
            assert_eq!(m.strip(s).as_deref(), Some(rest), "{s}");
        }
    }

    #[test]
    fn vocabulary_lists_canonical_terms() {
        assert_eq!(rules().vocabulary(), ["FastWisper", "Hyprland", "Omarchy"]);
    }
}
