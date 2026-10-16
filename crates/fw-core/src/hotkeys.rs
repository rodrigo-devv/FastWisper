//! Atalhos editáveis (`[hotkey] dictate`, `hands_free`, `paste_last`).
//!
//! A config guarda o atalho como texto (`ALT+SPACE`). Daqui sai o arquivo
//! de dados que `packaging/omarchy/fastwisper.lua` lê ao carregar
//! (`$XDG_DATA_HOME/fastwisper/hotkeys.conf`): só linhas `nome ATALHO
//! keycodes`, que o Lua confere com padrões estritos e nunca executa. Se o
//! arquivo faltar ou não passar, o Lua usa os padrões embutidos nele, que o
//! teste `lua_defaults_match_the_generator` mantém iguais aos daqui.
//!
//! Garantias de hoje: todo atalho tem um modificador além
//! de Shift (uma tecla sozinha ou Shift+tecla faria o Hyprland engolir a
//! digitação), Esc é fixo para cancelar e nenhum bind de release é criado:
//! a soltura do "segurar" é vista pelo observador `input.keyboard.key`, que
//! compara os keycodes desta tabela.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::HotkeyConfig;

/// Keycode xkb (evdev + 8) do Esc, a tecla fixa de cancelar.
pub const ESC_KEYCODE: u32 = 9;

/// Prefixo da descrição dos binds do FastWisper (`fastwisper.lua`): o que
/// `hyprctl binds -j` mostra com ele é nosso e não conta como conflito.
pub const BIND_DESCRIPTION_PREFIX: &str = "FastWisper:";

/// Primeira linha do arquivo gerado; o desinstalador só apaga o que a tem.
// i18n: dado (comentário do arquivo gerado; o Lua e o desinstalador o leem)
pub const FILE_HEADER: &str = "# FastWisper: atalhos gerados da config ([hotkey]).";

/// Tamanho máximo que o Lua aceita ler do arquivo.
pub const FILE_MAX_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    /// Ordem canônica do texto (`SUPER+CTRL+ALT+SHIFT+tecla`).
    pub const ALL: [Modifier; 4] = [
        Modifier::Super,
        Modifier::Ctrl,
        Modifier::Alt,
        Modifier::Shift,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Modifier::Super => "SUPER",
            Modifier::Ctrl => "CTRL",
            Modifier::Alt => "ALT",
            Modifier::Shift => "SHIFT",
        }
    }

    /// Como aparece no hub.
    pub fn label(self) -> &'static str {
        match self {
            Modifier::Super => "Super",
            Modifier::Ctrl => "Ctrl",
            Modifier::Alt => "Alt",
            Modifier::Shift => "Shift",
        }
    }

    /// Bit no `modmask` do Hyprland (`hyprctl binds -j`).
    pub fn modmask(self) -> u32 {
        match self {
            Modifier::Shift => 1,
            Modifier::Ctrl => 4,
            Modifier::Alt => 8,
            Modifier::Super => 64,
        }
    }

    /// Keycodes xkb da tecla esquerda e da direita.
    pub fn keycodes(self) -> [u32; 2] {
        match self {
            Modifier::Shift => [50, 62],
            Modifier::Ctrl => [37, 105],
            Modifier::Alt => [64, 108],
            Modifier::Super => [133, 134],
        }
    }

    fn parse(word: &str) -> Option<Modifier> {
        Some(match word {
            "SUPER" | "WIN" | "LOGO" | "MOD4" | "META" => Modifier::Super,
            "CTRL" | "CONTROL" => Modifier::Ctrl,
            "ALT" | "MOD1" => Modifier::Alt,
            "SHIFT" => Modifier::Shift,
            _ => return None,
        })
    }
}

/// Uma tecla principal aceita. `name` é o nome do keysym que o Hyprland
/// entende (sem diferenciar maiúsculas); `keycode` é a posição física
/// (xkb = evdev + 8, como `input.keyboard.key` entrega); `code` é o
/// `KeyboardEvent.code` da captura no hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Key {
    pub name: &'static str,
    pub keycode: u32,
    pub code: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    pub aliases: &'static [&'static str],
}

const fn key(name: &'static str, keycode: u32, code: &'static str, label: &'static str) -> Key {
    Key {
        name,
        keycode,
        code,
        label,
        aliases: &[],
    }
}

const fn alias(
    name: &'static str,
    keycode: u32,
    code: &'static str,
    label: &'static str,
    aliases: &'static [&'static str],
) -> Key {
    Key {
        name,
        keycode,
        code,
        label,
        aliases,
    }
}

