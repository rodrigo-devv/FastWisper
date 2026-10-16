//! Contrato entre o actor da sessão e o trabalho de captura/ASR.
//!
//! O resultado volta para o actor, que só o guarda se o ticket ainda for o
//! da sessão corrente. Nada de conteúdo é publicado antes dessa checagem.

use std::future::Future;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::asr::Language;
use crate::config::Config;
use crate::inject::Delivery;
use crate::ipc::ErrorCode;
use crate::llm::LlmOutcome;
use crate::session::Ticket;
use crate::vram::{ModelOp, Models, Profile, ProfileSettings, Residency};

/// Por quanto tempo o último texto fica em RAM para `show-last`.
pub const RESULT_TTL: Duration = Duration::from_secs(120);

/// Falhas tipadas da captura/ASR. Viram códigos no IPC, nunca texto livre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// `audio.device` vazio: rode `fastwisper setup`.
    DeviceNotConfigured,
    /// Nenhum microfone corresponde a `audio.device`.
    DeviceUnavailable,
    /// Mais de um microfone corresponde a `audio.device`.
    DeviceAmbiguous,
    /// O microfone sumiu durante a gravação.
    DeviceLost,
    /// Passou de `hotkey.max_recording_secs`: descartado, não truncado.
    RecordingTooLong,
    /// A gravação inteira foi silêncio digital: microfone mudo ou desligado
    /// no próprio aparelho (o PipeWire não sabe).
    MicSilent,
    /// Falha do PipeWire ou da captura.
    CaptureFailed,
    /// Modelo ausente ou que falhou ao carregar.
    AsrUnavailable,
    AsrFailed,
}

impl Failure {
    /// Aviso para o usuário (notificação), sem conteúdo do ditado.
    pub fn user_message(self) -> &'static str {
        match self {
            Failure::DeviceNotConfigured => {
                "Nenhum microfone configurado. Rode `fastwisper setup`."
            }
            Failure::DeviceUnavailable => "O microfone configurado não está conectado.",
            Failure::DeviceAmbiguous => "Mais de um microfone corresponde à config (audio.device).",
            Failure::DeviceLost => "O microfone desconectou; nada foi transcrito.",
            Failure::RecordingTooLong => "A gravação passou do limite e foi descartada.",
            Failure::MicSilent => {
                "O microfone só captou silêncio: ele está mudo ou desligado no próprio aparelho?"
            }
            Failure::CaptureFailed => "Falha na captura de áudio.",
            Failure::AsrUnavailable => {
                "Modelo de transcrição indisponível. Rode `fastwisper doctor`."
            }
            Failure::AsrFailed => "Falha na transcrição.",
        }
    }

    pub fn error_code(self) -> ErrorCode {
        match self {
            Failure::DeviceNotConfigured => ErrorCode::DeviceNotConfigured,
            Failure::DeviceUnavailable => ErrorCode::DeviceUnavailable,
            Failure::DeviceAmbiguous => ErrorCode::DeviceAmbiguous,
            Failure::DeviceLost => ErrorCode::DeviceLost,
            Failure::RecordingTooLong => ErrorCode::RecordingTooLong,
            Failure::MicSilent => ErrorCode::MicSilent,
            Failure::CaptureFailed => ErrorCode::CaptureFailed,
            Failure::AsrUnavailable => ErrorCode::AsrUnavailable,
            Failure::AsrFailed => ErrorCode::AsrFailed,
        }
    }
}

/// Métricas de uma sessão. Sem texto, janela ou áudio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    /// Do `press` até o primeiro buffer do microfone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_start_ms: Option<u64>,
    pub audio_ms: u64,
    /// Trecho com fala depois do corte de silêncio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<u64>,
    /// O que aconteceu com o LLM (usado, pulado, prazo, guarda...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_ms: Option<u64>,
    /// Do release até a entrega (clipboard + atalho) terminar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2e_ms: Option<u64>,
    /// Carga a frio do ASR feita para este ditado (perfis sob demanda).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    /// Carga a frio do LLM feita para este ditado, se terminou a tempo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_load_ms: Option<u64>,
    /// Perfil de VRAM com que o ditado começou.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Transcribed {
        text: String,
        language: Language,
        metrics: Metrics,
        /// Terminou com uma frase do "enviar agora", já fora de `text`
        /// (opt-in): Enter depois de colar, se a entrega
        /// permitir.
        submit: bool,
    },
    /// Nenhuma fala detectada: o ASR nem roda (evita "you" em silêncio).
    NoSpeech(Metrics),
    /// Gravação abaixo de `hotkey.min_recording_ms` (toque acidental).
    TooShort(Metrics),
    Failed(Failure, Metrics),
}

/// Resumo público do último resultado, para o status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSummary {
    pub outcome: OutcomeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    #[serde(flatten)]
    pub metrics: Metrics,
    /// Como o texto chegou ao destino (só para `transcribed`).
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

