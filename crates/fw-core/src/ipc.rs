//! Protocolo do socket local: JSONL versionado.
//!
//! Uma requisição por linha, uma resposta por linha. Comandos fora da
//! allowlist, campos desconhecidos e versões diferentes recebem erros tipados.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::inject::Delivery;
use crate::pipeline::{LastSummary, Readiness};
use crate::sandbox::NetworkSandbox;
use crate::session::{Command, Mode, SessionId, Snapshot, StateName};
use crate::vram::{VramMode, VramStatus};

pub const PROTOCOL_VERSION: u32 = 1;
/// Tamanho máximo de uma linha, sem contar o `\n`.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
pub const MAX_REQUEST_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidJson,
    InvalidRequest,
    UnsupportedVersion,
    UnknownCommand,
    InvalidArgs,
    LineTooLong,
    Timeout,
    Busy,
    Unavailable,
    DeviceNotConfigured,
    DeviceUnavailable,
    DeviceAmbiguous,
    DeviceLost,
    RecordingTooLong,
    MicSilent,
    CaptureFailed,
    AsrUnavailable,
    AsrFailed,
    NoResult,
    DeliveryFailed,
    InvalidConfig,
    NotesDisabled,
    NotesStorage,
    NoteNotFound,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidJson => "invalid_json",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::UnsupportedVersion => "unsupported_version",
            ErrorCode::UnknownCommand => "unknown_command",
            ErrorCode::InvalidArgs => "invalid_args",
            ErrorCode::LineTooLong => "line_too_long",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Busy => "busy",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::DeviceNotConfigured => "device_not_configured",
            ErrorCode::DeviceUnavailable => "device_unavailable",
            ErrorCode::DeviceAmbiguous => "device_ambiguous",
            ErrorCode::DeviceLost => "device_lost",
            ErrorCode::RecordingTooLong => "recording_too_long",
            ErrorCode::MicSilent => "mic_silent",
            ErrorCode::CaptureFailed => "capture_failed",
            ErrorCode::AsrUnavailable => "asr_unavailable",
            ErrorCode::AsrFailed => "asr_failed",
            ErrorCode::NoResult => "no_result",
            ErrorCode::DeliveryFailed => "delivery_failed",
            ErrorCode::InvalidConfig => "invalid_config",
            ErrorCode::NotesDisabled => "notes_disabled",
            ErrorCode::NotesStorage => "notes_storage",
            ErrorCode::NoteNotFound => "note_not_found",
        }
    }

    fn default_message(self) -> &'static str {
        match self {
            ErrorCode::InvalidJson => "a linha não é JSON UTF-8 válido",
            ErrorCode::InvalidRequest => "envelope inválido",
            ErrorCode::UnsupportedVersion => "versão de protocolo não suportada",
            ErrorCode::UnknownCommand => "comando desconhecido",
            ErrorCode::InvalidArgs => "argumentos inválidos para o comando",
            ErrorCode::LineTooLong => "linha acima do limite de 64 KiB",
            ErrorCode::Timeout => "tempo esgotado esperando o fim da linha",
            ErrorCode::Busy => "ainda processando o ditado anterior",
            ErrorCode::Unavailable => "o daemon está encerrando",
            ErrorCode::DeviceNotConfigured => {
                "nenhum microfone configurado: rode `fastwisper setup`"
            }
            ErrorCode::DeviceUnavailable => "o microfone configurado não está conectado",
            ErrorCode::DeviceAmbiguous => "mais de um microfone corresponde a audio.device",
            ErrorCode::DeviceLost => "o microfone desconectou durante a gravação",
            ErrorCode::RecordingTooLong => "a gravação passou do limite e foi descartada",
            ErrorCode::MicSilent => {
                "o microfone só captou silêncio (mudo ou desligado no aparelho?)"
            }
            ErrorCode::CaptureFailed => "falha na captura de áudio",
            ErrorCode::AsrUnavailable => "o modelo de transcrição não está disponível",
            ErrorCode::AsrFailed => "falha na transcrição",
            ErrorCode::NoResult => "não há resultado recente (expira em 2 minutos)",
            ErrorCode::DeliveryFailed => "não foi possível copiar para o clipboard",
            ErrorCode::InvalidConfig => "config inválida; a anterior continua valendo",
            ErrorCode::NotesDisabled => "Notas por voz desligado; ative experimental.notes",
            ErrorCode::NotesStorage => "não foi possível acessar o armazenamento de notas",
            ErrorCode::NoteNotFound => "nota não encontrada",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Argumentos tipados: `set_vram` exige `mode`; `notes_copy`/`notes_delete`
/// exigem `id`; os demais comandos aceitam `{}` ou nada.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<VramMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

impl Args {
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.id.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u32,
    pub request_id: String,
    pub cmd: Command,
    #[serde(default, skip_serializing_if = "Args::is_empty")]
    pub args: Args,
}

impl Request {
    pub fn new(request_id: impl Into<String>, cmd: Command) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            request_id: request_id.into(),
            cmd,
            args: Args::default(),
        }
    }

    /// `set_vram` com o perfil novo.
    pub fn set_vram(request_id: impl Into<String>, mode: VramMode) -> Self {
        Self {
            args: Args {
                mode: Some(mode),
                ..Args::default()
            },
            ..Request::new(request_id, Command::SetVram)
        }
    }

    pub fn to_line(&self) -> Vec<u8> {
        to_line(self)
    }
}

