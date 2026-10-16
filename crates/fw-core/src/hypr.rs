//! Cliente do socket IPC do Hyprland.
//!
//! Lê a janela ativa (sem título: pode conter assunto privado) e envia o
//! atalho de colar com modificadores explícitos via `hl.dsp.send_key_state`.
//! Com o "enviar agora" (opt-in), também um Enter sem modificadores na
//! mesma janela. O Lua enviado só contém valores validados aqui; texto
//! ditado nunca entra nele (vai pelo clipboard).

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE: u64 = 1 << 20;
/// Intervalo entre `down` e `up`: contorna tecla sintética presa no Hyprland
/// (mesmo valor do "universal paste" do Omarchy).
const KEY_GAP: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum HyprError {
    #[error("Hyprland indisponível: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hyprland não respondeu a tempo")]
    Timeout,
    #[error("resposta inválida do Hyprland")]
    InvalidResponse,
    #[error("Hyprland recusou o comando: {0}")]
    Rejected(String),
    #[error("endereço de janela inválido")]
    InvalidAddress,
}

/// Janela como o Hyprland a descreve. O título é ignorado de propósito.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub address: String,
    /// O Hyprland serializa como string (`"18000006"`).
    #[serde(default)]
    pub stable_id: Option<String>,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub initial_class: String,
    #[serde(default)]
    pub pid: i64,
    #[serde(default)]
    pub xwayland: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteShortcut {
    /// Apps gráficos.
    CtrlV,
    /// Terminais (Ctrl+V é outro comando; Shift+Insert pode colar a seleção primária).
    CtrlShiftV,
}

