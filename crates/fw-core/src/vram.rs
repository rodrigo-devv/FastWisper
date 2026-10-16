//! Perfis de VRAM (M7): quando o FastWisper mantém os modelos na GPU.
//!
//! A política é pura e mora no actor do daemon; carga e descarga rodam fora
//! dele. Cada pedido leva um `seq` crescente: um pedido mais velho que o
//! último aplicado é ignorado, então uma descarga atrasada nunca derruba a
//! carga de um ditado mais novo.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::session::Ticket;

/// Perfil configurado em `[performance] vram`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VramMode {
    /// Segue o detector de jogo: jogo aberto → `jogo`; fora dele, o perfil base.
    #[default]
    Automatico,
    /// Modelos sempre carregados (`keep_alive: -1`).
    Desempenho,
    /// Carrega no press e descarrega depois de `unload_after_secs` ocioso.
    SobDemanda,
    /// ASR sob demanda na GPU, descarga logo após o ditado; LLM conforme `game_llm`.
    Jogo,
}

impl VramMode {
    pub const ALL: [VramMode; 4] = [
        VramMode::Automatico,
        VramMode::Desempenho,
        VramMode::SobDemanda,
        VramMode::Jogo,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VramMode::Automatico => "automatico",
            VramMode::Desempenho => "desempenho",
            VramMode::SobDemanda => "sob_demanda",
            VramMode::Jogo => "jogo",
        }
    }

    pub fn from_name(name: &str) -> Option<VramMode> {
        VramMode::ALL.into_iter().find(|m| m.as_str() == name)
    }
}

impl fmt::Display for VramMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// LLM no modo jogo (`[performance] game_llm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameLlm {
    /// LLM configurado carregado no press e descarregado logo depois do
    /// ditado (~1 s de carga, feita enquanto a pessoa fala).
    #[default]
    Local,
    /// Só regras: nenhum LLM durante o jogo.
    Off,
    /// Continua usando o provider remoto já consentido (nada na VRAM local).
    Remote,
}

/// Perfil em vigor (o `automatico` já resolvido).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Desempenho,
    SobDemanda,
    Jogo,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Desempenho => "desempenho",
            Profile::SobDemanda => "sob_demanda",
            Profile::Jogo => "jogo",
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Perfil base fora do jogo: sob demanda, confirmado por benchmark de VRAM;
/// 300 s preservam ditados em sequência.
pub const AUTOMATIC_BASE: Profile = Profile::SobDemanda;

/// O que o pipeline precisa saber do perfil em vigor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileSettings {
    pub profile: Profile,
    /// O LLM pode ser usado (jogo com `game_llm = "off"`: não).
    pub llm: bool,
    /// Ocioso por quanto tempo antes de descarregar; `None` = nunca.
    pub unload_after: Option<Duration>,
}

impl Default for ProfileSettings {
    /// Comportamento anterior ao M7: tudo residente.
    fn default() -> Self {
        Self {
            profile: Profile::Desempenho,
            llm: true,
            unload_after: None,
        }
    }
}

/// Máquina de estados dos perfis. Pura: o actor aplica os efeitos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VramPolicy {
    mode: VramMode,
    base: Profile,
    game: bool,
    game_llm: GameLlm,
    unload_after: Duration,
}

impl Default for VramPolicy {
    fn default() -> Self {
        VramPolicy::new(VramMode::default(), Duration::from_secs(300), GameLlm::Off)
    }
}

impl VramPolicy {
    pub fn new(mode: VramMode, unload_after: Duration, game_llm: GameLlm) -> Self {
        Self {
            mode,
            base: AUTOMATIC_BASE,
            game: false,
            game_llm,
            unload_after,
        }
    }

    pub fn from_config(config: &crate::config::PerformanceConfig) -> Self {
        VramPolicy::new(
            config.vram,
            Duration::from_secs(u64::from(config.unload_after_secs)),
            config.game_llm,
        )
    }

    /// Perfil base do `automatico` fora do jogo.
    pub fn with_base(mut self, base: Profile) -> Self {
        self.base = if base == Profile::Jogo {
            AUTOMATIC_BASE
        } else {
            base
        };
        self
    }

    pub fn mode(&self) -> VramMode {
        self.mode
    }