/// Teclas principais aceitas. Os keycodes são as posições do teclado
/// físico (evdev); o teste `keycodes_match_linux_input_event_codes` os
/// confere contra `linux/input-event-codes.h`. Letras e pontuação seguem
/// a posição do layout US/ABNT2; Espaço, F1–F12, Enter e setas valem em
/// qualquer layout.
pub const KEYS: &[Key] = &[
    key("A", 38, "KeyA", "A"),
    key("B", 56, "KeyB", "B"),
    key("C", 54, "KeyC", "C"),
    key("D", 40, "KeyD", "D"),
    key("E", 26, "KeyE", "E"),
    key("F", 41, "KeyF", "F"),
    key("G", 42, "KeyG", "G"),
    key("H", 43, "KeyH", "H"),
    key("I", 31, "KeyI", "I"),
    key("J", 44, "KeyJ", "J"),
    key("K", 45, "KeyK", "K"),
    key("L", 46, "KeyL", "L"),
    key("M", 58, "KeyM", "M"),
    key("N", 57, "KeyN", "N"),
    key("O", 32, "KeyO", "O"),
    key("P", 33, "KeyP", "P"),
    key("Q", 24, "KeyQ", "Q"),
    key("R", 27, "KeyR", "R"),
    key("S", 39, "KeyS", "S"),
    key("T", 28, "KeyT", "T"),
    key("U", 30, "KeyU", "U"),
    key("V", 55, "KeyV", "V"),
    key("W", 25, "KeyW", "W"),
    key("X", 53, "KeyX", "X"),
    key("Y", 29, "KeyY", "Y"),
    key("Z", 52, "KeyZ", "Z"),
    key("1", 10, "Digit1", "1"),
    key("2", 11, "Digit2", "2"),
    key("3", 12, "Digit3", "3"),
    key("4", 13, "Digit4", "4"),
    key("5", 14, "Digit5", "5"),
    key("6", 15, "Digit6", "6"),
    key("7", 16, "Digit7", "7"),
    key("8", 17, "Digit8", "8"),
    key("9", 18, "Digit9", "9"),
    key("0", 19, "Digit0", "0"),
    key("F1", 67, "F1", "F1"),
    key("F2", 68, "F2", "F2"),
    key("F3", 69, "F3", "F3"),
    key("F4", 70, "F4", "F4"),
    key("F5", 71, "F5", "F5"),
    key("F6", 72, "F6", "F6"),
    key("F7", 73, "F7", "F7"),
    key("F8", 74, "F8", "F8"),
    key("F9", 75, "F9", "F9"),
    key("F10", 76, "F10", "F10"),
    key("F11", 95, "F11", "F11"),
    key("F12", 96, "F12", "F12"),
    // i18n: dado (nomes aceitos na entrada; o rótulo passa pelo catálogo)
    alias("SPACE", 65, "Space", "Espaço", &["ESPACO", "ESPAÇO"]),
    alias("RETURN", 36, "Enter", "Enter", &["ENTER"]),
    key("TAB", 23, "Tab", "Tab"),
    key("BACKSPACE", 22, "Backspace", "Backspace"),
    alias("DELETE", 119, "Delete", "Delete", &["DEL"]),
    alias("INSERT", 118, "Insert", "Insert", &["INS"]),
    key("HOME", 110, "Home", "Home"),
    key("END", 115, "End", "End"),
    alias(
        "PAGE_UP",
        112,
        "PageUp",
        "Page Up",
        &["PRIOR", "PAGEUP", "PGUP"],
    ),
    alias(
        "PAGE_DOWN",
        117,
        "PageDown",
        "Page Down",
        &["NEXT", "PAGEDOWN", "PGDN"],
    ),
    key("UP", 111, "ArrowUp", "↑"),
    key("DOWN", 116, "ArrowDown", "↓"),
    key("LEFT", 113, "ArrowLeft", "←"),
    key("RIGHT", 114, "ArrowRight", "→"),
    key("MINUS", 20, "Minus", "-"),
    key("EQUAL", 21, "Equal", "="),
    key("BRACKETLEFT", 34, "BracketLeft", "["),
    key("BRACKETRIGHT", 35, "BracketRight", "]"),
    key("SEMICOLON", 47, "Semicolon", ";"),
    key("APOSTROPHE", 48, "Quote", "'"),
    key("GRAVE", 49, "Backquote", "`"),
    key("BACKSLASH", 51, "Backslash", "\\"),
    key("COMMA", 59, "Comma", ","),
    key("PERIOD", 60, "Period", "."),
    key("SLASH", 61, "Slash", "/"),
    alias("PRINT", 107, "PrintScreen", "Print", &["SYSRQ"]),
    key("PAUSE", 127, "Pause", "Pause"),
];

impl Key {
    pub fn find(word: &str) -> Option<&'static Key> {
        KEYS.iter()
            .find(|k| k.name == word || k.aliases.contains(&word))
    }

    /// Nome da tecla em `hyprctl binds -j` (ex.: `comma`, `Prior`) é esta?
    fn matches_bind_name(&self, name: &str) -> bool {
        let upper = name.to_uppercase();
        self.name == upper || self.aliases.contains(&upper.as_str())
    }
}

/// As três ações com atalho editável.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Dictate,
    HandsFree,
    PasteLast,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Dictate, Action::HandsFree, Action::PasteLast];

    /// Nome no arquivo gerado e no fim da chave da config.
    pub fn name(self) -> &'static str {
        match self {
            Action::Dictate => "dictate",
            Action::HandsFree => "hands_free",
            Action::PasteLast => "paste_last",
        }
    }

    /// Chave da config (`hotkey.dictate`).
    pub fn config_key(self) -> &'static str {
        match self {
            Action::Dictate => "hotkey.dictate",
            Action::HandsFree => "hotkey.hands_free",
            Action::PasteLast => "hotkey.paste_last",
        }
    }

    pub fn from_config_key(key: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.config_key() == key)
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|a| a.name() == name || a.config_key() == name)
    }

    /// Como aparece para o usuário.
    pub fn label(self) -> &'static str {
        match self {
            Action::Dictate => "Atalho para ditar",
            Action::HandsFree => "Mãos livres",
            Action::PasteLast => "Colar de novo",
        }
    }

    /// Padrão: os atalhos de antes da E2.
    pub fn default_text(self) -> &'static str {
        match self {
            Action::Dictate => "ALT+SPACE",
            Action::HandsFree => "ALT+SHIFT+SPACE",
            Action::PasteLast => "SUPER+ALT+V",
        }
    }

    pub fn get(self, config: &HotkeyConfig) -> &str {
        match self {
            Action::Dictate => &config.dictate,
            Action::HandsFree => &config.hands_free,
            Action::PasteLast => &config.paste_last,
        }
    }
}

