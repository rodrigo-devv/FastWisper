//! Entrega do texto: clipboard + atalho de colar na
//! janela capturada no `press`, revalidada logo antes. Qualquer dúvida vira
//! copy-only. Nunca envia Enter, salvo a exceção opt-in do "enviar agora"
//! um Enter só, num terminal ou harness, depois de colar na mesma
//! janela.

use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::config::InjectConfig;
use crate::hypr::{self, Hyprland, PasteShortcut, Window};

const WL_COPY_TIMEOUT: Duration = Duration::from_secs(2);
/// Do fim da colagem ao Enter do "enviar agora": o app lê o clipboard de
/// forma assíncrona depois do atalho, e um Enter adiantado iria antes do
/// texto. Intervalo fixo, não confirmação de consumo.
const SUBMIT_DELAY: Duration = Duration::from_millis(200);

/// Destino capturado no `press`. Sem título nem conteúdo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub address: String,
    pub stable_id: Option<String>,
    pub pid: i64,
    pub class: String,
    pub initial_class: String,
    /// Executável da janela (`ghostty`), para apps com classe customizada.
    pub process: Option<String>,
    pub tags: Vec<String>,
    /// Terminal (tag, executável ou `inject.terminal_apps`): Ctrl+Shift+V.
    pub terminal: bool,
    /// Harness desktop configurado: Ctrl+V e regras de vibe coding.
    pub harness: bool,
}

impl Target {
    pub fn from_window(window: Window) -> Target {
        let process = hypr::process_name(window.pid);
        Target {
            address: window.address,
            stable_id: window.stable_id,
            pid: window.pid,
            class: window.class,
            initial_class: window.initial_class,
            process,
            tags: window.tags,
            terminal: false,
            harness: false,
        }
    }

    /// Classifica para a formatação no press; nunca confunde um harness
    /// desktop com um terminal para decidir o atalho de colar.
    pub fn classify(&mut self, config: &InjectConfig) {
        self.terminal = self.is_terminal(config);
        self.harness = !self.terminal && self.matches_any(&config.harness_apps);
    }

    /// Destino das regras de vibe coding (formatação e opt-ins associados).
    pub fn is_terminal_or_harness(&self) -> bool {
        self.terminal || self.harness
    }

    /// Mesma janela: endereço, id estável e processo. Fechar e reabrir uma
    /// janela da mesma classe é outro destino.
    pub fn same_window(&self, other: &Target) -> bool {
        self.address == other.address && self.stable_id == other.stable_id && self.pid == other.pid
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        [
            Some(self.class.as_str()),
            Some(self.initial_class.as_str()),
            self.process.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
    }

    fn matches_any(&self, list: &[String]) -> bool {
        self.names()
            .any(|name| list.iter().any(|item| item.eq_ignore_ascii_case(name)))
    }

    /// `paste_apps` (com `"*"` = qualquer app) e fora de `copy_only_apps`.
    fn paste_allowed(&self, config: &InjectConfig) -> bool {
        let listed = config.paste_apps.iter().any(|item| item == "*")
            || self.matches_any(&config.paste_apps);
        listed && !self.matches_any(&config.copy_only_apps)
    }

    fn is_terminal(&self, config: &InjectConfig) -> bool {
        self.tags
            .iter()
            .any(|t| t == "terminal" || t == "terminal*")
            || self.matches_any(&config.terminal_apps)
            || crate::terminal::process_is_terminal(self.pid)
    }

    /// Destino em que o "enviar agora" pode mandar Enter: terminal ou
    /// harness de IA (`inject.harness_apps`), como classificado no press
    /// (quando a frase foi detectada) **e** ainda pela config em vigor.
    fn send_now_allowed(&self, config: &InjectConfig) -> bool {
        self.is_terminal_or_harness()
            && (self.is_terminal(config) || self.matches_any(&config.harness_apps))
    }
}

/// Por que o texto foi só copiado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyReason {
    /// Pedido explícito (`copy-last`).
    Requested,
    /// O foco saiu da janela do `press`.
    FocusChanged,
    /// Não havia janela com foco no `press` (ou o Hyprland não respondeu).
    NoTarget,
    SessionLocked,
    /// App fora de `inject.paste_apps` ou em `inject.copy_only_apps`.
    AppNotApproved,
    /// O Hyprland recusou o atalho.
    ShortcutFailed,
}