    pub fn game(&self) -> bool {
        self.game
    }

    pub fn profile(&self) -> Profile {
        match self.mode {
            VramMode::Desempenho => Profile::Desempenho,
            VramMode::SobDemanda => Profile::SobDemanda,
            VramMode::Jogo => Profile::Jogo,
            VramMode::Automatico if self.game => Profile::Jogo,
            VramMode::Automatico => self.base,
        }
    }

    /// Troca o perfil configurado. Devolve `(antes, depois)` se o perfil em
    /// vigor mudou.
    pub fn set_mode(&mut self, mode: VramMode) -> Option<(Profile, Profile)> {
        let before = self.profile();
        self.mode = mode;
        changed(before, self.profile())
    }

    /// `[performance]` recarregado da config. O estado do detector fica.
    /// Devolve `(antes, depois)` se o perfil em vigor mudou.
    pub fn reconfigure(
        &mut self,
        config: &crate::config::PerformanceConfig,
    ) -> Option<(Profile, Profile)> {
        let before = self.profile();
        self.mode = config.vram;
        self.unload_after = Duration::from_secs(u64::from(config.unload_after_secs));
        self.game_llm = config.game_llm;
        changed(before, self.profile())
    }

    /// O detector viu um jogo abrir ou fechar. Só muda algo no `automatico`.
    pub fn set_game(&mut self, running: bool) -> Option<(Profile, Profile)> {
        let before = self.profile();
        self.game = running;
        changed(before, self.profile())
    }

    pub fn settings(&self) -> ProfileSettings {
        let profile = self.profile();
        ProfileSettings {
            profile,
            llm: !(profile == Profile::Jogo && self.game_llm == GameLlm::Off),
            unload_after: match profile {
                Profile::Desempenho => None,
                Profile::SobDemanda => Some(self.unload_after),
                // No jogo cada MB conta: o ASR recarrega em ~100 ms.
                Profile::Jogo => Some(Duration::ZERO),
            },
        }
    }
}

fn changed(before: Profile, after: Profile) -> Option<(Profile, Profile)> {
    (before != after).then_some((before, after))
}

/// Quais modelos uma carga/descarga envolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Models {
    pub asr: bool,
    pub llm: bool,
}

impl Models {
    pub const ALL: Models = Models {
        asr: true,
        llm: true,
    };
}

/// Identidade de um pedido de carga/descarga. `seq` é dado pelo actor, em
/// ordem; `ticket` é a sessão que pediu a carga (para o `load_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelOp {
    pub seq: u64,
    pub ticket: Option<Ticket>,
}

/// Estado de um modelo, para o status. Sem conteúdo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    #[default]
    Unloaded,
    Loading,
    Loaded,
    Failed,
    /// Provider que o FastWisper não carrega nem descarrega (remoto,
    /// OpenAI-compatível): nada dele na VRAM sob nossa conta.
    External,
    Disabled,
}

impl ModelState {
    fn from_u8(value: u8) -> ModelState {
        match value {
            1 => ModelState::Loading,
            2 => ModelState::Loaded,
            3 => ModelState::Failed,
            4 => ModelState::External,
            5 => ModelState::Disabled,
            _ => ModelState::Unloaded,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            ModelState::Unloaded => 0,
            ModelState::Loading => 1,
            ModelState::Loaded => 2,
            ModelState::Failed => 3,
            ModelState::External => 4,
            ModelState::Disabled => 5,
        }
    }
}

/// O que está carregado agora e quanto de VRAM isso ocupa (estimativa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Residency {
    pub asr: ModelState,
    pub llm: ModelState,
    /// Bytes de VRAM dos modelos carregados pelo FastWisper; `None` sem medida.
    pub vram_bytes: Option<u64>,
}

impl Default for Residency {
    fn default() -> Self {
        Self {
            asr: ModelState::Loaded,
            llm: ModelState::Disabled,
            vram_bytes: None,
        }
    }
}