/// O que uma recarga da config fez (`reload_config`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadReport {
    /// Chaves com valor diferente do que estava em vigor.
    pub changed: Vec<String>,
    /// Chaves que mudaram mas só valem reiniciando o daemon; até lá fica o
    /// valor do início.
    pub restart_required: Vec<String>,
    /// Há um ditado em andamento: a config nova vale quando ele terminar.
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

/// Resposta a uma linha. Conteúdo só nas consultas explícitas `show_last`
/// e `notes_list`; nunca janela nem áudio.
/// O cliente tolera campos novos (por exemplo, `seq` no M5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub request_id: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr: Option<Readiness>,
    /// Resumo do último ditado: só código e métricas, nunca o texto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<LastSummary>,
    /// Só na resposta de `show_last`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Resultado de `copy_last`/`paste_last`.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    /// Bloqueio de rede do daemon (Landlock); só na resposta de `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkSandbox>,
    /// Perfil de VRAM e modelos carregados (`status`, `subscribe`,
    /// `unload`, `set_vram`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram: Option<VramStatus>,
    /// Resultado de `reload_config`: só nomes de chaves, nunca valores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<ReloadReport>,
    /// Arquivo (absoluto) que o `reload_config` relê; só na resposta de
    /// `status`. Ausente em daemon antigo ou sem config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<crate::notes::Note>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<usize>,
    #[serde(default)]
    pub note_session: bool,
    /// Ordem dos eventos de `subscribe` (lacuna = eventos coalescidos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Ditados que falharam desde o início do daemon (só em `subscribe`;
    /// monotônico). Ausente em daemon antigo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    pub fn ok(request_id: String, snapshot: Snapshot) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            request_id: Some(request_id),
            ok: true,
            state: Some(snapshot.state),
            mode: snapshot.mode,
            session_id: snapshot.session_id,
            generation: Some(snapshot.generation),
            asr: None,
            last: None,
            text: None,
            delivery: None,
            network: None,
            vram: None,
            reload: None,
            config_path: None,
            notes: None,
            deleted: None,
            note_session: false,
            seq: None,
            failures: None,
            error: None,
        }
    }

    pub fn error(request_id: Option<String>, code: ErrorCode) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            request_id,
            ok: false,
            state: None,
            mode: None,
            session_id: None,
            generation: None,
            asr: None,
            last: None,
            text: None,
            delivery: None,
            network: None,
            vram: None,
            reload: None,
            config_path: None,
            notes: None,
            deleted: None,
            note_session: false,
            seq: None,
            failures: None,
            error: Some(ErrorBody {
                code,
                message: code.default_message().to_owned(),
            }),
        }
    }

    /// Erro acompanhado do estado atual (por exemplo, `busy`).
    pub fn rejected(request_id: String, code: ErrorCode, snapshot: Snapshot) -> Self {
        Self {
            ok: false,
            error: Response::error(None, code).error,
            ..Response::ok(request_id, snapshot)
        }
    }

    pub fn error_code(&self) -> Option<ErrorCode> {
        self.error.as_ref().map(|e| e.code)
    }

    pub fn to_line(&self) -> Vec<u8> {
        to_line(self)
    }
}