impl CopyReason {
    fn notice(self) -> Option<&'static str> {
        match self {
            CopyReason::Requested => None,
            CopyReason::FocusChanged => Some("o foco mudou durante o ditado"),
            CopyReason::NoTarget => Some("nenhuma janela com foco"),
            CopyReason::SessionLocked => Some("tela bloqueada"),
            CopyReason::AppNotApproved => Some("colagem automática desligada para este app"),
            CopyReason::ShortcutFailed => Some("o atalho de colar falhou"),
        }
    }
}

/// Estado de entrega real. `Pasted` = atalho enviado e aceito pelo
/// Hyprland; não prova que o app consumiu o texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "delivery", content = "reason")]
pub enum Delivery {
    Pasted,
    /// Colado e, depois, um Enter aceito pelo Hyprland na mesma janela
    /// ("enviar agora").
    Submitted,
    Copied(CopyReason),
    /// Nem o clipboard funcionou: o texto só está no `show-last`.
    Failed,
}

/// O que fazer, decidido antes de tocar no clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Paste {
        shortcut: PasteShortcut,
        address: String,
        /// Enter depois de colar ("enviar agora"), se o foco continuar.
        submit: bool,
    },
    CopyOnly(CopyReason),
}

/// Revalida o destino logo antes de entregar. `submit`: o ditado pediu
/// "enviar agora"; só vale com `inject.send_now` e destino de terminal.
pub fn plan(
    captured: Option<&Target>,
    focused: Option<&Target>,
    locked: bool,
    config: &InjectConfig,
    submit: bool,
) -> Plan {
    let Some(captured) = captured else {
        return Plan::CopyOnly(CopyReason::NoTarget);
    };
    if locked {
        return Plan::CopyOnly(CopyReason::SessionLocked);
    }
    match focused {
        Some(now) if now.same_window(captured) => {}
        _ => return Plan::CopyOnly(CopyReason::FocusChanged),
    }
    if !captured.paste_allowed(config) {
        return Plan::CopyOnly(CopyReason::AppNotApproved);
    }
    let shortcut = if captured.is_terminal(config) {
        PasteShortcut::CtrlShiftV
    } else {
        PasteShortcut::CtrlV
    };
    Plan::Paste {
        shortcut,
        address: captured.address.clone(),
        submit: submit && config.send_now && captured.send_now_allowed(config),
    }
}

/// Mundo externo da entrega: janela com foco, clipboard e atalho.
pub trait Desktop: Send + Sync + 'static {
    /// Janela com foco agora (capturada no `press`).
    fn focused(&self) -> impl Future<Output = Option<Target>> + Send;
    /// Entrega automática ao fim do ditado. `submit`: o ditado terminou com
    /// "enviar agora"; a entrega decide se o Enter pode ir.
    fn deliver(
        &self,
        text: String,
        target: Option<Target>,
        submit: bool,
    ) -> impl Future<Output = Delivery> + Send;
    /// `copy-last`: só clipboard.
    fn copy(&self, text: String) -> impl Future<Output = Delivery> + Send;
    /// `paste-last`: cola na janela com foco agora, escolhida pelo usuário.
    fn paste_focused(&self, text: String) -> impl Future<Output = Delivery> + Send;
    /// Aviso curto ao usuário (falha de microfone, modelo...). Sem texto ditado.
    fn notify(&self, message: &str);
    fn reconfigure_language(&self, _language: crate::config::UiLanguage) {}
    /// `[inject]` recarregado da config; vale a partir da próxima entrega.
    fn reconfigure(&self, _config: &InjectConfig) {}
}

/// Sem desktop (testes, ambiente sem Wayland): nada é copiado nem colado.
#[derive(Debug, Default)]
pub struct NoDesktop;

impl Desktop for NoDesktop {
    async fn focused(&self) -> Option<Target> {
        None
    }
    async fn deliver(&self, _text: String, _target: Option<Target>, _submit: bool) -> Delivery {
        Delivery::Failed
    }
    async fn copy(&self, _text: String) -> Delivery {
        Delivery::Failed
    }
    async fn paste_focused(&self, _text: String) -> Delivery {
        Delivery::Failed
    }
    fn notify(&self, _message: &str) {}
}