impl PasteShortcut {
    fn mods(self) -> &'static str {
        match self {
            PasteShortcut::CtrlV => "CTRL",
            PasteShortcut::CtrlShiftV => "CTRL SHIFT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    /// `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`.
    pub fn from_env() -> Option<Hyprland> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        if signature.is_empty()
            || !signature
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return None;
        }
        Some(Hyprland {
            socket: PathBuf::from(runtime)
                .join("hypr")
                .join(signature)
                .join(".socket.sock"),
        })
    }

    pub fn with_socket(socket: PathBuf) -> Hyprland {
        Hyprland { socket }
    }

    async fn request(&self, message: &str) -> Result<String, HyprError> {
        let exchange = async {
            let mut stream = UnixStream::connect(&self.socket).await?;
            stream.write_all(message.as_bytes()).await?;
            let mut out = Vec::new();
            (&mut stream)
                .take(MAX_RESPONSE)
                .read_to_end(&mut out)
                .await?;
            Ok::<_, HyprError>(out)
        };
        let bytes = tokio::time::timeout(TIMEOUT, exchange)
            .await
            .map_err(|_| HyprError::Timeout)??;
        String::from_utf8(bytes).map_err(|_| HyprError::InvalidResponse)
    }

    /// Janela com foco, ou `None` (área de trabalho vazia).
    pub async fn active_window(&self) -> Result<Option<Window>, HyprError> {
        let raw = self.request("j/activewindow").await?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| HyprError::InvalidResponse)?;
        if value.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(None);
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|_| HyprError::InvalidResponse)
    }

    /// Ctrl(+Shift)+V na janela `address`, com modificadores explícitos:
    /// um Alt fisicamente pressionado não se mistura ao atalho.
    pub async fn send_paste(
        &self,
        shortcut: PasteShortcut,
        address: &str,
    ) -> Result<(), HyprError> {
        if !is_valid_address(address) {
            return Err(HyprError::InvalidAddress);
        }
        self.key_state(shortcut, Some(address), KeyState::Down)
            .await?;
        tokio::time::sleep(KEY_GAP).await;
        // Com o down aceito, a colagem aconteceu. O up só libera a tecla
        // sintética; se a janela já fechou (o app reagiu ao paste), solta
        // sem alvo para nada ficar preso.
        if let Err(e) = self.key_state(shortcut, Some(address), KeyState::Up).await {
            tracing::debug!("up na janela falhou ({e}); soltando sem alvo");
            if let Err(e) = self.key_state(shortcut, None, KeyState::Up).await {
                tracing::warn!("não foi possível soltar a tecla sintética: {e}");
            }
        }
        Ok(())
    }

    /// Um Enter, sem modificadores, na janela `address` ("enviar agora",
    /// opt-in). Quem chama garante colagem aceita e foco
    /// revalidado na mesma janela; aqui só se exige um endereço válido.
    pub async fn send_enter(&self, address: &str) -> Result<(), HyprError> {
        let Some(down) = enter_lua(Some(address), KeyState::Down) else {
            return Err(HyprError::InvalidAddress);
        };
        self.eval(&down).await?;
        tokio::time::sleep(KEY_GAP).await;
        let up = enter_lua(Some(address), KeyState::Up).unwrap_or_default();
        if let Err(e) = self.eval(&up).await {
            tracing::debug!("up do Enter na janela falhou ({e}); soltando sem alvo");
            if let Some(up) = enter_lua(None, KeyState::Up)
                && let Err(e) = self.eval(&up).await
            {
                tracing::warn!("não foi possível soltar o Enter sintético: {e}");
            }
        }
        Ok(())
    }

    async fn key_state(
        &self,
        shortcut: PasteShortcut,
        address: Option<&str>,
        state: KeyState,
    ) -> Result<(), HyprError> {
        self.eval(&paste_lua(shortcut, address, state)).await
    }

    async fn eval(&self, lua: &str) -> Result<(), HyprError> {
        match self.request(lua).await?.trim() {
            "ok" => Ok(()),
            other => Err(HyprError::Rejected(other.chars().take(200).collect())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
}

/// Lua do atalho de colar: tecla V com modificadores fixos, numa janela de
/// endereço já validado (sem alvo: superfície com foco). Nunca texto; o
/// único outro Lua de tecla é o Enter opt-in de [`enter_lua`].
pub fn paste_lua(shortcut: PasteShortcut, address: Option<&str>, state: KeyState) -> String {
    let state = match state {
        KeyState::Down => "down",
        KeyState::Up => "up",
    };
    let window = address
        .filter(|a| is_valid_address(a))
        .map(|a| format!(", window = \"address:{a}\""))
        .unwrap_or_default();
    format!(
        "eval hl.dispatch(hl.dsp.send_key_state({{ mods = \"{}\", key = \"V\", state = \"{state}\"{window} }}))",
        shortcut.mods()
    )
}

/// O outro Lua possível: Enter (`Return`), sem modificadores, só com
/// `"enviar agora"`. `down` exige janela de endereço válido (`None` se não
/// houver); só o `up` pode ir sem alvo, para não deixar a tecla presa.
pub fn enter_lua(address: Option<&str>, state: KeyState) -> Option<String> {
    let window = match (address, state) {
        (Some(a), _) if is_valid_address(a) => format!(", window = \"address:{a}\""),
        (None, KeyState::Up) => String::new(),
        _ => return None,
    };
    let state = match state {
        KeyState::Down => "down",
        KeyState::Up => "up",
    };
    Some(format!(
        "eval hl.dispatch(hl.dsp.send_key_state({{ mods = \"\", key = \"Return\", state = \"{state}\"{window} }}))"
    ))
}

/// `0x` + hexadecimal, como o Hyprland imprime.
pub fn is_valid_address(address: &str) -> bool {
    address.strip_prefix("0x").is_some_and(|hex| {
        !hex.is_empty() && hex.len() <= 16 && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Tela bloqueada (hyprlock em execução): nunca colar.
pub fn session_locked() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "hyprlock")
    })
}

/// Nome do executável real da janela; `comm` é mutável e pode ser truncado.
pub fn process_name(pid: i64) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    process_executable(pid)?
        .file_name()?
        .to_str()
        .map(str::to_owned)
}