/// Perfil e residência no `status`/`subscribe`. Sem texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VramStatus {
    /// Configurado (`automatico`, `desempenho`...).
    pub mode: VramMode,
    /// Em vigor.
    pub profile: Profile,
    /// O detector vê um jogo aberto.
    pub game: bool,
    pub asr: ModelState,
    pub llm: ModelState,
    /// VRAM estimada do FastWisper, em MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_mb: Option<u64>,
    /// Descarga agendada (só no `status`, não nos eventos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unload_in_secs: Option<u64>,
    /// Descarga pedida e ainda não concluída (esperando o ditado em
    /// andamento terminar, ou em execução).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unload_pending: bool,
}

/// Detecta jogo aberto (implementação real: B2). Chamado periodicamente
/// fora do actor; pode ler `/proc` ou D-Bus, nunca rede.
pub trait GameDetector: Send + Sync + 'static {
    fn is_game_running(&self) -> bool;

    /// Intervalo entre consultas; `None` = nunca consulta.
    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(2))
    }
}

/// Padrão enquanto não há detector real: nunca há jogo.
#[derive(Debug, Default)]
pub struct NoGameDetector;

impl GameDetector for NoGameDetector {
    fn is_game_running(&self) -> bool {
        false
    }

    fn poll_interval(&self) -> Option<Duration> {
        None
    }
}

/// Detector ligado à mão (testes).
#[derive(Debug)]
pub struct ManualGameDetector {
    running: AtomicBool,
    interval: Duration,
}

impl ManualGameDetector {
    pub fn new(interval: Duration) -> Self {
        Self {
            running: AtomicBool::new(false),
            interval,
        }
    }

    pub fn set(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
    }
}

impl GameDetector for ManualGameDetector {
    fn is_game_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(self.interval)
    }
}

/// Falha de [`ModelSlot::with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotError {
    /// Ninguém pediu a carga (ou ela foi descartada).
    NotLoaded,
    /// A carga falhou.
    Failed,
    /// Ainda carregando depois da espera máxima.
    Timeout,
}

/// Resultado de uma carga: o modelo e a VRAM que ele ocupou (se medida).
pub type Loaded<E> = (E, Option<u64>);

/// O que [`ModelSlot::ensure`] fez.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ensured {
    /// Carregou a frio, nesta duração.
    Cold(Duration),
    /// Já estava carregado (ou outro pedido carregou).
    Ready,
    /// Carregou, mas uma descarga chegou no meio: o modelo saiu sem uso.
    Discarded,
    /// Pedido velho: uma descarga mais nova venceu; nada feito.
    Stale,
}

struct SlotInner<E> {
    engine: Option<E>,
    loading: bool,
    failed: bool,
    /// `false`: uma descarga chegou durante a carga; o modelo carregado é
    /// descartado assim que ficar pronto.
    keep: bool,
    last_seq: u64,
    /// Carga a frio feita para a sessão desta generation.
    cold: Option<(u64, Duration)>,
}

/// Um modelo local que pode estar ou não carregado (ASR). Bloqueante: use
/// em threads de trabalho, nunca no actor.
///
/// O modelo fica sob o mutex enquanto é usado ([`ModelSlot::with`]), então
/// uma descarga espera a transcrição em andamento terminar.
pub struct ModelSlot<E> {
    inner: Mutex<SlotInner<E>>,
    changed: Condvar,
    state: AtomicU8,
    vram: AtomicU64,
}

impl<E> Default for ModelSlot<E> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(SlotInner {
                engine: None,
                loading: false,
                failed: false,
                keep: false,
                last_seq: 0,
                cold: None,
            }),
            changed: Condvar::new(),
            state: AtomicU8::new(ModelState::Unloaded.to_u8()),
            vram: AtomicU64::new(0),
        }
    }
}