/// Atalho validado: modificadores (ao menos um além de Shift) e uma tecla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Sem repetição, na ordem de [`Modifier::ALL`].
    modifiers: [Option<Modifier>; 4],
    key: &'static Key,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeyError {
    #[error("atalho vazio; use modificadores + uma tecla, ex.: SUPER+D")]
    Empty,
    #[error("atalho longo demais")]
    TooLong,
    #[error("tecla desconhecida: {0} (veja `fastwisper hotkeys keys`)")]
    UnknownKey(String),
    #[error("{0} aparece duas vezes")]
    Repeated(String),
    #[error("falta a tecla principal depois dos modificadores (ex.: SUPER+D)")]
    MissingKey,
    #[error("use uma tecla só, por último (recebido {0} e {1})")]
    TwoKeys(String, String),
    #[error(
        "use ao menos um modificador além de Shift (Super, Ctrl ou Alt): sem ele, o Hyprland engole a tecla durante a digitação"
    )]
    NeedsModifier,
    #[error("Esc é fixo para cancelar a gravação e não pode ser atalho")]
    Escape,
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    /// Aceita `SUPER+D`, `super + d`, `Ctrl+Alt+Enter`; sem diferenciar
    /// maiúsculas. Texto canônico: [`Hotkey::to_string`].
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 64 {
            return Err(HotkeyError::TooLong);
        }
        let words: Vec<String> = text.split('+').map(|w| w.trim().to_uppercase()).collect();
        if words.iter().all(String::is_empty) {
            return Err(HotkeyError::Empty);
        }
        let mut modifiers = [None; 4];
        let mut key: Option<&'static Key> = None;
        for word in &words {
            if word.is_empty() {
                return Err(HotkeyError::MissingKey);
            }
            if let Some(modifier) = Modifier::parse(word) {
                let slot = Modifier::ALL
                    .iter()
                    .position(|m| *m == modifier)
                    .expect("modificador conhecido");
                if modifiers[slot].replace(modifier).is_some() {
                    return Err(HotkeyError::Repeated(modifier.label().to_owned()));
                }
                continue;
            }
            if matches!(word.as_str(), "ESC" | "ESCAPE") {
                return Err(HotkeyError::Escape);
            }
            let found = Key::find(word).ok_or_else(|| HotkeyError::UnknownKey(word.clone()))?;
            if let Some(previous) = key {
                return Err(if previous == found {
                    HotkeyError::Repeated(found.label.to_owned())
                } else {
                    HotkeyError::TwoKeys(previous.name.to_owned(), found.name.to_owned())
                });
            }
            key = Some(found);
        }
        let key = key.ok_or(HotkeyError::MissingKey)?;
        if !modifiers.iter().flatten().any(|m| *m != Modifier::Shift) {
            return Err(HotkeyError::NeedsModifier);
        }
        Ok(Hotkey { modifiers, key })
    }
}

impl fmt::Display for Hotkey {
    /// Canônico: `SUPER+ALT+V`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for modifier in self.modifiers() {
            write!(f, "{}+", modifier.name())?;
        }
        f.write_str(self.key.name)
    }
}

impl Hotkey {
    pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
        self.modifiers.iter().flatten().copied()
    }

    pub fn key(&self) -> &'static Key {
        self.key
    }

    pub fn modmask(&self) -> u32 {
        self.modifiers().map(Modifier::modmask).sum()
    }

    /// Rótulos das teclas, como o hub mostra: `["Alt", "Espaço"]`.
    pub fn labels(&self) -> Vec<&'static str> {
        self.modifiers()
            .map(Modifier::label)
            .chain([self.key.label])
            .collect()
    }

    /// Texto para pessoas: `Alt + Espaço`.
    pub fn label(&self) -> String {
        self.labels().join(" + ")
    }

    /// Keycodes cuja soltura encerra o "segurar": a tecla principal e os
    /// dois lados (esquerdo e direito) de cada modificador do atalho.
    pub fn release_keycodes(&self) -> Vec<u32> {
        std::iter::once(self.key.keycode)
            .chain(self.modifiers().flat_map(Modifier::keycodes))
            .collect()
    }
}

/// Os três atalhos, validados e distintos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkeys {
    pub dictate: Hotkey,
    pub hands_free: Hotkey,
    pub paste_last: Hotkey,
}

impl Hotkeys {
    /// Valida cada atalho e recusa dois iguais (conflito entre os três).
    pub fn from_config(config: &HotkeyConfig) -> Result<Hotkeys, String> {
        let parse = |action: Action| {
            action
                .get(config)
                .parse::<Hotkey>()
                .map_err(|e| format!("{} = {:?}: {e}", action.config_key(), action.get(config)))
        };
        let hotkeys = Hotkeys {
            dictate: parse(Action::Dictate)?,
            hands_free: parse(Action::HandsFree)?,
            paste_last: parse(Action::PasteLast)?,
        };
        for (i, a) in Action::ALL.iter().enumerate() {
            for b in &Action::ALL[i + 1..] {
                if hotkeys.get(*a) == hotkeys.get(*b) {
                    return Err(format!(
                        "{} e {} usam o mesmo atalho ({}); cada ação precisa do seu",
                        a.config_key(),
                        b.config_key(),
                        hotkeys.get(*a)
                    ));
                }
            }
        }
        Ok(hotkeys)
    }

    pub fn get(&self, action: Action) -> Hotkey {
        match action {
            Action::Dictate => self.dictate,
            Action::HandsFree => self.hands_free,
            Action::PasteLast => self.paste_last,
        }
    }
}

/// Texto do arquivo que `fastwisper.lua` lê. Formato (uma ação por linha):
/// `nome ATALHO [keycodes]`; só `dictate` leva os keycodes da soltura.
pub fn render(hotkeys: &Hotkeys) -> String {
    // i18n: dado (comentário do arquivo gerado)
    let mut text = format!(
        "{FILE_HEADER}\n# Não edite: `fastwisper config set hotkey.<ação> ...` (ou o hub) reescreve.\nversion 1\n"
    );
    for action in Action::ALL {
        let hotkey = hotkeys.get(action);
        text.push_str(action.name());
        text.push(' ');
        text.push_str(&hotkey.to_string());
        if action == Action::Dictate {
            for code in hotkey.release_keycodes() {
                text.push_str(&format!(" {code}"));
            }
        }
        text.push('\n');
    }
    text
}

/// `$XDG_DATA_HOME/fastwisper/hotkeys.conf`, ou `~/.local/share/...`: o
/// mesmo `data_root` do `install.sh`, que grava o caminho no Lua.
pub fn file_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?).join(".local/share")
        }
    };
    Some(base.join("fastwisper").join("hotkeys.conf"))
}

// ------------------------------------------------------------- conflitos

/// Um bind de `hyprctl binds -j` (só os campos usados).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Bind {
    #[serde(default)]
    pub modmask: u32,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub keycode: u32,
    #[serde(default)]
    pub submap: String,
    #[serde(default)]
    pub mouse: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub dispatcher: String,
    #[serde(default)]
    pub arg: String,
}

impl Bind {
    /// Algo legível para o aviso: a descrição, ou o dispatcher.
    pub fn summary(&self) -> String {
        if !self.description.trim().is_empty() {
            return self.description.trim().to_owned();
        }
        match self.dispatcher.as_str() {
            "" | "__lua" => "bind sem descrição".to_owned(),
            dispatcher if self.arg.is_empty() => dispatcher.to_owned(),
            dispatcher => format!("{dispatcher} {}", self.arg),
        }
    }