/// Serializa como uma string só: `"transcribed"`, `"no_speech"`,
/// `"too_short"` ou o código da falha (`"device_lost"`...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutcomeKind {
    Done(DoneKind),
    Failed(Failure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneKind {
    Transcribed,
    NoSpeech,
    TooShort,
}

impl OutcomeKind {
    pub const TRANSCRIBED: OutcomeKind = OutcomeKind::Done(DoneKind::Transcribed);
    pub const NO_SPEECH: OutcomeKind = OutcomeKind::Done(DoneKind::NoSpeech);
    pub const TOO_SHORT: OutcomeKind = OutcomeKind::Done(DoneKind::TooShort);

    pub fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

impl Outcome {
    pub fn summary(&self) -> LastSummary {
        let (outcome, language, metrics) = match self {
            Outcome::Transcribed {
                language, metrics, ..
            } => (OutcomeKind::TRANSCRIBED, Some(*language), *metrics),
            Outcome::NoSpeech(m) => (OutcomeKind::NO_SPEECH, None, *m),
            Outcome::TooShort(m) => (OutcomeKind::TOO_SHORT, None, *m),
            Outcome::Failed(f, m) => (OutcomeKind::Failed(*f), None, *m),
        };
        LastSummary {
            outcome,
            language,
            metrics,
            delivery: None,
        }
    }
}

/// Estado do motor de ASR, mostrado no status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Loading,
    Ready,
    Failed,
}

/// Captura e processamento de uma sessão. O actor espera `start` (que pode
/// recusar a gravação), roda `finish` num worker e chama `abort` ao cancelar.
pub trait Pipeline: Send + Sync + 'static {
    fn start(&self, ticket: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static;
    /// `literal`: o destino do press é um terminal ou harness de IA
    /// (`Target::is_terminal_or_harness`): regras de terminal, sem LLM a
    /// menos que `llm.format_in_terminals`, e só nele o "enviar agora"
    /// (opt-in) é detectado e tirado do texto antes do LLM,
    /// voltando como `Outcome::Transcribed { submit: true, .. }`.
    fn finish(
        &self,
        ticket: Ticket,
        literal: bool,
    ) -> impl Future<Output = Outcome> + Send + 'static;
    fn abort(&self, ticket: Ticket);
    fn readiness(&self) -> Readiness {
        Readiness::Ready
    }
    /// Nível do microfone na gravação em andamento, de 0 a 1 em escala
    /// perceptual ([`crate::capture::level_from_rms`]); 0 sem gravação.
    /// Barato e sem travar: o stream `meter` lê ~25 vezes por segundo.
    fn level(&self) -> f32 {
        0.0
    }
    /// Gravação mais longa que isso é abortada pelo daemon, com o microfone
    /// fechado, mesmo sem `release` (tecla presa, `press` sem par).
    fn max_recording(&self) -> Duration {
        Duration::from_secs(300)
    }

    /// Perfil de VRAM em vigor. Vale a partir do próximo `start`: o ditado
    /// em andamento termina com o perfil com que começou.
    fn set_profile(&self, _settings: ProfileSettings) {}

    /// Carrega `models` (o que já estiver carregado fica). Roda fora do
    /// actor; `op.seq` mais velho que o último pedido aplicado é ignorado.
    fn load(&self, _op: ModelOp, _models: Models) -> impl Future<Output = ()> + Send + 'static {
        std::future::ready(())
    }

    /// Descarrega `models`, só o que o FastWisper carregou. Nunca no meio
    /// de uma transcrição/formatação: espera o uso em andamento terminar.
    fn unload(&self, _op: ModelOp, _models: Models) -> impl Future<Output = ()> + Send + 'static {
        std::future::ready(())
    }

    /// O que está carregado agora. Barato e sem travar (status).
    fn residency(&self) -> Residency {
        Residency::default()
    }

    /// Config recarregada (`reload_config`), já sem as chaves que exigem
    /// reinício. O daemon só chama sem ditado nem worker em andamento. O
    /// que é imediato vale ao retornar; o future termina o que precisa
    /// esperar (trocar o LLM: geração em andamento, descarga do modelo
    /// antigo). `op` ordena essa troca junto das cargas e descargas.
    fn reconfigure(
        &self,
        _op: ModelOp,
        _config: &Config,
    ) -> impl Future<Output = ()> + Send + 'static {
        std::future::ready(())
    }
}

/// Pipeline vazio do M0: não abre microfone nem transcreve nada.
#[derive(Debug, Default)]
pub struct NullPipeline;

impl Pipeline for NullPipeline {
    fn start(&self, _ticket: Ticket) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        std::future::ready(Ok(()))
    }

    fn finish(
        &self,
        _ticket: Ticket,
        _literal: bool,
    ) -> impl Future<Output = Outcome> + Send + 'static {
        std::future::ready(Outcome::NoSpeech(Metrics::default()))
    }

    fn abort(&self, _ticket: Ticket) {}
}