/// Evento do stream `meter`: só o nível do microfone, um número por quadro
/// (nunca áudio). Sai ~25 Hz enquanto `state` é `recording`; quando a
/// gravação acaba, um último evento com o estado novo e `level` 0, e depois
/// silêncio até a próxima gravação.
/// O cliente tolera campos novos.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeterEvent {
    pub v: u32,
    pub state: StateName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// 0..1, dBFS de −60 a 0 em escala linear, com 3 casas.
    pub level: f32,
}

impl MeterEvent {
    pub fn new(snapshot: Snapshot, level: f32) -> Self {
        let level = if level.is_finite() {
            (level.clamp(0.0, 1.0) * 1000.0).round() / 1000.0
        } else {
            0.0
        };
        Self {
            v: PROTOCOL_VERSION,
            state: snapshot.state,
            session_id: snapshot.session_id,
            level,
        }
    }

    pub fn to_line(&self) -> Vec<u8> {
        to_line(self)
    }
}

/// Requisição rejeitada antes de chegar à sessão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Ecoado na resposta quando pôde ser lido com segurança.
    pub request_id: Option<String>,
    pub code: ErrorCode,
}

impl ParseError {
    pub fn into_response(self) -> Response {
        Response::error(self.request_id, self.code)
    }
}

/// Valida uma linha (sem o `\n`) e devolve a requisição tipada.
pub fn parse_request(line: &[u8]) -> Result<Request, ParseError> {
    let fail = |request_id: Option<String>, code| ParseError { request_id, code };

    let value: Value =
        serde_json::from_slice(line).map_err(|_| fail(None, ErrorCode::InvalidJson))?;
    let Value::Object(mut object) = value else {
        return Err(fail(None, ErrorCode::InvalidRequest));
    };

    let request_id = match object.remove("request_id") {
        Some(Value::String(id)) if is_valid_request_id(&id) => id,
        _ => return Err(fail(None, ErrorCode::InvalidRequest)),
    };
    let id = || Some(request_id.clone());

    match object.remove("v") {
        Some(v) if v.as_u64() == Some(u64::from(PROTOCOL_VERSION)) => {}
        Some(Value::Number(_)) => return Err(fail(id(), ErrorCode::UnsupportedVersion)),
        _ => return Err(fail(id(), ErrorCode::InvalidRequest)),
    }

    let cmd = match object.remove("cmd") {
        Some(Value::String(name)) => {
            Command::from_name(&name).ok_or_else(|| fail(id(), ErrorCode::UnknownCommand))?
        }
        _ => return Err(fail(id(), ErrorCode::InvalidRequest)),
    };

    // Argumentos exclusivos por comando; nenhum caminho ou texto do cliente.
    let args = match object.remove("args") {
        None => Args::default(),
        Some(args @ Value::Object(_)) => {
            serde_json::from_value::<Args>(args).map_err(|_| fail(id(), ErrorCode::InvalidArgs))?
        }
        Some(_) => return Err(fail(id(), ErrorCode::InvalidArgs)),
    };
    let expected = cmd == Command::SetVram;
    let needs_id = matches!(cmd, Command::NotesCopy | Command::NotesDelete);
    if expected != args.mode.is_some()
        || needs_id != args.id.is_some()
        || args.id.is_some_and(|id| id == 0 || id > i64::MAX as u64)
    {
        return Err(fail(id(), ErrorCode::InvalidArgs));
    }

    if !object.is_empty() {
        return Err(fail(id(), ErrorCode::InvalidRequest));
    }

    Ok(Request {
        args,
        ..Request::new(request_id, cmd)
    })
}

fn is_valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REQUEST_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn to_line<T: Serialize>(value: &T) -> Vec<u8> {
    let mut line = serde_json::to_vec(value)
        .expect("tipos do protocolo sempre serializam (sem mapas não-string)");
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> Snapshot {
        Snapshot {
            state: StateName::Recording,
            mode: Some(Mode::Ptt),
            session_id: Some(SessionId(3)),
            generation: 5,
        }
    }

    #[test]
    fn meter_event_is_only_state_and_a_rounded_level() {
        let line = MeterEvent::new(recording(), 0.421_87).to_line();
        assert_eq!(
            String::from_utf8(line).expect("utf-8"),
            "{\"v\":1,\"state\":\"recording\",\"session_id\":3,\"level\":0.422}\n"
        );
        assert_eq!(MeterEvent::new(recording(), f32::NAN).level, 0.0);
        assert_eq!(MeterEvent::new(recording(), 1.7).level, 1.0);
        assert_eq!(MeterEvent::new(recording(), -0.2).level, 0.0);
    }
}