    /// Texto das teclas do bind (`SUPER+V`), na mesma forma dos atalhos.
    pub fn combo(&self) -> String {
        let mut text = String::new();
        for modifier in Modifier::ALL {
            if self.modmask & modifier.modmask() != 0 {
                text.push_str(modifier.name());
                text.push('+');
            }
        }
        text.push_str(&self.key.to_uppercase());
        text
    }

    fn is_ours(&self) -> bool {
        self.description.starts_with(BIND_DESCRIPTION_PREFIX)
    }
}

/// Binds de outros (submapa padrão, teclado) que disparam com `hotkey`.
/// Os do próprio FastWisper (descrição `FastWisper:`) ficam de fora.
pub fn conflicts<'a>(hotkey: &Hotkey, binds: &'a [Bind]) -> Vec<&'a Bind> {
    binds
        .iter()
        .filter(|b| b.submap.is_empty() && !b.mouse && !b.is_ours())
        .filter(|b| b.modmask == hotkey.modmask())
        .filter(|b| {
            if b.keycode != 0 {
                b.keycode == hotkey.key.keycode
            } else {
                hotkey.key.matches_bind_name(&b.key)
            }
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum HyprError {
    #[error("Hyprland não está rodando nesta sessão")]
    NotRunning,
    #[error("falha ao rodar hyprctl: {0}")]
    Spawn(std::io::Error),
    #[error("hyprctl {0} falhou: {1}")]
    Failed(&'static str, String),
    #[error("resposta inesperada de hyprctl binds -j: {0}")]
    Json(String),
    #[error("hyprctl {0} não respondeu em {1} s")]
    Timeout(&'static str, u64),
}

/// Prazo de cada chamada ao `hyprctl` (`binds -j`, `reload`): um Hyprland
/// travado não pode prender a CLI nem o hub.
pub const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(5);

/// `hyprctl` da sessão. Sem `HYPRLAND_INSTANCE_SIGNATURE`, não há Hyprland
/// para consultar nem recarregar.
#[derive(Debug, Clone)]
pub struct Hyprctl {
    program: PathBuf,
    running: bool,
    timeout: Duration,
}

impl Hyprctl {
    /// O `hyprctl` do `PATH`, se houver uma sessão Hyprland.
    pub fn from_env() -> Hyprctl {
        Hyprctl {
            program: PathBuf::from("hyprctl"),
            running: std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty()),
            timeout: HYPRCTL_TIMEOUT,
        }
    }

    /// Um programa específico, sempre tratado como sessão ativa (testes).
    pub fn with_program(program: impl Into<PathBuf>) -> Hyprctl {
        Hyprctl {
            program: program.into(),
            running: true,
            timeout: HYPRCTL_TIMEOUT,
        }
    }

    /// Outro prazo por chamada (testes).
    pub fn with_timeout(mut self, timeout: Duration) -> Hyprctl {
        self.timeout = timeout;
        self
    }

    pub fn running(&self) -> bool {
        self.running
    }

    fn run(&self, what: &'static str, args: &[&str]) -> Result<String, HyprError> {
        if !self.running {
            return Err(HyprError::NotRunning);
        }
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(HyprError::Spawn)?;
        // Leitura em threads e espera com prazo: um `hyprctl` que não sai
        // (Hyprland travado) é morto em vez de prender quem chamou.
        // A lista de binds tem dezenas de KiB; nada além de 4 MiB é esperado.
        let (tx, rx) = mpsc::channel();
        for (stream, is_stdout) in [
            (
                child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
                true,
            ),
            (
                child
                    .stderr
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
                false,
            ),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = match stream {
                    Some(s) => s.take(4 << 20).read_to_end(&mut bytes).map(|_| ()),
                    None => Ok(()),
                };
                let _ = tx.send((is_stdout, read.map(|()| bytes)));
            });
        }
        drop(tx);
        let deadline = Instant::now() + self.timeout;
        let timed_out = || HyprError::Timeout(what, self.timeout.as_secs().max(1));
        let status = loop {
            if let Some(status) = child.try_wait().map_err(HyprError::Spawn)? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(timed_out());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let left = deadline.saturating_duration_since(Instant::now());
            let (is_stdout, bytes) = rx.recv_timeout(left).map_err(|_| timed_out())?;
            let bytes = bytes.map_err(HyprError::Spawn)?;
            if is_stdout {
                stdout = bytes;
            } else {
                stderr = bytes;
            }
        }
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(HyprError::Failed(what, stderr.trim().to_owned()));
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// `hyprctl binds -j` (só leitura).
    pub fn binds(&self) -> Result<Vec<Bind>, HyprError> {
        let text = self.run("binds -j", &["binds", "-j"])?;
        serde_json::from_str(&text).map_err(|e| HyprError::Json(e.to_string()))
    }

    /// `hyprctl reload`: o Hyprland relê a config, e o Lua, o arquivo gerado.
    pub fn reload(&self) -> Result<(), HyprError> {
        let reply = self.run("reload", &["reload"])?;
        let reply = reply.trim();
        if reply.is_empty() || reply == "ok" {
            Ok(())
        } else {
            Err(HyprError::Failed("reload", reply.to_owned()))
        }
    }
}

/// Um bind de outro programa que dispara com o atalho, para o aviso.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictView {
    pub description: String,
    pub combo: String,
}

/// Prévia de um atalho novo para uma ação: texto canônico, rótulos e os
/// binds de outros que ele atropela (conferidos em `hyprctl binds -j`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub action: Action,
    pub combo: String,
    pub labels: Vec<&'static str>,
    pub conflicts: Vec<ConflictView>,
    /// `false` quando os binds do Hyprland não puderam ser lidos.
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Valida `text` como novo atalho de `action`, ao lado dos outros dois da
/// config. Recusa (Err) atalho inválido ou igual ao de outra ação; conflito
/// com bind de outro programa só entra na prévia (quem grava pede
/// confirmação).
pub fn preview(
    config: &HotkeyConfig,
    action: Action,
    text: &str,
    hypr: &Hyprctl,
) -> Result<Preview, String> {
    let hotkey: Hotkey = text.parse().map_err(|e: HotkeyError| e.to_string())?;
    for other in Action::ALL.into_iter().filter(|a| *a != action) {
        if other.get(config).parse::<Hotkey>().ok() == Some(hotkey) {
            return Err(format!(
                "{} já é o atalho de {}; escolha outro",
                hotkey.label(),
                other.label()
            ));
        }
    }
    let (conflicts, checked, note) = match hypr.binds() {
        Ok(binds) => (
            conflicts(&hotkey, &binds)
                .into_iter()
                .map(|b| ConflictView {
                    description: b.summary(),
                    combo: b.combo(),
                })
                .collect(),
            true,
            None,
        ),
        Err(HyprError::NotRunning) => (
            Vec::new(),
            false,
            Some("Hyprland fora do ar: conflitos com outros atalhos não conferidos.".to_owned()),
        ),
        Err(e) => (
            Vec::new(),
            false,
            Some(format!("Conflitos não conferidos: {e}.")),
        ),
    };
    Ok(Preview {
        action,
        combo: hotkey.to_string(),
        labels: hotkey.labels(),
        conflicts,
        checked,
        note,
    })
}

// --------------------------------------------------------------- aplicar

/// O que [`apply`] fez.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Applied {
    /// O arquivo já estava assim (ou faltava e os atalhos são os padrões,
    /// que o Lua já usa sem arquivo): nada gravado nem recarregado.
    Unchanged,
    /// Gravado e o Hyprland recarregou.
    Reloaded,
    /// Gravado; o Hyprland não recarregou (fora de uma sessão, pedido
    /// `--no-reload`, ou a recarga falhou com `reload_error`).
    Written {
        #[serde(skip_serializing_if = "Option::is_none")]
        reload_error: Option<String>,
    },
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("{0}")]
    Invalid(String),
    #[error("não foi possível localizar a pasta de dados (defina XDG_DATA_HOME ou HOME)")]
    NoDataDir,
    #[error(transparent)]
    Write(#[from] crate::config_edit::ConfigEditError),
}