/// Only strip the kernel marker when the executable inode is actually unlinked.
/// A live executable whose real filename ends in " (deleted)" stays unchanged.
pub(crate) fn process_executable(pid: i64) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::MetadataExt;
    if pid <= 0 {
        return None;
    }
    let proc_path = format!("/proc/{pid}/exe");
    let executable = std::fs::read_link(&proc_path).ok()?;
    let metadata = std::fs::metadata(&proc_path).ok()?;
    let bytes = executable.as_os_str().as_bytes();
    if metadata.nlink() == 0
        && let Some(original) = bytes.strip_suffix(b" (deleted)")
    {
        return Some(std::ffi::OsString::from_vec(original.to_vec()).into());
    }
    Some(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_json_parses_without_title() {
        let raw = r#"{"address":"0x63bb9546e8e0","stableId":"18000006","class":"org.example.agent",
            "initialClass":"org.example.agent","title":"assunto privado","pid":441235,
            "xwayland":false,"tags":["terminal*"]}"#;
        let w: Window = serde_json::from_str(raw).expect("json");
        assert_eq!(w.address, "0x63bb9546e8e0");
        assert_eq!(w.stable_id.as_deref(), Some("18000006"));
        assert_eq!(w.tags, ["terminal*"]);
        assert!(
            !format!("{w:?}").contains("privado"),
            "título não é guardado"
        );
    }

    #[test]
    fn paste_lua_is_only_a_v_chord_never_enter() {
        for shortcut in [PasteShortcut::CtrlV, PasteShortcut::CtrlShiftV] {
            for state in [KeyState::Down, KeyState::Up] {
                let lua = paste_lua(shortcut, Some("0xabc"), state);
                assert!(lua.contains("key = \"V\""), "{lua}");
                assert!(lua.contains("window = \"address:0xabc\""), "{lua}");
                for forbidden in ["Return", "Enter", "KP_Enter", "exec", "os."] {
                    assert!(!lua.contains(forbidden), "{forbidden} em {lua}");
                }
            }
        }
        assert!(
            paste_lua(PasteShortcut::CtrlShiftV, Some("0x1"), KeyState::Down)
                .contains("\"CTRL SHIFT\"")
        );
    }

    #[test]
    fn enter_lua_is_a_bare_return_in_a_valid_window() {
        let down = enter_lua(Some("0xabc"), KeyState::Down).expect("down");
        assert_eq!(
            down,
            "eval hl.dispatch(hl.dsp.send_key_state({ mods = \"\", key = \"Return\", state = \"down\", window = \"address:0xabc\" }))"
        );
        for forbidden in ["CTRL", "SHIFT", "ALT", "SUPER", "exec", "os."] {
            assert!(!down.contains(forbidden), "{forbidden} em {down}");
        }
        // `down` nunca vai sem alvo nem com endereço inválido.
        assert_eq!(enter_lua(None, KeyState::Down), None);
        assert_eq!(enter_lua(Some("0x1\" }) --"), KeyState::Down), None);
        assert_eq!(enter_lua(Some("0x1\" }) --"), KeyState::Up), None);
        let up = enter_lua(None, KeyState::Up).expect("up sem alvo");
        assert!(
            up.contains("state = \"up\"") && !up.contains("window"),
            "{up}"
        );
    }

    #[test]
    fn invalid_address_never_reaches_lua() {
        let lua = paste_lua(PasteShortcut::CtrlV, Some("0x1\" }) --"), KeyState::Up);
        assert!(!lua.contains("window"), "{lua}");
    }

    #[test]
    fn only_hex_addresses_reach_lua() {
        assert!(is_valid_address("0x63bb9546e8e0"));
        for bad in [
            "",
            "0x",
            "63bb",
            "0x12\" }) os.exit() --",
            "0xZZ",
            "0x11111111111111111",
        ] {
            assert!(!is_valid_address(bad), "{bad}");
        }
    }
}

#[cfg(test)]
pub(crate) struct ScratchProcess {
    pub directory: tempfile::TempDir,
    pub executable: PathBuf,
    pub child: std::process::Child,
}

#[cfg(test)]
impl ScratchProcess {
    pub fn new(name: &str) -> Self {
        let directory = tempfile::tempdir().expect("scratch directory");
        let executable = directory.path().join(name);
        std::fs::copy("/usr/bin/sleep", &executable).expect("copy scratch executable");
        // Parallel tests may briefly inherit another thread's copy descriptor
        // across fork before exec closes it. Retry only Linux ETXTBSY.
        let mut attempts = 0;
        let child = loop {
            match std::process::Command::new(&executable).arg("60").spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(26) && attempts < 20 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("start own scratch process: {error}"),
            }
        };
        let process = Self {
            directory,
            executable,
            child,
        };
        // spawn may return before the child's exec is observable in procfs.
        for _ in 0..200 {
            if std::fs::read_link(format!("/proc/{}/exe", process.pid()))
                .is_ok_and(|path| path == process.executable)
            {
                return process;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("scratch child did not exec its copied executable");
    }

    pub fn pid(&self) -> i64 {
        i64::from(self.child.id())
    }
}

#[cfg(test)]
impl Drop for ScratchProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod executable_tests {
    use super::*;

    #[test]
    fn kernel_deleted_marker_is_removed_once_only_for_unlinked_inode() {
        for name in [
            "scratch terminal",
            "scratch (deleted)",
            "scratch (deleted) extra",
        ] {
            let process = ScratchProcess::new(name);
            assert_eq!(process_name(process.pid()).as_deref(), Some(name));
            std::fs::remove_file(&process.executable).expect("unlink own executable");
            assert_eq!(process_name(process.pid()).as_deref(), Some(name));
            assert_eq!(
                process_executable(process.pid()),
                Some(process.executable.clone())
            );
        }
        assert_eq!(process_executable(-1), None);
    }
}