/// Último resultado em RAM: texto com TTL, resumo sem conteúdo.
#[derive(Debug, Default)]
pub struct LastResult {
    text: Option<(String, Instant)>,
    summary: Option<LastSummary>,
    /// Ditados que falharam desde o início do daemon (monotônico): a pílula
    /// avisa quando ele muda, não quando a linha da barra se repete.
    failures: u64,
}

impl LastResult {
    pub fn record(&mut self, outcome: Outcome, now: Instant) {
        if matches!(outcome, Outcome::Failed(..)) {
            self.failures += 1;
        }
        self.summary = Some(outcome.summary());
        self.text = match outcome {
            Outcome::Transcribed { text, .. } if !text.is_empty() => Some((text, now + RESULT_TTL)),
            _ => None,
        };
    }

    /// `cancel`/`clear`/encerramento apagam o texto.
    pub fn clear_text(&mut self) {
        self.text = None;
    }

    pub fn text(&mut self, now: Instant) -> Option<&str> {
        self.expire(now);
        self.text.as_ref().map(|(t, _)| t.as_str())
    }

    pub fn summary(&self) -> Option<LastSummary> {
        self.summary
    }

    pub fn failures(&self) -> u64 {
        self.failures
    }

    pub fn set_e2e(&mut self, e2e: Duration) {
        if let Some(summary) = &mut self.summary {
            summary.metrics.e2e_ms = Some(e2e.as_millis() as u64);
        }
    }

    pub fn set_profile(&mut self, profile: Profile) {
        if let Some(summary) = &mut self.summary {
            summary.metrics.profile = Some(profile);
        }
    }

    pub fn set_delivery(&mut self, delivery: Delivery) {
        if let Some(summary) = &mut self.summary {
            summary.delivery = Some(delivery);
        }
    }

    pub fn expires_at(&self) -> Option<Instant> {
        self.text.as_ref().map(|(_, at)| *at)
    }

    pub fn expire(&mut self, now: Instant) {
        if self.expires_at().is_some_and(|at| at <= now) {
            self.text = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcribed(text: &str) -> Outcome {
        Outcome::Transcribed {
            text: text.into(),
            language: Language::Pt,
            metrics: Metrics::default(),
            submit: false,
        }
    }

    #[test]
    fn text_expires_after_ttl_but_summary_stays() {
        let mut last = LastResult::default();
        let t0 = Instant::now();
        last.record(transcribed("olá"), t0);
        assert_eq!(
            last.text(t0 + RESULT_TTL - Duration::from_millis(1)),
            Some("olá")
        );
        assert_eq!(last.text(t0 + RESULT_TTL), None);
        assert_eq!(
            last.summary().map(|s| s.outcome),
            Some(OutcomeKind::TRANSCRIBED)
        );
    }

    #[test]
    fn non_text_outcome_replaces_previous_text() {
        let mut last = LastResult::default();
        let t0 = Instant::now();
        last.record(transcribed("antigo"), t0);
        last.record(Outcome::NoSpeech(Metrics::default()), t0);
        assert_eq!(last.text(t0), None, "resultado antigo não reaparece");
    }

    #[test]
    fn summary_serializes_without_text() {
        let mut last = LastResult::default();
        last.record(transcribed("segredo"), Instant::now());
        let json = serde_json::to_string(&last.summary()).expect("json");
        assert!(!json.contains("segredo"));
        assert!(json.contains("\"outcome\":\"transcribed\""));
    }

    #[test]
    fn delivery_is_flattened_into_the_summary() {
        use crate::inject::CopyReason;
        let mut last = LastResult::default();
        last.record(transcribed("x"), Instant::now());
        last.set_delivery(Delivery::Copied(CopyReason::FocusChanged));
        let json = serde_json::to_value(last.summary()).expect("json");
        assert_eq!(json["delivery"], "copied");
        assert_eq!(json["reason"], "focus_changed");
        let back: LastSummary = serde_json::from_value(json).expect("ida e volta");
        assert_eq!(
            back.delivery,
            Some(Delivery::Copied(CopyReason::FocusChanged))
        );
    }

    #[test]
    fn failure_outcome_is_a_flat_code() {
        let kind = Outcome::Failed(Failure::DeviceLost, Metrics::default())
            .summary()
            .outcome;
        assert_eq!(kind.as_str(), "device_lost");
        let back: OutcomeKind = serde_json::from_str("\"device_lost\"").expect("json");
        assert_eq!(back, kind);
        let done: OutcomeKind = serde_json::from_str("\"no_speech\"").expect("json");
        assert_eq!(done, OutcomeKind::NO_SPEECH);
    }
}