/// Regera `file` a partir de `config` (gravação atômica) e, se mudou,
/// recarrega o Hyprland por `hypr` (`None`: só grava). Arquivo ausente com
/// os atalhos padrão fica ausente: o Lua já cai nos mesmos padrões.
pub fn apply(
    config: &HotkeyConfig,
    file: &Path,
    hypr: Option<&Hyprctl>,
) -> Result<Applied, ApplyError> {
    let hotkeys = Hotkeys::from_config(config).map_err(ApplyError::Invalid)?;
    let text = render(&hotkeys);
    // Só os três atalhos contam: os limites de gravação de `[hotkey]` não
    // vão para o arquivo e não justificam recarregar o Hyprland.
    let defaults = Hotkeys::from_config(&HotkeyConfig::default()).map_err(ApplyError::Invalid)?;
    match std::fs::read(file) {
        Ok(current) if current == text.as_bytes() => return Ok(Applied::Unchanged),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && hotkeys == defaults => {
            return Ok(Applied::Unchanged);
        }
        _ => {}
    }
    crate::config_edit::write_atomic(file, &text)?;
    let Some(hypr) = hypr.filter(|h| h.running()) else {
        return Ok(Applied::Written { reload_error: None });
    };
    Ok(match hypr.reload() {
        Ok(()) => Applied::Reloaded,
        Err(e) => Applied::Written {
            reload_error: Some(e.to_string()),
        },
    })
}

/// [`apply`] no arquivo padrão, com o `hyprctl` da sessão.
pub fn apply_default(config: &HotkeyConfig, reload: bool) -> Result<Applied, ApplyError> {
    let file = file_path().ok_or(ApplyError::NoDataDir)?;
    let hypr = Hyprctl::from_env();
    apply(config, &file, reload.then_some(&hypr))
}

