//! Cliente síncrono do socket, usado pela CLI (chamada a cada tecla do bind)
//! e pelo hub.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::ipc::{ErrorCode, MAX_LINE_BYTES, ReloadReport, Request, Response};
use crate::session::Command;

pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("o fastwisperd não está rodando (sem socket em {0})")]
    NotRunning(PathBuf),
    #[error("falha de comunicação com o fastwisperd: {0}")]
    Io(#[from] io::Error),
    #[error("resposta inválida do fastwisperd")]
    InvalidResponse,
}

/// Envia uma requisição e espera a resposta correspondente.
pub fn send(socket: &Path, request: &Request) -> Result<Response, ClientError> {
    let mut stream = UnixStream::connect(socket).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            ClientError::NotRunning(socket.to_path_buf())
        }
        _ => ClientError::Io(e),
    })?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    stream.write_all(&request.to_line())?;
    stream.shutdown(Shutdown::Write)?;

    let mut line = String::new();
    BufReader::new(stream.take(MAX_LINE_BYTES as u64 + 1)).read_line(&mut line)?;
    let response: Response =
        serde_json::from_str(line.trim_end()).map_err(|_| ClientError::InvalidResponse)?;
    if response.request_id.as_deref() != Some(request.request_id.as_str()) {
        return Err(ClientError::InvalidResponse);
    }
    Ok(response)
}

/// Resposta do daemon a `reload_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reload {
    NotRunning,
    Done(ReloadReport),
    /// Daemon de antes do `reload_config`.
    Outdated,
    Refused(String),
}

/// Pede ao daemon para reler a config. Usado pela CLI (`config set`,
/// `vram`, `ollama`) e pelo hub depois de gravar.
pub fn reload_config(socket: &Path, request_id: &str) -> Result<Reload, ClientError> {
    let request = Request::new(request_id, Command::ReloadConfig);
    let response = match send(socket, &request) {
        Ok(response) => response,
        Err(ClientError::NotRunning(_)) => return Ok(Reload::NotRunning),
        Err(e) => return Err(e),
    };
    if let Some(error) = response.error {
        return Ok(match error.code {
            ErrorCode::UnknownCommand => Reload::Outdated,
            _ => Reload::Refused(error.message),
        });
    }
    Ok(Reload::Done(response.reload.unwrap_or_default()))
}

/// Config que o daemon da sessão relê no `reload_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonConfig {
    NotRunning,
    /// Caminho absoluto informado no `status`.
    Uses(PathBuf),
    /// Daemon de antes do campo `config_path` (ou sem config): não diz.
    Unknown,
}

/// Pergunta ao daemon (`status`) qual config ele usa.
pub fn daemon_config(socket: &Path, request_id: &str) -> Result<DaemonConfig, ClientError> {
    match send(socket, &Request::new(request_id, Command::Status)) {
        Ok(response) => Ok(response
            .config_path
            .map_or(DaemonConfig::Unknown, DaemonConfig::Uses)),
        Err(ClientError::NotRunning(_)) => Ok(DaemonConfig::NotRunning),
        Err(e) => Err(e),
    }
}

/// `a` e `b` são o mesmo arquivo: caminho igual (absoluto) ou o mesmo destino
/// depois de resolver links. Arquivo ausente só casa pelo caminho.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if absolute(a) == absolute(b) {
        return true;
    }
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_follows_links_and_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("config.toml");
        std::fs::write(&real, "").expect("config");
        let link = dir.path().join("link.toml");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        assert!(same_file(&real, &link));
        assert!(same_file(&real, &dir.path().join(".").join("config.toml")));
        assert!(!same_file(&real, &dir.path().join("outra.toml")));
        let missing = dir.path().join("ausente.toml");
        assert!(same_file(&missing, &missing));
        let cwd = std::env::current_dir().expect("cwd");
        assert!(same_file(Path::new("x.toml"), &cwd.join("x.toml")));
    }
}