impl<E> ModelSlot<E> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Estado sem travar (status).
    pub fn state(&self) -> ModelState {
        ModelState::from_u8(self.state.load(Ordering::SeqCst))
    }

    /// VRAM do modelo carregado, se medida.
    pub fn vram_bytes(&self) -> Option<u64> {
        (self.state() == ModelState::Loaded)
            .then(|| self.vram.load(Ordering::SeqCst))
            .filter(|b| *b > 0)
    }

    fn set_state(&self, state: ModelState) {
        self.state.store(state.to_u8(), Ordering::SeqCst);
    }

    /// Carrega se ainda não estiver carregado.
    pub fn ensure<F>(&self, op: ModelOp, load: F) -> Result<Ensured, String>
    where
        F: FnOnce() -> Result<Loaded<E>, String>,
    {
        self.ensure_at(Some(op.seq), op.ticket, load)
    }

    /// Quem vai usar o modelo agora garante a carga, na posição atual da
    /// fila (sem passar na frente de pedido nenhum), esperando no máximo
    /// `wait`. A carga, se precisar começar aqui, roda numa thread própria:
    /// uma carga presa (driver, GPU) estoura o prazo em vez de prender quem
    /// chamou; quando terminar, o modelo fica conforme o perfil.
    pub fn ensure_within<F>(
        self: &Arc<Self>,
        ticket: Option<Ticket>,
        wait: Duration,
        load: F,
    ) -> Result<Ensured, String>
    where
        F: FnOnce() -> Result<Loaded<E>, String> + Send + 'static,
        E: Send + 'static,
    {
        let Ok(mut guard) = self.inner.lock() else {
            return Err("estado do modelo envenenado".into());
        };
        let op = ModelOp {
            seq: guard.last_seq,
            ticket,
        };
        guard.keep = true;
        if guard.engine.is_some() {
            return Ok(Ensured::Ready);
        }
        if !guard.loading {
            guard.loading = true;
            guard.failed = false;
            self.set_state(ModelState::Loading);
            let slot = Arc::clone(self);
            let spawned = thread::Builder::new()
                .name("fw-model-load".into())
                .spawn(move || {
                    let started = Instant::now();
                    let result = load();
                    let _ = slot.complete_load(op, result, started.elapsed());
                });
            if spawned.is_err() {
                guard.loading = false;
                guard.failed = true;
                self.set_state(ModelState::Failed);
                return Err("não foi possível iniciar a carga do modelo".into());
            }
        }
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, wait, |s| s.loading)
            .map_err(|_| "estado do modelo envenenado".to_owned())?;
        if guard.loading {
            return Err(format!(
                "a carga do modelo passou do prazo de {} s",
                wait.as_secs()
            ));
        }
        match (&guard.engine, guard.failed) {
            (Some(_), _) => Ok(Ensured::Ready),
            (None, true) => Err("a carga do modelo falhou".into()),
            (None, false) => Err("o modelo foi descarregado durante a carga".into()),
        }
    }

    fn ensure_at<F>(
        &self,
        seq: Option<u64>,
        ticket: Option<Ticket>,
        load: F,
    ) -> Result<Ensured, String>
    where
        F: FnOnce() -> Result<Loaded<E>, String>,
    {
        let Ok(mut guard) = self.inner.lock() else {
            return Err("estado do modelo envenenado".into());
        };
        let op = ModelOp {
            seq: seq.unwrap_or(guard.last_seq),
            ticket,
        };
        if op.seq < guard.last_seq {
            return Ok(Ensured::Stale);
        }
        guard.last_seq = op.seq;
        guard.keep = true;
        let waited = guard.loading;
        while guard.loading {
            guard = match self.changed.wait(guard) {
                Ok(guard) => guard,
                Err(_) => return Err("estado do modelo envenenado".into()),
            };
            if guard.last_seq > op.seq && !guard.keep {
                return Ok(Ensured::Stale);
            }
        }
        // Quem esperou a carga de outro pedido não repete uma que falhou.
        if waited && guard.failed {
            return Err("a carga do modelo falhou".into());
        }
        if guard.engine.is_some() {
            return Ok(Ensured::Ready);
        }
        guard.loading = true;
        guard.failed = false;
        self.set_state(ModelState::Loading);
        drop(guard);

        let started = Instant::now();
        let result = load();
        self.complete_load(op, result, started.elapsed())
    }

    /// Fim de uma carga: guarda o modelo, ou o descarta se uma descarga
    /// chegou no meio, e acorda quem esperava.
    fn complete_load(
        &self,
        op: ModelOp,
        result: Result<Loaded<E>, String>,
        elapsed: Duration,
    ) -> Result<Ensured, String> {
        let Ok(mut guard) = self.inner.lock() else {
            return Err("estado do modelo envenenado".into());
        };
        guard.loading = false;
        let outcome = match result {
            Ok((engine, vram)) if guard.keep => {
                guard.engine = Some(engine);
                guard.cold = op.ticket.map(|t| (t.generation, elapsed));
                self.vram.store(vram.unwrap_or(0), Ordering::SeqCst);
                self.set_state(ModelState::Loaded);
                Ok(Ensured::Cold(elapsed))
            }
            // Descarga pedida durante a carga (`unload`, modo jogo):
            // o modelo recém-carregado sai sem ser usado.
            Ok((engine, _)) => {
                drop(engine);
                self.set_state(ModelState::Unloaded);
                Ok(Ensured::Discarded)
            }
            Err(e) => {
                guard.failed = true;
                self.set_state(ModelState::Failed);
                Err(e)
            }
        };
        drop(guard);
        self.changed.notify_all();
        outcome
    }

    /// Descarrega. Espera o uso em andamento terminar; durante uma carga,
    /// marca o modelo para sair assim que ela acabar. Devolve `true` se
    /// havia um modelo carregado.
    pub fn unload(&self, op: ModelOp) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        if op.seq < guard.last_seq {
            return false;
        }
        guard.last_seq = op.seq;
        guard.keep = false;
        guard.cold = None;
        let engine = guard.engine.take();
        let had = engine.is_some();
        // Solta a memória ainda sob o mutex: uma carga nova só começa depois.
        drop(engine);
        if !guard.loading {
            guard.failed = false;
            self.vram.store(0, Ordering::SeqCst);
            self.set_state(ModelState::Unloaded);
        }
        drop(guard);
        self.changed.notify_all();
        had
    }

    /// Usa o modelo, esperando até `wait` por uma carga em andamento.
    /// Devolve também a carga a frio feita para `generation`, se houve.
    pub fn with<R>(
        &self,
        generation: u64,
        wait: Duration,
        f: impl FnOnce(&mut E) -> R,
    ) -> Result<(R, Option<Duration>), SlotError> {
        let guard = self.inner.lock().map_err(|_| SlotError::Failed)?;
        let (mut guard, _) = self
            .changed
            .wait_timeout_while(guard, wait, |s| s.loading)
            .map_err(|_| SlotError::Failed)?;
        let cold = match guard.cold {
            Some((g, d)) if g == generation => {
                guard.cold = None;
                Some(d)
            }
            _ => None,
        };
        let (loading, failed) = (guard.loading, guard.failed);
        match guard.engine.as_mut() {
            Some(engine) => Ok((f(engine), cold)),
            None if loading => Err(SlotError::Timeout),
            None if failed => Err(SlotError::Failed),
            None => Err(SlotError::NotLoaded),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::session::SessionId;

    fn op(seq: u64) -> ModelOp {
        ModelOp {
            seq,
            ticket: Some(Ticket {
                session_id: SessionId(seq),
                generation: seq,
            }),
        }
    }

    #[test]
    fn automatic_follows_the_game_and_returns_to_the_base() {
        let mut p = VramPolicy::default();
        assert_eq!(p.mode(), VramMode::Automatico);
        assert_eq!(p.profile(), AUTOMATIC_BASE);
        assert_eq!(
            p.set_game(true),
            Some((AUTOMATIC_BASE, Profile::Jogo)),
            "jogo abriu"
        );
        assert_eq!(p.set_game(true), None, "mesmo estado não muda nada");
        assert_eq!(p.set_game(false), Some((Profile::Jogo, AUTOMATIC_BASE)));

        let mut p = VramPolicy::default().with_base(Profile::Desempenho);
        p.set_game(true);
        assert_eq!(
            p.set_game(false),
            Some((Profile::Jogo, Profile::Desempenho))
        );
    }

    #[test]
    fn explicit_modes_ignore_the_detector() {
        for (mode, profile) in [
            (VramMode::Desempenho, Profile::Desempenho),
            (VramMode::SobDemanda, Profile::SobDemanda),
            (VramMode::Jogo, Profile::Jogo),
        ] {
            let mut p = VramPolicy::new(mode, Duration::from_secs(300), GameLlm::Off);
            assert_eq!(p.set_game(true), None);
            assert_eq!(p.set_game(false), None);
            assert_eq!(p.profile(), profile);
        }
        let mut p = VramPolicy::default();
        p.set_game(true);
        assert_eq!(
            p.set_mode(VramMode::Desempenho),
            Some((Profile::Jogo, Profile::Desempenho))
        );
        assert_eq!(
            p.set_mode(VramMode::Automatico),
            Some((Profile::Desempenho, Profile::Jogo)),
            "de volta ao automatico com o jogo ainda aberto"
        );
    }

    #[test]
    fn settings_per_profile() {
        let secs = Duration::from_secs(300);
        let s = VramPolicy::new(VramMode::Desempenho, secs, GameLlm::Off).settings();
        assert_eq!((s.llm, s.unload_after), (true, None));
        let s = VramPolicy::new(VramMode::SobDemanda, secs, GameLlm::Off).settings();
        assert_eq!((s.llm, s.unload_after), (true, Some(secs)));
        let s = VramPolicy::new(VramMode::Jogo, secs, GameLlm::Off).settings();
        assert_eq!((s.llm, s.unload_after), (false, Some(Duration::ZERO)));
        let s = VramPolicy::new(VramMode::Jogo, secs, GameLlm::Local).settings();
        assert_eq!(
            (s.llm, s.unload_after),
            (true, Some(Duration::ZERO)),
            "game_llm = local carrega no press e descarrega logo depois"
        );
        let s = VramPolicy::new(VramMode::Jogo, secs, GameLlm::Remote).settings();
        assert!(s.llm, "game_llm = remote mantém o provider remoto");
        assert_eq!(
            VramPolicy::default().with_base(Profile::Jogo).profile(),
            AUTOMATIC_BASE,
            "jogo não é base"
        );
    }

    #[test]
    fn mode_names_round_trip() {
        for mode in VramMode::ALL {
            assert_eq!(VramMode::from_name(mode.as_str()), Some(mode));
            let json = serde_json::to_string(&mode).expect("json");
            assert_eq!(json, format!("\"{mode}\""));
        }
        assert_eq!(VramMode::from_name("gaming"), None);
    }

    #[test]
    fn slot_loads_once_and_reports_the_cold_load() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        assert_eq!(slot.state(), ModelState::Unloaded);
        let cold = slot.ensure(op(1), || Ok((7, Some(1024)))).expect("carga");
        assert!(matches!(cold, Ensured::Cold(_)));
        assert_eq!(slot.state(), ModelState::Loaded);
        assert_eq!(slot.vram_bytes(), Some(1024));
        let warm = slot
            .ensure(op(2), || Err("não deveria recarregar".into()))
            .expect("já carregado");
        assert_eq!(warm, Ensured::Ready);

        let (value, cold) = slot.with(1, Duration::ZERO, |e| *e).expect("modelo pronto");
        assert_eq!(value, 7);
        assert!(cold.is_some(), "carga a frio da generation 1");
        let (_, cold) = slot.with(1, Duration::ZERO, |e| *e).expect("pronto");
        assert_eq!(cold, None, "a carga a frio conta uma vez só");

        assert!(slot.unload(op(3)));
        assert_eq!(slot.state(), ModelState::Unloaded);
        assert_eq!(slot.vram_bytes(), None);
        assert_eq!(
            slot.with(4, Duration::ZERO, |e| *e).map(|(v, _)| v),
            Err(SlotError::NotLoaded)
        );
    }

    #[test]
    fn stale_unload_never_drops_a_newer_load() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        slot.ensure(op(5), || Ok((1, None))).expect("carga");
        assert!(!slot.unload(op(4)), "descarga mais velha que a carga");
        assert_eq!(slot.state(), ModelState::Loaded);
        assert_eq!(
            slot.ensure(op(3), || Ok((2, None))).expect("velho"),
            Ensured::Stale,
            "carga velha não faz nada"
        );
    }

    #[test]
    fn unload_during_load_discards_the_model_when_it_arrives() {
        let slot: Arc<ModelSlot<u32>> = Arc::new(ModelSlot::new());
        let (started_tx, started_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let loader = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            loader.ensure(op(1), || {
                let _ = started_tx.send(());
                let _ = go_rx.recv();
                Ok((9, Some(1)))
            })
        });
        started_rx.recv().expect("carga começou");
        assert_eq!(slot.state(), ModelState::Loading);
        assert!(!slot.unload(op(2)), "nada carregado ainda");
        go_tx.send(()).expect("libera a carga");
        assert_eq!(handle.join().expect("thread"), Ok(Ensured::Discarded));
        assert_eq!(slot.state(), ModelState::Unloaded);
        assert_eq!(
            slot.with(1, Duration::ZERO, |e| *e).map(|(v, _)| v),
            Err(SlotError::NotLoaded)
        );
    }

    #[test]
    fn unload_waits_for_the_model_in_use() {
        let slot: Arc<ModelSlot<u32>> = Arc::new(ModelSlot::new());
        slot.ensure(op(1), || Ok((1, None))).expect("carga");
        let (using_tx, using_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let user = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            user.with(1, Duration::ZERO, |_| {
                let _ = using_tx.send(());
                let _ = done_rx.recv();
                "transcrito"
            })
        });
        using_rx.recv().expect("em uso");
        let unloader = Arc::clone(&slot);
        let unload = thread::spawn(move || unloader.unload(op(2)));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            slot.state(),
            ModelState::Loaded,
            "descarga não acontece no meio do uso"
        );
        done_tx.send(()).expect("termina o uso");
        assert_eq!(
            handle.join().expect("thread").map(|(r, _)| r),
            Ok("transcrito")
        );
        assert!(unload.join().expect("thread"));
        assert_eq!(slot.state(), ModelState::Unloaded);
    }

    #[test]
    fn stuck_load_times_out_for_the_user_without_blocking() {
        let slot: Arc<ModelSlot<u32>> = Arc::new(ModelSlot::new());
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let started = Instant::now();
        // Carga presa (driver travado): quem vai usar desiste no prazo.
        let result = slot.ensure_within(None, Duration::from_millis(100), move || {
            let _ = go_rx.recv();
            Ok((5, None))
        });
        assert!(result.is_err(), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(slot.state(), ModelState::Loading);
        // Outro pedido também não fica preso esperando.
        let again = slot.ensure_within(None, Duration::from_millis(50), || Ok((6, None)));
        assert!(again.is_err());

        // A carga destrava depois: o modelo fica pronto para o próximo uso.
        go_tx.send(()).expect("destrava");
        let deadline = Instant::now() + Duration::from_secs(2);
        while slot.state() != ModelState::Loaded {
            assert!(Instant::now() < deadline, "carga não terminou");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            slot.ensure_within(None, Duration::ZERO, || Err("não recarrega".into())),
            Ok(Ensured::Ready)
        );
        assert_eq!(slot.with(0, Duration::ZERO, |e| *e).map(|(v, _)| v), Ok(5));
    }

    #[test]
    fn ensure_within_loads_and_reports_the_cold_load() {
        let slot: Arc<ModelSlot<u32>> = Arc::new(ModelSlot::new());
        let ticket = op(7).ticket;
        assert_eq!(
            slot.ensure_within(ticket, Duration::from_secs(2), || Ok((1, None))),
            Ok(Ensured::Ready)
        );
        let (_, cold) = slot.with(7, Duration::ZERO, |e| *e).expect("pronto");
        assert!(cold.is_some(), "carga a frio atribuída à sessão");
        assert!(
            slot.ensure_within(None, Duration::from_secs(2), || Err("x".into()))
                .is_ok(),
            "já carregado"
        );
        let failing: Arc<ModelSlot<u32>> = Arc::new(ModelSlot::new());
        assert!(
            failing
                .ensure_within(None, Duration::from_secs(2), || Err("sem GPU".into()))
                .is_err()
        );
        assert_eq!(failing.state(), ModelState::Failed);
    }

    #[test]
    fn failed_load_is_retried_by_the_next_request() {
        let slot: ModelSlot<u32> = ModelSlot::new();
        assert!(slot.ensure(op(1), || Err("sem GPU".into())).is_err());
        assert_eq!(slot.state(), ModelState::Failed);
        assert_eq!(
            slot.with(1, Duration::ZERO, |e| *e).map(|(v, _)| v),
            Err(SlotError::Failed)
        );
        assert!(matches!(
            slot.ensure(op(2), || Ok((3, None))).expect("ok"),
            Ensured::Cold(_)
        ));
        assert_eq!(slot.state(), ModelState::Loaded);
    }
}
