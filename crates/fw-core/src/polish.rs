//! Texto do ASR → texto final: regras e, quando vale a pena, o LLM com prazo
//! duro. Qualquer falha do LLM devolve o texto das regras: o ditado nunca
//! falha por causa dele.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};
use tracing::{info, warn};

use crate::asr::Language;
use crate::config::{Config, LlmConfig, Style};
use crate::llm::{FormatRequest, LlmClient, LlmOutcome, MAX_LLM_INPUT_CHARS};
use crate::terminal::{
    Executables, TerminalMode, classify, finish_terminal, spoken_options, subcommand_hints,
};
use crate::text::{SendNowMatcher, TextRules};
use crate::vram::{ModelOp, ModelState};

/// Prazo mínimo restante para valer a pena chamar o LLM.
const MIN_LLM_WINDOW: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polished {
    pub text: String,
    pub llm: LlmOutcome,
    /// Tempo gasto com o LLM, quando ele foi chamado.
    pub llm_ms: Option<u64>,
    /// O ditado terminou com uma frase do "enviar agora" (já removida do
    /// texto): a entrega pode mandar Enter depois de colar.
    pub submit: bool,
}

/// Como o LLM entra num ditado, pelo perfil de VRAM com que ele começou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmUse {
    /// Residente (desempenho, e o uso direto da biblioteca): chama o
    /// provider como sempre.
    #[default]
    Resident,
    /// Sob demanda: só chama se a carga pedida no press terminou dentro do
    /// prazo; senão, regras. Nunca dispara uma carga fora da conta.
    OnDemand,
    /// Jogo com `game_llm = "off"`: só regras.
    Off,
}

/// Quem carregou o LLM e em que estado ele está. Cargas e descargas passam
/// em fila (`ops`); o estado é lido sem travar.
struct LlmResidency {
    ops: tokio::sync::Mutex<ResidencyOps>,
    state: watch::Sender<ModelState>,
    vram: AtomicU64,
    /// Carga a frio feita para a sessão desta generation.
    cold: Mutex<Option<(u64, Duration)>>,
}

struct ResidencyOps {
    last_seq: u64,
    /// Foi o FastWisper que carregou o modelo no Ollama (só então descarrega).
    ours: bool,
}

/// O que vem da config e pode mudar numa recarga. Cada ditado usa uma foto.
struct Settings {
    rules: TextRules,
    client: Option<LlmClient>,
    /// `[llm]` que criou `client` (para saber se uma recarga o troca).
    llm: LlmConfig,
    style: Style,
    skip_below_words: usize,
    format_in_terminals: bool,
    timeout: Duration,
    /// "Enviar agora" ligado: frases dos idiomas de `asr.languages`.
    send_now: Option<SendNowMatcher>,
}

impl Settings {
    fn new(config: &Config, client: Option<LlmClient>) -> Settings {
        Settings {
            rules: TextRules::new(&config.dictionary, &config.snippets),
            client,
            llm: config.llm.clone(),
            style: config.llm.style,
            skip_below_words: config.llm.skip_below_words as usize,
            format_in_terminals: config.llm.format_in_terminals,
            timeout: Duration::from_millis(u64::from(config.llm.timeout_ms)),
            send_now: config.inject.send_now.then(|| {
                SendNowMatcher::new(config.asr.languages.iter().flat_map(|language| {
                    config
                        .inject
                        .send_now_phrases
                        .for_language(*language)
                        .iter()
                        .map(String::as_str)
                }))
            }),
        }
    }
}

/// Config de LLM inválida (endpoint, chave) não derruba o ditado: fica só
/// com as regras e registra o motivo.
fn build_client(config: &LlmConfig) -> Option<LlmClient> {
    if !config.enabled {
        return None;
    }
    match LlmClient::new(config) {
        Ok(client) => {
            if client.is_remote() {
                warn!(host = %client.host(), "LLM remoto ativo: o texto ditado sai desta máquina");
            }
            Some(client)
        }
        Err(e) => {
            warn!("LLM desativado: {e}");
            None
        }
    }
}