/// Hyprland + `wl-copy` + notificações do mako.
pub struct HyprDesktop {
    hypr: Option<Hyprland>,
    config: RwLock<Arc<InjectConfig>>,
    language: RwLock<crate::config::UiLanguage>,
    /// Programa do clipboard (`wl-copy`).
    wl_copy: PathBuf,
    /// Tela bloqueada agora?
    locked: fn() -> bool,
}

impl HyprDesktop {
    pub fn new(config: InjectConfig) -> HyprDesktop {
        let hypr = Hyprland::from_env();
        if hypr.is_none() {
            warn!("HYPRLAND_INSTANCE_SIGNATURE ausente: só copy-only");
        }
        HyprDesktop::with_parts(hypr, config, "wl-copy".into(), hypr::session_locked)
    }

    /// Para testes de contrato: Hyprland num socket falso, `wl-copy` falso
    /// e trava de tela decidida pelo teste. Nada toca o desktop real.
    #[doc(hidden)]
    pub fn with_parts(
        hypr: Option<Hyprland>,
        config: InjectConfig,
        wl_copy: PathBuf,
        locked: fn() -> bool,
    ) -> HyprDesktop {
        HyprDesktop {
            hypr,
            config: RwLock::new(Arc::new(config)),
            language: RwLock::new(crate::config::UiLanguage::En),
            wl_copy,
            locked,
        }
    }

    /// Config em vigor (cópia barata; nunca segura o lock num `await`).
    fn config(&self) -> Arc<InjectConfig> {
        Arc::clone(&self.config.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn notification_message(&self, message: &str) -> String {
        let language = *self.language.read().unwrap_or_else(|e| e.into_inner());
        crate::i18n::translate(message, language)
    }

    async fn focused_target(&self) -> Option<Target> {
        let hypr = self.hypr.as_ref()?;
        match hypr.active_window().await {
            Ok(window) => window.map(|w| {
                let mut target = Target::from_window(w);
                target.classify(&self.config());
                target
            }),
            Err(e) => {
                debug!("janela ativa indisponível: {e}");
                None
            }
        }
    }

    async fn execute(&self, text: String, plan: Plan, captured: Option<&Target>) -> Delivery {
        let config = self.config();
        if copy_to_clipboard(&self.wl_copy, &text, !config.clipboard_history)
            .await
            .is_err()
        {
            return Delivery::Failed;
        }
        let delivery = match (&self.hypr, plan) {
            (
                Some(hypr),
                Plan::Paste {
                    shortcut,
                    address,
                    submit,
                },
            ) => match hypr.send_paste(shortcut, &address).await {
                Ok(()) if submit => self.submit(hypr, &address, captured).await,
                Ok(()) => Delivery::Pasted,
                Err(e) => {
                    warn!("atalho de colar falhou: {e}");
                    Delivery::Copied(CopyReason::ShortcutFailed)
                }
            },
            (None, Plan::Paste { .. }) => Delivery::Copied(CopyReason::NoTarget),
            (_, Plan::CopyOnly(reason)) => Delivery::Copied(reason),
        };
        if let Delivery::Copied(reason) = delivery
            && config.notify
            && let Some(why) = reason.notice()
        {
            notify(&self.notification_message(&format!("Texto copiado: cole com Ctrl+V ({why}).")));
        }
        delivery
    }

    /// Enter do "enviar agora", com a colagem já aceita: espera o app
    /// ler o clipboard, revalida foco, trava e opt-in, e manda um Enter sem
    /// modificadores na mesma janela. Qualquer dúvida: fica só colado.
    async fn submit(&self, hypr: &Hyprland, address: &str, captured: Option<&Target>) -> Delivery {
        tokio::time::sleep(SUBMIT_DELAY).await;
        let now = self.focused_target().await;
        let same = matches!((captured, &now), (Some(c), Some(n)) if n.same_window(c) && n.address == address);
        let why = if !same {
            "o foco mudou"
        } else if (self.locked)() {
            "tela bloqueada"
        } else if !self.config().send_now {
            return Delivery::Pasted;
        } else {
            match hypr.send_enter(address).await {
                Ok(()) => return Delivery::Submitted,
                Err(e) => {
                    warn!("Enter do \"enviar agora\" falhou: {e}");
                    "o Enter falhou"
                }
            }
        };
        self.notify(&format!("Texto colado, mas não enviado ({why})."));
        Delivery::Pasted
    }
}

impl Desktop for HyprDesktop {
    async fn focused(&self) -> Option<Target> {
        self.focused_target().await
    }

    fn notify(&self, message: &str) {
        if self.config().notify {
            notify(&self.notification_message(message));
        }
    }

    fn reconfigure_language(&self, language: crate::config::UiLanguage) {
        *self.language.write().unwrap_or_else(|e| e.into_inner()) = language;
    }

    fn reconfigure(&self, config: &InjectConfig) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(config.clone());
    }

    async fn deliver(&self, text: String, target: Option<Target>, submit: bool) -> Delivery {
        let now = self.focused_target().await;
        let plan = plan(
            target.as_ref(),
            now.as_ref(),
            (self.locked)(),
            &self.config(),
            submit,
        );
        self.execute(text, plan, target.as_ref()).await
    }

    async fn copy(&self, text: String) -> Delivery {
        self.execute(text, Plan::CopyOnly(CopyReason::Requested), None)
            .await
    }

    async fn paste_focused(&self, text: String) -> Delivery {
        // Pedido explícito: a janela com foco agora é o destino escolhido,
        // mas tela bloqueada e app não aprovado continuam valendo. Nunca
        // envia Enter.
        let now = self.focused_target().await;
        let plan = plan(
            now.as_ref(),
            now.as_ref(),
            (self.locked)(),
            &self.config(),
            false,
        );
        self.execute(text, plan, None).await
    }
}

/// Texto via stdin (nunca em argumentos, que aparecem em `/proc`).
/// `sensitive` marca `x-kde-passwordManagerHint`: históricos de clipboard que
/// respeitam a marca (o do Omarchy respeita) não guardam o ditado.
async fn copy_to_clipboard(program: &PathBuf, text: &str, sensitive: bool) -> Result<(), ()> {
    let mut cmd = Command::new(program);
    cmd.args(["--type", "text/plain;charset=utf-8"]);
    if sensitive {
        cmd.arg("--sensitive");
    }
    let run = async {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).await?;
        }
        child.wait().await
    };
    match tokio::time::timeout(WL_COPY_TIMEOUT, run).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        other => {
            warn!(ok = matches!(other, Ok(Ok(_))), "wl-copy falhou");
            Err(())
        }
    }
}

