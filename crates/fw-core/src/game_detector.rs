//! Detector local: GameMode no bus de sessão e ambientes Steam do próprio UID.
//! Consultado pelo worker da B1 a cada 5 s: até 5 s para entrar/sair do jogo,
//! sem varrer `/proc` a cada press nem manter subprocessos residentes.
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::vram::GameDetector;

/// Porta para testar D-Bus indisponível, zero clientes e clientes ativos.
pub trait GameModeSource: Send + Sync + 'static {
    fn client_count(&self) -> Option<u32>;
}

#[derive(Default)]
pub struct SessionGameMode;

impl GameModeSource for SessionGameMode {
    fn client_count(&self) -> Option<u32> {
        // busctl faz apenas IPC Unix. Não ativa GameMode para consultar, e a
        // chamada tem prazo; ausência de systemd/D-Bus não impede o Steam.
        let output = Command::new("busctl")
            // --user usa $XDG_RUNTIME_DIR/bus sem este override. Um endereço
            // herdado com transporte TCP não deve criar rede no detector.
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .args([
                "--user",
                "--auto-start=no",
                "--timeout=1s",
                "get-property",
                "com.feralinteractive.GameMode",
                "/com/feralinteractive/GameMode",
                "com.feralinteractive.GameMode",
                "ClientCount",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_client_count(&output.stdout)
    }
}

fn parse_client_count(bytes: &[u8]) -> Option<u32> {
    let mut fields = std::str::from_utf8(bytes).ok()?.split_whitespace();
    if fields.next()? != "i" {
        return None;
    }
    let count = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some(count)
}

pub struct SystemGameDetector<S = SessionGameMode> {
    gamemode: S,
    proc_root: PathBuf,
    uid: u32,
}

impl Default for SystemGameDetector {
    fn default() -> Self {
        Self::new(
            SessionGameMode,
            PathBuf::from("/proc"),
            rustix::process::getuid().as_raw(),
        )
    }
}

impl<S: GameModeSource> SystemGameDetector<S> {
    pub fn new(gamemode: S, proc_root: PathBuf, uid: u32) -> Self {
        Self {
            gamemode,
            proc_root,
            uid,
        }
    }

    fn steam_running(&self) -> bool {
        let Ok(entries) = fs::read_dir(&self.proc_root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            if !name.as_encoded_bytes().iter().all(u8::is_ascii_digit) {
                return false;
            }
            // Verificar antes de abrir; conferir também o arquivo aberto para
            // não ler outro UID se o PID for reciclado entre as operações.
            let Ok(meta) = entry.metadata() else {
                return false;
            };
            if !meta.is_dir() || meta.uid() != self.uid {
                return false;
            }
            let Ok(file) = File::open(entry.path().join("environ")) else {
                return false;
            };
            if !file.metadata().is_ok_and(|m| m.uid() == self.uid) {
                return false;
            }
            let mut bytes = Vec::new();
            if file.take(256 * 1024).read_to_end(&mut bytes).is_err() {
                return false;
            }
            steam_environment(&bytes)
        })
    }
}

impl<S: GameModeSource> GameDetector for SystemGameDetector<S> {
    fn is_game_running(&self) -> bool {
        self.gamemode.client_count().is_some_and(|n| n > 0) || self.steam_running()
    }
    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }
}

fn steam_environment(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == 0).any(|field| {
        let value = field
            .strip_prefix(b"SteamGameId=")
            .or_else(|| field.strip_prefix(b"SteamAppId="));
        value.is_some_and(|v| {
            !v.is_empty() && v.iter().all(u8::is_ascii_digit) && v.iter().any(|b| *b != b'0')
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    struct FakeBus(AtomicI32);
    impl GameModeSource for FakeBus {
        fn client_count(&self) -> Option<u32> {
            u32::try_from(self.0.load(Ordering::SeqCst)).ok()
        }
    }
    #[test]
    fn fake_bus_and_steam_enter_and_leave_game_mode() {
        let dir = tempfile::tempdir().expect("proc falso");
        let detector = SystemGameDetector::new(
            FakeBus(AtomicI32::new(-1)),
            dir.path().into(),
            rustix::process::getuid().as_raw(),
        );
        assert!(!detector.is_game_running());
        let pid = dir.path().join("123");
        fs::create_dir(&pid).expect("pid");
        fs::write(pid.join("environ"), b"OTHER=privado\0SteamAppId=999999\0").expect("ambiente");
        assert!(detector.is_game_running(), "Steam funciona sem D-Bus");
        fs::remove_dir_all(pid).expect("jogo fechado");
        assert!(!detector.is_game_running());
        detector.gamemode.0.store(1, Ordering::SeqCst);
        assert!(detector.is_game_running());
        detector.gamemode.0.store(0, Ordering::SeqCst);
        assert!(!detector.is_game_running());
    }
    #[test]
    fn ignores_other_uids_and_non_pid_entries() {
        let dir = tempfile::tempdir().expect("proc");
        for name in ["123", "self"] {
            fs::create_dir(dir.path().join(name)).expect("pid");
            fs::write(dir.path().join(name).join("environ"), b"SteamGameId=999\0").expect("env");
        }
        let uid = rustix::process::getuid().as_raw();
        let detector = SystemGameDetector::new(
            FakeBus(AtomicI32::new(0)),
            dir.path().into(),
            uid.wrapping_add(1),
        );
        assert!(!detector.is_game_running());
        fs::remove_dir_all(dir.path().join("123")).expect("pid");
        let detector = SystemGameDetector::new(FakeBus(AtomicI32::new(0)), dir.path().into(), uid);
        assert!(!detector.is_game_running());
    }
    #[test]
    fn exact_environment_keys_and_valid_ids_only() {
        for env in [
            b"SteamAppId=0\0".as_slice(),
            b"XSteamAppId=12\0",
            b"SteamAppId=abc\0",
            b"SteamGameId=\0",
        ] {
            assert!(!steam_environment(env));
        }
        assert!(steam_environment(b"SteamGameId=18446744073709551615\0"));
        assert_eq!(parse_client_count(b"i 2\n"), Some(2));
        for output in [b"i -1".as_slice(), b"s 2", b"i 2 extra", b""] {
            assert_eq!(parse_client_count(output), None);
        }
    }
}