fn initial_state(client: Option<&LlmClient>) -> ModelState {
    match client {
        None => ModelState::Disabled,
        Some(c) if !c.manages_residency() => ModelState::External,
        Some(_) => ModelState::Unloaded,
    }
}

/// Mesmo provider e modelo: só estilo, prazos e limiares mudaram.
fn same_client(a: &LlmConfig, b: &LlmConfig) -> bool {
    (
        a.enabled,
        a.engine,
        &a.endpoint,
        a.model.trim(),
        a.think,
        a.remote_consent,
        &a.api_key_env,
        &a.api_key_file,
    ) == (
        b.enabled,
        b.engine,
        &b.endpoint,
        b.model.trim(),
        b.think,
        b.remote_consent,
        &b.api_key_env,
        &b.api_key_file,
    )
}

pub struct Polisher {
    settings: RwLock<Arc<Settings>>,
    /// `keep_alive` em vigor, herdado por um cliente novo numa recarga.
    keep_alive: Mutex<Option<Duration>>,
    /// Executáveis do `PATH`, para corrigir nomes de comando no terminal.
    executables: Executables,
    /// Uma geração por vez: cancelar não prova que o provider parou.
    gate: Semaphore,
    residency: LlmResidency,
}

impl Polisher {
    pub fn new(config: &Config) -> Polisher {
        let client = build_client(&config.llm);
        let state = initial_state(client.as_ref());
        Polisher {
            settings: RwLock::new(Arc::new(Settings::new(config, client))),
            keep_alive: Mutex::new(None),
            gate: Semaphore::new(1),
            executables: Executables::from_path_env(),
            residency: LlmResidency {
                ops: tokio::sync::Mutex::new(ResidencyOps {
                    last_seq: 0,
                    ours: false,
                }),
                state: watch::Sender::new(state),
                vram: AtomicU64::new(0),
                cold: Mutex::new(None),
            },
        }
    }

    /// Troca a lista de executáveis (testes, PATH diferente).
    pub fn with_executables(mut self, executables: Executables) -> Polisher {
        self.executables = executables;
        self
    }