/// Notificação curta, sem conteúdo do ditado.
fn notify(message: &str) {
    let spawned = std::process::Command::new("notify-send")
        .args([
            "--app-name=FastWisper",
            "--urgency=low",
            "FastWisper",
            message,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        debug!("notify-send indisponível: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_use_english_by_default_and_new_language_after_reconfigure() {
        let desktop =
            HyprDesktop::with_parts(None, InjectConfig::default(), "fake-wl-copy".into(), || {
                false
            });
        let message = "Texto copiado: cole com Ctrl+V (o foco mudou).";
        assert_eq!(
            desktop.notification_message(message),
            "Text copied: paste with Ctrl+V (focus changed)."
        );
        desktop.reconfigure_language(crate::config::UiLanguage::PtBr);
        assert_eq!(desktop.notification_message(message), message);
        desktop.reconfigure_language(crate::config::UiLanguage::En);
        assert_eq!(
            desktop.notification_message(crate::pipeline::Failure::MicSilent.user_message()),
            "The microphone only captured silence: is it muted or switched off on the device?"
        );
    }

    #[test]
    fn every_daemon_notification_has_an_english_message() {
        use crate::config::UiLanguage::En;
        use crate::pipeline::Failure;
        let desktop =
            HyprDesktop::with_parts(None, InjectConfig::default(), "fake-wl-copy".into(), || {
                false
            });
        let reasons = [
            CopyReason::FocusChanged,
            CopyReason::NoTarget,
            CopyReason::SessionLocked,
            CopyReason::AppNotApproved,
            CopyReason::ShortcutFailed,
        ];
        let mut messages: Vec<String> = reasons
            .into_iter()
            .filter_map(CopyReason::notice)
            .map(|why| format!("Texto copiado: cole com Ctrl+V ({why})."))
            .collect();
        for why in ["o foco mudou", "tela bloqueada", "o Enter falhou"] {
            messages.push(format!("Texto colado, mas não enviado ({why})."));
        }
        messages.extend(
            [
                Failure::DeviceNotConfigured,
                Failure::DeviceUnavailable,
                Failure::DeviceAmbiguous,
                Failure::DeviceLost,
                Failure::RecordingTooLong,
                Failure::MicSilent,
                Failure::CaptureFailed,
                Failure::AsrUnavailable,
                Failure::AsrFailed,
            ]
            .map(|failure| failure.user_message().to_owned()),
        );
        messages.push(crate::daemon::GAME_MODE_NOTICE.to_owned());
        messages.push(crate::gpu_probe::NOTICE.to_owned());
        messages.push("Não foi possível salvar a nota nesta máquina.".to_owned());
        let accented = |text: &str| text.chars().any(|c| "áàâãéêíóôõúç".contains(c));
        for message in messages {
            let english = desktop.notification_message(&message);
            assert_eq!(english, crate::i18n::translate(&message, En));
            assert!(
                english != message && !accented(&english),
                "notificação sem inglês: {english}"
            );
            assert!(
                !["foco", "janela", "atalho", "tela", "falhou"]
                    .iter()
                    .any(|word| english.contains(word)),
                "notificação misturada: {english}"
            );
        }
    }

    fn target(address: &str, class: &str, process: &str) -> Target {
        Target {
            address: address.into(),
            stable_id: Some("7".into()),
            pid: 100,
            class: class.into(),
            initial_class: class.into(),
            process: Some(process.into()),
            tags: vec![],
            terminal: false,
            harness: false,
        }
    }

    fn config() -> InjectConfig {
        InjectConfig {
            paste_apps: vec!["ghostty".into(), "example-browser".into()],
            ..InjectConfig::default()
        }
    }

    #[test]
    fn copy_only_executable_survives_package_replacement() {
        let process = hypr::ScratchProcess::new("deny-scratch");
        let mut cfg = config();
        cfg.paste_apps = vec!["*".into()];
        cfg.copy_only_apps = vec!["deny-scratch".into()];
        for replaced in [false, true] {
            if replaced {
                std::fs::remove_file(&process.executable).expect("unlink own executable");
                std::fs::copy("/usr/bin/sleep", &process.executable).expect("package replacement");
            }
            let t = Target::from_window(Window {
                address: "0x1".into(),
                stable_id: None,
                pid: process.pid(),
                xwayland: false,
                class: "custom-class".into(),
                initial_class: "custom-class".into(),
                tags: vec![],
            });
            assert_eq!(t.process.as_deref(), Some("deny-scratch"));
            assert_eq!(
                plan(Some(&t), Some(&t), false, &cfg, false),
                Plan::CopyOnly(CopyReason::AppNotApproved)
            );
        }
    }

    #[test]
    fn same_focused_approved_window_gets_ctrl_v() {
        let t = target("0x1", "example-browser", "example-browser");
        assert_eq!(
            plan(Some(&t), Some(&t), false, &config(), false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlV,
                address: "0x1".into(),
                submit: false,
            }
        );
    }

    #[test]
    fn terminal_by_process_or_tag_gets_ctrl_shift_v() {
        // Ghostty com classe customizada (org.example.agent): o processo decide.
        let agent = target("0x2", "org.example.agent", "ghostty");
        assert!(matches!(
            plan(Some(&agent), Some(&agent), false, &config(), false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlShiftV,
                ..
            }
        ));
        let mut tagged = target("0x3", "example-browser", "example-browser");
        tagged.tags = vec!["terminal*".into()];
        assert!(matches!(
            plan(Some(&tagged), Some(&tagged), false, &config(), false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlShiftV,
                ..
            }
        ));
    }

    #[test]
    fn terminal_overrides_are_exact_and_copy_only_still_wins() {
        let mut unknown = target("0x5", "unknown-terminal", "unrelated");
        unknown.pid = -1;
        unknown.tags = vec!["terminal***".into()];
        let mut cfg = config();
        assert!(!unknown.is_terminal(&cfg));
        cfg.terminal_apps.push("unknown-terminal".into());
        assert!(unknown.is_terminal(&cfg));
        cfg.copy_only_apps.push("unknown-terminal".into());
        assert_eq!(
            plan(Some(&unknown), Some(&unknown), false, &cfg, false),
            Plan::CopyOnly(CopyReason::AppNotApproved)
        );
        cfg.terminal_apps.clear();
        unknown.class = "terminal-editor".into();
        assert!(!unknown.is_terminal(&cfg));
    }

    #[test]
    fn desktop_harness_uses_vibe_rules_but_keeps_ctrl_v_and_copy_only_wins() {
        // Classe sintética de teste; não é um padrão nem classe de app real.
        let mut harness = target("0x8", "fixture.ai.desktop", "fixture-ai");
        harness.pid = -1;
        let mut cfg = InjectConfig::default();
        assert!(cfg.harness_apps.is_empty());
        harness.classify(&cfg);
        assert!(!harness.is_terminal_or_harness());
        cfg.harness_apps = vec!["FIXTURE.AI.DESKTOP".into()];
        harness.classify(&cfg);
        assert!(harness.harness);
        assert!(!harness.terminal);
        assert!(harness.is_terminal_or_harness());
        assert_eq!(
            plan(Some(&harness), Some(&harness), false, &cfg, false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlV,
                address: "0x8".into(),
                submit: false,
            }
        );
        cfg.copy_only_apps.push("fixture-ai".into());
        assert_eq!(
            plan(Some(&harness), Some(&harness), false, &cfg, false),
            Plan::CopyOnly(CopyReason::AppNotApproved)
        );
        cfg.copy_only_apps.clear();
        cfg.paste_apps.clear();
        assert_eq!(
            plan(Some(&harness), Some(&harness), false, &cfg, false),
            Plan::CopyOnly(CopyReason::AppNotApproved)
        );
    }

    #[test]
    fn terminal_harnesses_keep_terminal_shortcut_and_matching_is_exact() {
        let mut cfg = InjectConfig {
            harness_apps: vec!["fixture.ai.desktop".into(), "ghostty".into()],
            ..InjectConfig::default()
        };
        let mut terminal = target("0x9", "custom-terminal", "ghostty");
        terminal.pid = -1;
        terminal.classify(&cfg);
        assert!(terminal.terminal);
        assert!(!terminal.harness);
        assert!(terminal.is_terminal_or_harness());
        assert!(matches!(
            plan(Some(&terminal), Some(&terminal), false, &cfg, false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlShiftV,
                ..
            }
        ));
        let mut initial = target("0xa", "custom-class", "electron");
        initial.pid = -1;
        initial.initial_class = "fixture.ai.desktop".into();
        initial.classify(&cfg);
        assert!(initial.harness);
        initial.initial_class = "fixture.ai.desktop-other".into();
        initial.classify(&cfg);
        assert!(!initial.harness);
        cfg.harness_apps = vec!["electron".into()];
        initial.classify(&cfg);
        assert!(initial.harness);
        let other = target("0xb", "custom-class", "electron");
        assert_eq!(
            plan(Some(&initial), Some(&other), false, &cfg, false),
            Plan::CopyOnly(CopyReason::FocusChanged)
        );
        assert_eq!(
            plan(Some(&initial), Some(&initial), true, &cfg, false),
            Plan::CopyOnly(CopyReason::SessionLocked)
        );
    }

    #[test]
    fn focus_change_or_recreated_window_is_copy_only() {
        let t = target("0x1", "example-browser", "example-browser");
        let other = target("0x9", "example-browser", "example-browser");
        assert_eq!(
            plan(Some(&t), Some(&other), false, &config(), false),
            Plan::CopyOnly(CopyReason::FocusChanged)
        );
        assert_eq!(
            plan(Some(&t), None, false, &config(), false),
            Plan::CopyOnly(CopyReason::FocusChanged)
        );
        // Mesma classe e até mesmo endereço, mas janela recriada.
        let mut reopened = t.clone();
        reopened.stable_id = Some("8".into());
        assert_eq!(
            plan(Some(&t), Some(&reopened), false, &config(), false),
            Plan::CopyOnly(CopyReason::FocusChanged)
        );
    }

    #[test]
    fn locked_session_unapproved_app_and_no_target_never_paste() {
        let t = target("0x1", "example-browser", "example-browser");
        assert_eq!(
            plan(Some(&t), Some(&t), true, &config(), false),
            Plan::CopyOnly(CopyReason::SessionLocked)
        );
        let unknown = target("0x4", "example-app", "electron");
        assert_eq!(
            plan(Some(&unknown), Some(&unknown), false, &config(), false),
            Plan::CopyOnly(CopyReason::AppNotApproved)
        );
        assert_eq!(
            plan(None, Some(&t), false, &config(), false),
            Plan::CopyOnly(CopyReason::NoTarget)
        );
    }

    #[test]
    fn default_pastes_anywhere_except_copy_only_apps() {
        let mut config = InjectConfig::default();
        // Casos comuns: kitty e um agente (kitty com outra classe).
        let mut agent = target("0x5", "org.example.agent", "kitty");
        agent.tags = vec!["terminal*".into()];
        let electron = target("0x6", "example-app", "electron");
        for t in [&agent, &target("0x7", "kitty", "kitty")] {
            assert!(matches!(
                plan(Some(t), Some(t), false, &config, false),
                Plan::Paste {
                    shortcut: PasteShortcut::CtrlShiftV,
                    ..
                }
            ));
        }
        assert!(matches!(
            plan(Some(&electron), Some(&electron), false, &config, false),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlV,
                ..
            }
        ));
        config.copy_only_apps = vec!["Example-App".into()];
        assert_eq!(
            plan(Some(&electron), Some(&electron), false, &config, false),
            Plan::CopyOnly(CopyReason::AppNotApproved)
        );
        // A trava continua valendo com "*".
        assert_eq!(
            plan(Some(&agent), Some(&agent), true, &config, false),
            Plan::CopyOnly(CopyReason::SessionLocked)
        );
    }

    fn submit_of(plan: Plan) -> Option<bool> {
        match plan {
            Plan::Paste { submit, .. } => Some(submit),
            Plan::CopyOnly(_) => None,
        }
    }

    #[test]
    fn send_now_enter_only_with_opt_in_terminal_and_paste() {
        let mut cfg = InjectConfig::default();
        let mut terminal = target("0x1", "kitty", "kitty");
        terminal.pid = -1;
        terminal.classify(&cfg);
        let mut browser = target("0x2", "example-browser", "example-browser");
        browser.pid = -1;
        browser.classify(&cfg);
        // Opt-in desligado (padrão): nunca, mesmo pedido.
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&terminal), false, &cfg, true)),
            Some(false)
        );
        cfg.send_now = true;
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&terminal), false, &cfg, true)),
            Some(true)
        );
        // Ditado sem a frase, ou app que não é terminal: sem Enter.
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&terminal), false, &cfg, false)),
            Some(false)
        );
        assert_eq!(
            submit_of(plan(Some(&browser), Some(&browser), false, &cfg, true)),
            Some(false)
        );
        // Harness desktop da E0 (classe sintética): Enter com Ctrl+V; fora
        // da lista na entrega, nada de Enter.
        let mut harness = target("0x3", "fixture.ai.desktop", "fixture-ai");
        harness.pid = -1;
        cfg.harness_apps = vec!["fixture.ai.desktop".into()];
        harness.classify(&cfg);
        assert!(matches!(
            plan(Some(&harness), Some(&harness), false, &cfg, true),
            Plan::Paste {
                shortcut: PasteShortcut::CtrlV,
                submit: true,
                ..
            }
        ));
        cfg.harness_apps.clear();
        assert_eq!(
            submit_of(plan(Some(&harness), Some(&harness), false, &cfg, true)),
            Some(false)
        );
        // Classificado como app comum no press: a frase nem foi detectada.
        browser.terminal = false;
        assert_eq!(
            submit_of(plan(Some(&browser), Some(&browser), false, &cfg, true)),
            Some(false)
        );
        // Foco trocado, tela bloqueada ou copy-only: nem cola.
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&browser), false, &cfg, true)),
            None
        );
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&terminal), true, &cfg, true)),
            None
        );
        cfg.copy_only_apps = vec!["kitty".into()];
        assert_eq!(
            submit_of(plan(Some(&terminal), Some(&terminal), false, &cfg, true)),
            None
        );
    }

    #[test]
    fn delivery_serializes_as_flat_fields() {
        let json =
            serde_json::to_string(&Delivery::Copied(CopyReason::FocusChanged)).expect("json");
        assert_eq!(json, r#"{"delivery":"copied","reason":"focus_changed"}"#);
        let json = serde_json::to_string(&Delivery::Pasted).expect("json");
        assert_eq!(json, r#"{"delivery":"pasted"}"#);
        let json = serde_json::to_string(&Delivery::Submitted).expect("json");
        assert_eq!(json, r#"{"delivery":"submitted"}"#);
    }
}