/// Mensagem curta para a CLI e o hub.
pub fn applied_message(applied: &Applied) -> String {
    match applied {
        Applied::Unchanged => "Atalhos do Hyprland já estavam em dia.".to_owned(),
        Applied::Reloaded => "Atalhos aplicados: o Hyprland recarregou.".to_owned(),
        Applied::Written { reload_error: None } => {
            "Atalhos gravados; valem na próxima recarga do Hyprland.".to_owned()
        }
        Applied::Written {
            reload_error: Some(e),
        } => {
            format!("Atalhos gravados, mas o Hyprland não recarregou ({e}); rode `hyprctl reload`.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(text: &str) -> Hotkey {
        text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parsing_normalizes_case_spaces_aliases_and_order() {
        assert_eq!(hk("super+d").to_string(), "SUPER+D");
        assert_eq!(hk(" Alt + Space ").to_string(), "ALT+SPACE");
        assert_eq!(hk("shift+alt+space").to_string(), "ALT+SHIFT+SPACE");
        assert_eq!(hk("Control+Enter").to_string(), "CTRL+RETURN");
        assert_eq!(hk("WIN+ALT+v").to_string(), "SUPER+ALT+V");
        assert_eq!(
            hk("ctrl+super+alt+shift+F12").to_string(),
            "SUPER+CTRL+ALT+SHIFT+F12"
        );
        assert_eq!(hk("ALT+prior").to_string(), "ALT+PAGE_UP");
        assert_eq!(hk("ALT+V+SHIFT").to_string(), "ALT+SHIFT+V");
        assert_eq!(hk("SUPER+ALT+V").modmask(), 72);
        assert_eq!(hk("ALT+SHIFT+SPACE").modmask(), 9);
        assert_eq!(hk("SUPER+ALT+V").label(), "Super + Alt + V");
        assert_eq!(hk("ALT+SPACE").labels(), ["Alt", "Espaço"]);
    }

    #[test]
    fn invalid_hotkeys_are_refused_with_a_reason() {
        use HotkeyError::*;
        let err = |text: &str| text.parse::<Hotkey>().expect_err(text);
        assert_eq!(err(""), Empty);
        assert_eq!(err(" + "), Empty);
        assert_eq!(err("SPACE"), NeedsModifier);
        assert_eq!(err("D"), NeedsModifier);
        assert_eq!(err("SHIFT+SPACE"), NeedsModifier);
        assert_eq!(err("SHIFT+A"), NeedsModifier);
        assert_eq!(err("ALT"), MissingKey);
        assert_eq!(err("ALT+SHIFT"), MissingKey);
        assert_eq!(err("ALT+"), MissingKey);
        assert_eq!(err("ALT++D"), MissingKey);
        assert_eq!(err("ALT+ALT+D"), Repeated("Alt".into()));
        assert_eq!(err("ALT+D+D"), Repeated("D".into()));
        assert_eq!(err("ALT+D+E"), TwoKeys("D".into(), "E".into()));
        assert_eq!(err("ALT+ESC"), Escape);
        assert_eq!(err("CTRL+ESCAPE"), Escape);
        assert_eq!(err("ALT+ç"), UnknownKey("Ç".into()));
        assert_eq!(err("ALT+XF86AudioMute"), UnknownKey("XF86AUDIOMUTE".into()));
        assert_eq!(
            err("ALT+SPACE\"); os.execute(\"x"),
            UnknownKey("SPACE\"); OS.EXECUTE(\"X".into())
        );
        assert_eq!(err(&"ALT+".repeat(20)), TooLong);
        // Nenhum nome aceito pode levar aspas, espaço ou algo fora de [A-Z0-9_].
        for key in KEYS {
            assert!(
                key.name
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'),
                "{}",
                key.name
            );
        }
    }

    /// Valores de `linux/input-event-codes.h`, copiados à parte da tabela:
    /// o keycode xkb é o evdev + 8.
    #[test]
    fn keycodes_match_linux_input_event_codes() {
        const EVDEV: &[(&str, u32)] = &[
            ("A", 30),
            ("B", 48),
            ("C", 46),
            ("D", 32),
            ("E", 18),
            ("F", 33),
            ("G", 34),
            ("H", 35),
            ("I", 23),
            ("J", 36),
            ("K", 37),
            ("L", 38),
            ("M", 50),
            ("N", 49),
            ("O", 24),
            ("P", 25),
            ("Q", 16),
            ("R", 19),
            ("S", 31),
            ("T", 20),
            ("U", 22),
            ("V", 47),
            ("W", 17),
            ("X", 45),
            ("Y", 21),
            ("Z", 44),
            ("1", 2),
            ("2", 3),
            ("3", 4),
            ("4", 5),
            ("5", 6),
            ("6", 7),
            ("7", 8),
            ("8", 9),
            ("9", 10),
            ("0", 11),
            ("F1", 59),
            ("F2", 60),
            ("F3", 61),
            ("F4", 62),
            ("F5", 63),
            ("F6", 64),
            ("F7", 65),
            ("F8", 66),
            ("F9", 67),
            ("F10", 68),
            ("F11", 87),
            ("F12", 88),
            ("SPACE", 57),
            ("RETURN", 28),
            ("TAB", 15),
            ("BACKSPACE", 14),
            ("DELETE", 111),
            ("INSERT", 110),
            ("HOME", 102),
            ("END", 107),
            ("PAGE_UP", 104),
            ("PAGE_DOWN", 109),
            ("UP", 103),
            ("DOWN", 108),
            ("LEFT", 105),
            ("RIGHT", 106),
            ("MINUS", 12),
            ("EQUAL", 13),
            ("BRACKETLEFT", 26),
            ("BRACKETRIGHT", 27),
            ("SEMICOLON", 39),
            ("APOSTROPHE", 40),
            ("GRAVE", 41),
            ("BACKSLASH", 43),
            ("COMMA", 51),
            ("DOT", 52),
            ("SLASH", 53),
            ("SYSRQ", 99),
            ("PAUSE", 119),
        ];
        assert_eq!(EVDEV.len(), KEYS.len(), "toda tecla da tabela é conferida");
        for (name, evdev) in EVDEV {
            let name = if *name == "DOT" { "PERIOD" } else { name };
            let key = Key::find(name).unwrap_or_else(|| panic!("{name} na tabela"));
            assert_eq!(key.keycode, evdev + 8, "{name}");
        }
        // KEY_LEFTSHIFT 42, KEY_RIGHTSHIFT 54, KEY_LEFTCTRL 29, KEY_RIGHTCTRL 97,
        // KEY_LEFTALT 56, KEY_RIGHTALT 100, KEY_LEFTMETA 125, KEY_RIGHTMETA 126, KEY_ESC 1.
        assert_eq!(Modifier::Shift.keycodes(), [42 + 8, 54 + 8]);
        assert_eq!(Modifier::Ctrl.keycodes(), [29 + 8, 97 + 8]);
        assert_eq!(Modifier::Alt.keycodes(), [56 + 8, 100 + 8]);
        assert_eq!(Modifier::Super.keycodes(), [125 + 8, 126 + 8]);
        assert_eq!(ESC_KEYCODE, 1 + 8);
        // Nomes, códigos e keycodes únicos.
        for (i, a) in KEYS.iter().enumerate() {
            for b in &KEYS[i + 1..] {
                assert_ne!(a.name, b.name);
                assert_ne!(a.code, b.code);
                assert_ne!(a.keycode, b.keycode, "{} e {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn release_keycodes_cover_the_key_and_both_sides_of_each_modifier() {
        assert_eq!(hk("ALT+SPACE").release_keycodes(), [65, 64, 108]);
        assert_eq!(
            hk("SUPER+CTRL+D").release_keycodes(),
            [40, 133, 134, 37, 105]
        );
        assert_eq!(
            hk("CTRL+SHIFT+F9").release_keycodes(),
            [75, 37, 105, 50, 62]
        );
    }

    #[test]
    fn defaults_are_the_hotkeys_from_before() {
        let config = HotkeyConfig::default();
        let hotkeys = Hotkeys::from_config(&config).expect("padrões válidos");
        assert_eq!(hotkeys.dictate.to_string(), "ALT+SPACE");
        assert_eq!(hotkeys.hands_free.to_string(), "ALT+SHIFT+SPACE");
        assert_eq!(hotkeys.paste_last.to_string(), "SUPER+ALT+V");
        for action in Action::ALL {
            assert_eq!(action.get(&config), action.default_text());
            assert_eq!(Action::from_config_key(action.config_key()), Some(action));
        }
    }

    #[test]
    fn the_three_hotkeys_must_differ() {
        let config = HotkeyConfig {
            hands_free: "alt+space".into(),
            ..HotkeyConfig::default()
        };
        let err = Hotkeys::from_config(&config).expect_err("repetido");
        assert!(err.contains("hotkey.dictate e hotkey.hands_free"), "{err}");
        let config = HotkeyConfig {
            paste_last: "SPACE".into(),
            ..HotkeyConfig::default()
        };
        let err = Hotkeys::from_config(&config).expect_err("inválido");
        assert!(err.starts_with("hotkey.paste_last = \"SPACE\""), "{err}");
    }

    #[test]
    fn rendered_file_snapshot() {
        let config = HotkeyConfig {
            dictate: "super+d".into(),
            hands_free: "ctrl+alt+space".into(),
            paste_last: "super+alt+v".into(),
            ..HotkeyConfig::default()
        };
        let text = render(&Hotkeys::from_config(&config).expect("válidos"));
        assert_eq!(
            text,
            "# FastWisper: atalhos gerados da config ([hotkey]).\n\
             # Não edite: `fastwisper config set hotkey.<ação> ...` (ou o hub) reescreve.\n\
             version 1\n\
             dictate SUPER+D 40 133 134\n\
             hands_free CTRL+ALT+SPACE\n\
             paste_last SUPER+ALT+V\n"
        );
        assert!(text.len() < FILE_MAX_BYTES);
    }

    /// O Lua embute os padrões no mesmo formato, para quando o arquivo falta.
    #[test]
    fn lua_defaults_match_the_generator() {
        let lua = include_str!("../../../packaging/omarchy/fastwisper.lua");
        let start = lua.find("local DEFAULTS = [[\n").expect("DEFAULTS no Lua") + 20;
        let end = start + lua[start..].find("]]").expect("fim de DEFAULTS");
        let defaults = render(&Hotkeys::from_config(&HotkeyConfig::default()).expect("padrões"));
        let body: String = defaults
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(&lua[start..end], body);
        assert!(lua.contains(&format!("local KEY_ESC = {ESC_KEYCODE}\n")));
        assert!(lua.contains(&format!("{BIND_DESCRIPTION_PREFIX} dictate (hold)")));
        assert!(lua.contains(&format!("{FILE_MAX_BYTES}")));
    }

    fn binds() -> Vec<Bind> {
        serde_json::from_str(
            r#"[
              {"modmask":8,"key":"SPACE","keycode":0,"submap":"","mouse":false,"description":"FastWisper: dictate (hold)","dispatcher":"__lua","arg":"1"},
              {"modmask":64,"key":"SPACE","keycode":0,"submap":"","mouse":false,"description":"Omarchy menu","dispatcher":"__lua","arg":"2"},
              {"modmask":64,"key":"d","keycode":0,"submap":"","mouse":false,"description":"","dispatcher":"exec","arg":"foot"},
              {"modmask":72,"key":"Prior","keycode":0,"submap":"","mouse":false,"description":"Página","dispatcher":"__lua","arg":"3"},
              {"modmask":4,"key":"","keycode":65,"submap":"","mouse":false,"description":"Por keycode","dispatcher":"__lua","arg":"4"},
              {"modmask":64,"key":"E","keycode":0,"submap":"resize","mouse":false,"description":"Outro submapa","dispatcher":"__lua","arg":"5"},
              {"modmask":64,"key":"mouse:272","keycode":0,"submap":"","mouse":true,"description":"Mover","dispatcher":"__lua","arg":"6"},
              {"modmask":64,"key":"comma","keycode":0,"submap":"","mouse":false,"description":"Vírgula","dispatcher":"__lua","arg":"7","extra":true}
            ]"#,
        )
        .expect("JSON do hyprctl")
    }

    #[test]
    fn conflicts_come_from_other_binds_with_the_same_mods_and_key() {
        let binds = binds();
        let summaries = |text: &str| -> Vec<String> {
            conflicts(&hk(text), &binds)
                .iter()
                .map(|b| b.summary())
                .collect()
        };
        assert!(summaries("ALT+SPACE").is_empty(), "o nosso não conta");
        assert_eq!(summaries("SUPER+SPACE"), ["Omarchy menu"]);
        assert_eq!(summaries("SUPER+D"), ["exec foot"]);
        assert_eq!(summaries("SUPER+ALT+PAGE_UP"), ["Página"]);
        assert_eq!(summaries("CTRL+SPACE"), ["Por keycode"]);
        assert_eq!(summaries("SUPER+COMMA"), ["Vírgula"]);
        assert!(summaries("SUPER+E").is_empty(), "submapa não é o padrão");
        assert!(
            summaries("SUPER+SHIFT+D").is_empty(),
            "modificadores diferentes"
        );
        assert_eq!(binds[2].combo(), "SUPER+D");
        assert_eq!(binds[3].combo(), "SUPER+ALT+PRIOR");
    }

    /// `hyprctl` falso que anota as chamadas. O script é gravado por um `cp`
    /// à parte: um `fork` de outro teste em paralelo herdaria um descritor
    /// de escrita aberto aqui, e executar o script falharia com ETXTBSY.
    fn fake_hyprctl(dir: &Path, binds: &str) -> PathBuf {
        let log = dir.join("calls");
        let script = dir.join("hyprctl");
        let source = dir.join("hyprctl.fonte");
        std::fs::write(dir.join("binds.json"), binds).expect("binds");
        std::fs::write(
            &source,
            format!(
                "#!/bin/bash\necho \"$*\" >> '{}'\ncase \"$*\" in\n  'binds -j') cat '{}' ;;\n  reload) echo ok ;;\n  *) exit 3 ;;\nesac\n",
                log.display(),
                dir.join("binds.json").display()
            ),
        )
        .expect("script");
        let copied = Command::new("cp").arg(&source).arg(&script).status();
        assert!(copied.expect("cp").success());
        let chmod = Command::new("chmod").arg("755").arg(&script).status();
        assert!(chmod.expect("chmod").success());
        script
    }

    #[test]
    fn hyprctl_binds_and_reload_with_a_fake() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hypr = Hyprctl::with_program(fake_hyprctl(dir.path(), r#"[{"modmask":64,"key":"D"}]"#));
        let binds = hypr.binds().expect("binds");
        assert_eq!(conflicts(&hk("SUPER+D"), &binds).len(), 1);
        hypr.reload().expect("reload");
        let calls = std::fs::read_to_string(dir.path().join("calls")).expect("log");
        assert_eq!(calls, "binds -j\nreload\n");
        std::fs::write(dir.path().join("binds.json"), "não é JSON").expect("binds");
        assert!(matches!(hypr.binds(), Err(HyprError::Json(_))));
        let off = Hyprctl {
            program: dir.path().join("hyprctl"),
            running: false,
            timeout: HYPRCTL_TIMEOUT,
        };
        assert!(matches!(off.binds(), Err(HyprError::NotRunning)));
    }

    #[test]
    fn preview_refuses_duplicates_and_lists_foreign_conflicts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binds = serde_json::to_string(&binds()).expect("JSON");
        let hypr = Hyprctl::with_program(fake_hyprctl(dir.path(), &binds));
        let config = HotkeyConfig::default();
        let p = preview(&config, Action::Dictate, "super + space", &hypr).expect("prévia");
        assert_eq!(p.combo, "SUPER+SPACE");
        assert_eq!(p.labels, ["Super", "Espaço"]);
        assert!(p.checked);
        assert_eq!(
            p.conflicts,
            [ConflictView {
                description: "Omarchy menu".into(),
                combo: "SUPER+SPACE".into()
            }]
        );
        // O próprio atalho atual da ação não conflita (o bind é nosso).
        let same = preview(&config, Action::Dictate, "ALT+SPACE", &hypr).expect("prévia");
        assert!(same.conflicts.is_empty());
        let err =
            preview(&config, Action::Dictate, "alt+shift+space", &hypr).expect_err("duplicado");
        assert_eq!(
            err,
            "Alt + Shift + Espaço já é o atalho de Mãos livres; escolha outro"
        );
        assert!(preview(&config, Action::PasteLast, "SPACE", &hypr).is_err());
        let off = Hyprctl {
            program: dir.path().join("hyprctl"),
            running: false,
            timeout: HYPRCTL_TIMEOUT,
        };
        let p = preview(&config, Action::Dictate, "SUPER+F9", &off).expect("prévia");
        assert!(!p.checked && p.note.is_some());
    }

    /// Hyprland travado: `hyprctl` que não sai é morto no prazo (A-27).
    #[test]
    fn hung_hyprctl_hits_the_deadline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("lento.fonte");
        let script = dir.path().join("hyprctl");
        std::fs::write(&source, "#!/bin/sh\nexec sleep 30\n").expect("script");
        assert!(
            Command::new("cp")
                .arg(&source)
                .arg(&script)
                .status()
                .expect("cp")
                .success()
        );
        assert!(
            Command::new("chmod")
                .arg("755")
                .arg(&script)
                .status()
                .expect("chmod")
                .success()
        );
        let hypr = Hyprctl::with_program(&script).with_timeout(Duration::from_millis(300));
        let started = Instant::now();
        assert!(matches!(
            hypr.binds(),
            Err(HyprError::Timeout("binds -j", 1))
        ));
        assert!(matches!(
            hypr.reload(),
            Err(HyprError::Timeout("reload", 1))
        ));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let file = dir.path().join("hotkeys.conf");
        let custom = HotkeyConfig {
            dictate: "SUPER+D".into(),
            ..HotkeyConfig::default()
        };
        match apply(&custom, &file, Some(&hypr)).expect("gravado") {
            Applied::Written {
                reload_error: Some(e),
            } => assert!(e.contains("não respondeu"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn apply_writes_atomically_reloads_once_and_skips_untouched_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hypr = Hyprctl::with_program(fake_hyprctl(dir.path(), "[]"));
        let file = dir.path().join("data/fastwisper/hotkeys.conf");
        let calls = || std::fs::read_to_string(dir.path().join("calls")).unwrap_or_default();

        // Padrões sem arquivo: o Lua já os usa; nada a gravar nem recarregar.
        let defaults = HotkeyConfig::default();
        assert_eq!(
            apply(&defaults, &file, Some(&hypr)).expect("ok"),
            Applied::Unchanged
        );
        // Só os limites de gravação mudaram, ou o atalho padrão escrito de
        // outro jeito: continua sem arquivo e sem recarga (A-26).
        let limits = HotkeyConfig {
            max_recording_secs: 60,
            min_recording_ms: 500,
            dictate: "alt + space".into(),
            ..HotkeyConfig::default()
        };
        assert_eq!(
            apply(&limits, &file, Some(&hypr)).expect("ok"),
            Applied::Unchanged
        );
        assert!(!file.exists());
        assert_eq!(calls(), "");

        let custom = HotkeyConfig {
            dictate: "SUPER+D".into(),
            ..HotkeyConfig::default()
        };
        assert_eq!(
            apply(&custom, &file, Some(&hypr)).expect("ok"),
            Applied::Reloaded
        );
        assert!(
            std::fs::read_to_string(&file)
                .expect("arquivo")
                .contains("dictate SUPER+D 40 133 134\n")
        );
        assert_eq!(calls(), "reload\n");
        assert_eq!(
            apply(&custom, &file, Some(&hypr)).expect("ok"),
            Applied::Unchanged
        );
        assert_eq!(calls(), "reload\n", "sem mudança, sem recarga");

        // De volta aos padrões com arquivo existente: regrava e recarrega.
        assert_eq!(
            apply(&defaults, &file, Some(&hypr)).expect("ok"),
            Applied::Reloaded
        );
        assert!(
            std::fs::read_to_string(&file)
                .expect("arquivo")
                .contains("dictate ALT+SPACE 65 64 108\n")
        );
        // Sem Hyprland: só grava.
        assert_eq!(
            apply(&custom, &file, None).expect("ok"),
            Applied::Written { reload_error: None }
        );
        assert_eq!(calls(), "reload\nreload\n");
        // Inválido não toca no arquivo.
        let before = std::fs::read(&file).expect("arquivo");
        let bad = HotkeyConfig {
            paste_last: "SUPER+D".into(),
            ..custom
        };
        assert!(matches!(
            apply(&bad, &file, Some(&hypr)),
            Err(ApplyError::Invalid(_))
        ));
        assert_eq!(std::fs::read(&file).expect("arquivo"), before);
        let leftovers = std::fs::read_dir(file.parent().expect("pasta"))
            .expect("dir")
            .count();
        assert_eq!(leftovers, 1, "sem temporário sobrando");
    }
}