    /// Foto da config em vigor (nunca segura o lock num `await`).
    fn settings(&self) -> Arc<Settings> {
        Arc::clone(&self.settings.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn client(&self) -> Option<LlmClient> {
        self.settings().client.clone()
    }

    pub fn llm_enabled(&self) -> bool {
        self.settings().client.is_some()
    }

    /// Config recarregada. Regras, dicionário, estilo, prazo e limiares
    /// valem já para o próximo ditado. Trocar o provider ou o modelo espera
    /// a geração em andamento e descarrega o modelo antigo, se foi o
    /// FastWisper que o carregou; o novo carrega quando o perfil pedir.
    pub async fn reconfigure(&self, op: ModelOp, config: &Config) {
        let current = self.settings();
        if same_client(&current.llm, &config.llm) {
            self.store(Settings::new(config, current.client.clone()));
            return;
        }
        let client = build_client(&config.llm);
        if let Some(client) = &client {
            client.set_keep_alive(*self.keep_alive.lock().unwrap_or_else(|e| e.into_inner()));
        }
        let mut ops = self.residency.ops.lock().await;
        ops.last_seq = ops.last_seq.max(op.seq);
        let Ok(_permit) = self.gate.acquire().await else {
            return;
        };
        let old = self.settings();
        if ops.ours
            && let Some(old) = old.client.as_ref().filter(|c| c.manages_residency())
        {
            match old.unload().await {
                Ok(()) => info!("llm anterior descarregado"),
                Err(e) => warn!("descarga do LLM anterior falhou: {e}"),
            }
        }
        ops.ours = false;
        self.residency.vram.store(0, Ordering::SeqCst);
        if let Ok(mut cold) = self.residency.cold.lock() {
            *cold = None;
        }
        self.residency
            .state
            .send_replace(initial_state(client.as_ref()));
        self.store(Settings::new(config, client));
        info!("provider/modelo do LLM trocado");
    }

    fn store(&self, settings: Settings) {
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(settings);
    }

    /// Carrega o modelo no provider. Falha só é registrada.
    pub async fn warmup(&self) {
        let Some(client) = self.client() else { return };
        let started = Instant::now();
        match client.warmup().await {
            Ok(()) => info!(warmup_ms = started.elapsed().as_millis(), "llm pronto"),
            Err(e) => warn!("aquecimento do LLM falhou (ditado segue só com regras): {e}"),
        }
    }

    /// Estado do LLM para o status (sem travar).
    pub fn llm_state(&self) -> ModelState {
        *self.residency.state.borrow()
    }

    /// VRAM do modelo que o FastWisper carregou, se medida.
    pub fn llm_vram_bytes(&self) -> Option<u64> {
        (self.llm_state() == ModelState::Loaded)
            .then(|| self.residency.vram.load(Ordering::SeqCst))
            .filter(|b| *b > 0)
    }

    /// `keep_alive` dos próximos pedidos ao Ollama; `None` = residente.
    pub fn set_keep_alive(&self, keep_alive: Option<Duration>) {
        *self.keep_alive.lock().unwrap_or_else(|e| e.into_inner()) = keep_alive;
        if let Some(client) = self.client() {
            client.set_keep_alive(keep_alive);
        }
    }

    /// Carga a frio feita para a sessão `generation`, uma vez só.
    pub fn take_cold_load(&self, generation: u64) -> Option<Duration> {
        let mut cold = self.residency.cold.lock().ok()?;
        match *cold {
            Some((g, d)) if g == generation => {
                *cold = None;
                Some(d)
            }
            _ => None,
        }
    }

    /// Carrega o modelo no provider (perfis de VRAM). Pedido mais velho que
    /// o último aplicado é ignorado; falha só é registrada.
    pub async fn load(&self, op: ModelOp) {
        let mut ops = self.residency.ops.lock().await;
        // Depois do lock: uma troca de modelo em andamento termina antes.
        let Some(client) = self.client().filter(|c| c.manages_residency()) else {
            return;
        };
        if op.seq < ops.last_seq {
            return;
        }
        ops.last_seq = op.seq;
        if self.llm_state() == ModelState::Loaded {
            // Reconcilia: o modelo pode ter saído por fora (outro cliente o
            // descarregou, `keep_alive` dele expirou). Na dúvida, fica como
            // está; nada é descarregado aqui.
            match client.resident().await {
                Ok(false) => {
                    info!("llm saiu da memória do Ollama por fora; recarregando");
                    ops.ours = false;
                }
                _ => return,
            }
        }
        self.residency.state.send_replace(ModelState::Loading);
        let started = Instant::now();
        match client.load().await {
            Ok(load) => {
                ops.ours = !load.already_resident;
                let vram = if ops.ours { load.vram_bytes } else { None };
                self.residency
                    .vram
                    .store(vram.unwrap_or(0), Ordering::SeqCst);
                let elapsed = started.elapsed();
                if let (Some(ticket), Ok(mut cold)) = (op.ticket, self.residency.cold.lock()) {
                    *cold = Some((ticket.generation, elapsed));
                }
                info!(
                    load_ms = elapsed.as_millis(),
                    ours = ops.ours,
                    vram_mb = vram.map(|b| b >> 20),
                    "llm carregado"
                );
                self.residency.state.send_replace(ModelState::Loaded);
            }
            Err(e) => {
                warn!("carga do LLM falhou (ditado segue só com regras): {e}");
                self.residency.state.send_replace(ModelState::Failed);
            }
        }
    }

    /// Descarrega o modelo, **só se foi o FastWisper que o carregou**
    /// (`keep_alive: 0` apenas no modelo configurado). Espera a geração em
    /// andamento terminar: nunca descarrega no meio de uma formatação.
    pub async fn unload(&self, op: ModelOp) {
        let mut ops = self.residency.ops.lock().await;
        let Some(client) = self.client().filter(|c| c.manages_residency()) else {
            return;
        };
        if op.seq < ops.last_seq {
            return;
        }
        ops.last_seq = op.seq;
        let Ok(_permit) = self.gate.acquire().await else {
            return;
        };
        if ops.ours {
            match client.unload().await {
                Ok(()) => info!("llm descarregado"),
                Err(e) => warn!("descarga do LLM falhou: {e}"),
            }
        }
        ops.ours = false;
        self.residency.vram.store(0, Ordering::SeqCst);
        if let Ok(mut cold) = self.residency.cold.lock() {
            *cold = None;
        }
        self.residency.state.send_replace(ModelState::Unloaded);
    }

    /// `language`: idioma detectado pelo ASR. `terminal`: destino é um
    /// terminal ou harness de IA (o `literal` de `Pipeline::finish`): o
    /// código decide antes do LLM se a fala é comando (curta, começando por
    /// um programa conhecido: regras de terminal e comandos conhecidos) ou
    /// texto (formatado como em qualquer app); nada muda com
    /// `format_in_terminals = false`. Só aí vale o "enviar agora".
    pub async fn polish(
        &self,
        asr_text: &str,
        language: Option<Language>,
        terminal: bool,
    ) -> Polished {
        self.polish_with(asr_text, language, terminal, LlmUse::Resident)
            .await
    }

    /// [`Polisher::polish`] com o uso do LLM do perfil de VRAM da sessão.
    pub async fn polish_with(
        &self,
        asr_text: &str,
        language: Option<Language>,
        terminal: bool,
        llm_use: LlmUse,
    ) -> Polished {
        let settings = self.settings();
        let mut rules = settings.rules.apply(asr_text);
        // "Enviar agora": sobre a saída das regras e antes do LLM, que
        // nunca vê a frase. Só com destino de vibe coding (terminal/harness).
        let submit = terminal
            && settings
                .send_now
                .as_ref()
                .and_then(|m| m.strip(&rules.text))
                .map(|rest| rules.text = rest)
                .is_some();
        if submit && rules.text.is_empty() {
            // Só a frase: nada a colar nem a enviar.
            return Polished {
                text: String::new(),
                llm: LlmOutcome::SkippedShort,
                llm_ms: None,
                submit: false,
            };
        }
        // Comando × texto, no código e antes do LLM: fala longa ou com cara
        // de frase nunca recebe as regras de terminal.
        let mode = (terminal && settings.format_in_terminals)
            .then(|| classify(&rules.text, &self.executables));
        if mode == Some(TerminalMode::Command) {
            // "traço la" → "-la" antes do LLM, só quando é comando.
            rules.text = spoken_options(&rules.text, &self.executables);
        }
        let command = mode == Some(TerminalMode::Command);
        // No terminal: comando sem maiúscula nem ponto; frase com pontuação.
        let finish = |text: String| match mode {
            Some(mode) => finish_terminal(&text, mode, &self.executables),
            None => text,
        };
        let skip = |llm| Polished {
            text: finish(rules.text.clone()),
            llm,
            llm_ms: None,
            submit,
        };
        if terminal && !settings.format_in_terminals {
            // Literal de verdade: nem a normalização mexe.
            return Polished {
                text: rules.text.clone(),
                llm: LlmOutcome::SkippedLiteral,
                llm_ms: None,
                submit,
            };
        }
        let Some(client) = &settings.client else {
            return skip(LlmOutcome::Disabled);
        };
        if llm_use == LlmUse::Off {
            return skip(LlmOutcome::SkippedGame);
        }
        if rules.snippet {
            return skip(LlmOutcome::SkippedSnippet);
        }
        // Limiar exclusivo: abaixo de `skip_below_words` não chama o LLM.
        // Comando de terminal é curto por natureza: lá o limiar não vale.
        if !command && rules.word_count() < settings.skip_below_words {
            return skip(LlmOutcome::SkippedShort);
        }
        // Fala longa demais para o contexto do modelo: as regras bastam.
        if rules.text.chars().count() > MAX_LLM_INPUT_CHARS {
            return skip(LlmOutcome::SkippedLong);
        }

        let started = Instant::now();
        let deadline = started + settings.timeout;
        // Sob demanda: a carga começou no press; o ditado não espera além
        // do prazo e nunca dispara uma carga que o FastWisper não contou.
        if llm_use == LlmUse::OnDemand && client.manages_residency() {
            let mut state = self.residency.state.subscribe();
            let settled = tokio::time::timeout_at(
                deadline.into(),
                state.wait_for(|s| !matches!(s, ModelState::Unloaded | ModelState::Loading)),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map(|s| *s);
            let outcome = match settled {
                Some(ModelState::Loaded) => None,
                Some(ModelState::Failed) => Some(LlmOutcome::Unavailable),
                _ => Some(LlmOutcome::NotLoaded),
            };
            if let Some(outcome) = outcome {
                return Polished {
                    llm_ms: Some(started.elapsed().as_millis() as u64),
                    ..skip(outcome)
                };
            }
        }
        let Ok(Ok(_permit)) = tokio::time::timeout_at(deadline.into(), self.gate.acquire()).await
        else {
            return skip(LlmOutcome::Busy);
        };
        // Uma troca de provider/modelo (`reconfigure`) terminou enquanto
        // esperávamos a vaga: o cliente da foto já foi descarregado ou
        // desligado. Usá-lo recarregaria um modelo sem dono (ou mandaria o
        // texto a um LLM desligado); fica com as regras.
        if !same_client(&settings.llm, &self.settings().llm) {
            return skip(LlmOutcome::Busy);
        }
        // Vaga conseguida no fim do prazo: não abre uma geração condenada.
        if deadline.saturating_duration_since(Instant::now()) < MIN_LLM_WINDOW {
            return skip(LlmOutcome::Busy);
        }
        let commands = if command {
            self.executables.suggest(&rules.text)
        } else {
            Vec::new()
        };
        let subcommands = if command {
            subcommand_hints(&rules.text)
        } else {
            Vec::new()
        };
        let command_refs: Vec<&str> = commands.iter().map(String::as_str).collect();
        let subcommand_refs: Vec<&str> = subcommands.iter().map(String::as_str).collect();
        let mut vocabulary = settings.rules.vocabulary();
        vocabulary.extend(command_refs.iter().copied());
        vocabulary.extend(subcommand_refs.iter().copied());
        let request = FormatRequest {
            text: &rules.text,
            language,
            style: settings.style,
            vocabulary: &vocabulary,
            terminal: command.then_some(command_refs.as_slice()),
            subcommands: &subcommand_refs,
        };
        let result = client.format(&request, deadline).await;
        let llm_ms = Some(started.elapsed().as_millis() as u64);
        match result {
            Ok(text) => Polished {
                text: finish(text),
                llm: LlmOutcome::Used,
                llm_ms,
                submit,
            },
            Err(e) => {
                // Só códigos no log: nunca o texto ditado nem a saída.
                info!(
                    outcome = ?e.outcome(),
                    guard = e.guard_reason().map(|r| r.code()),
                    "LLM não usado: {e}"
                );
                Polished {
                    text: finish(rules.text.clone()),
                    llm: e.outcome(),
                    llm_ms,
                    submit,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn config(model: &str) -> Config {
        // Porta 9 em loopback: nenhum Ollama de verdade é contatado.
        let text = format!(
            "[llm]\nendpoint = \"http://127.0.0.1:9\"\nmodel = \"{model}\"\ntimeout_ms = 5000\nskip_below_words = 1\n"
        );
        Config::parse(&text, Path::new("c.toml")).expect("config")
    }

    #[tokio::test]
    async fn generation_waiting_for_a_model_switch_does_not_use_the_old_client() {
        let polisher = Arc::new(Polisher::new(&config("antigo:1b")));
        // A troca segura a vaga enquanto descarrega o modelo antigo.
        let permit = polisher.gate.acquire().await.expect("vaga");
        let waiting = tokio::spawn({
            let polisher = Arc::clone(&polisher);
            async move {
                polisher
                    .polish("um ditado com várias palavras", None, false)
                    .await
            }
        });
        tokio::task::yield_now().await;
        let new = config("novo:1b");
        polisher.store(Settings::new(&new, build_client(&new.llm)));
        drop(permit);
        let out = waiting.await.expect("polish");
        assert_eq!(out.llm, LlmOutcome::Busy, "não chama o cliente antigo");
        assert_eq!(
            out.text, "um ditado com várias palavras",
            "texto das regras"
        );
    }
}
