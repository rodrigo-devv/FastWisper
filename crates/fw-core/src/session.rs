//! Máquina de estados da sessão de ditado.
//!
//! Pura e síncrona: o daemon serializa os eventos num único actor e aplica os
//! [`Effect`]s devolvidos. Captura e inferência rodam fora daqui.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Como a gravação foi iniciada. Define quem pode encerrá-la.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Push-to-talk: `press` inicia, `release` encerra.
    Ptt,
    /// Hands-free: `toggle` inicia e encerra; `release` é ignorado.
    Toggle,
    /// Iniciada e encerrada exclusivamente por comandos de nota do hub.
    Note,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Ptt => "ptt",
            Mode::Toggle => "toggle",
            Mode::Note => "note",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Identidade carregada por todo trabalho da sessão (captura, ASR, entrega).
/// Um resultado só é aceito se o ticket ainda for o da sessão corrente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub session_id: SessionId,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Idle,
    Recording {
        ticket: Ticket,
        mode: Mode,
    },
    Transcribing {
        ticket: Ticket,
    },
}

/// Nome público do estado, usado no IPC e no status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateName {
    Idle,
    Recording,
    Transcribing,
}

impl fmt::Display for StateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StateName::Idle => "idle",
            StateName::Recording => "recording",
            StateName::Transcribing => "transcribing",
        })
    }
}

/// Comandos aceitos pela sessão. É também a allowlist do IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Status,
    NoteStart,
    NoteSave,
    NoteCancel,
    NotesList,
    NotesDelete,
    NotesCopy,
    NotesClear,
    Press,
    Release,
    Toggle,
    Cancel,
    /// Devolve o último texto (pedido explícito; nunca vai no status).
    ShowLast,
    /// Copia o último texto para o clipboard, sem colar.
    CopyLast,
    /// Cola o último texto na janela com foco agora.
    PasteLast,
    /// Mantém a conexão aberta e recebe o status a cada mudança (barra).
    Subscribe,
    /// Libera a VRAM agora (adiado até o fim do ditado em andamento).
    Unload,
    /// Troca o perfil de VRAM em tempo real (`args.mode`).
    SetVram,
    /// Mantém a conexão aberta e recebe o nível do microfone (~25 Hz)
    /// enquanto grava (pílula). Separado do `subscribe`.
    Meter,
    /// Relê a config do daemon e aplica o que dá sem reiniciar (adiado até
    /// o fim do ditado em andamento).
    ReloadConfig,
}

impl Command {
    pub const ALL: [Command; 20] = [
        Command::Status,
        Command::NoteStart,
        Command::NoteSave,
        Command::NoteCancel,
        Command::NotesList,
        Command::NotesDelete,
        Command::NotesCopy,
        Command::NotesClear,
        Command::Press,
        Command::Release,
        Command::Toggle,
        Command::Cancel,
        Command::ShowLast,
        Command::CopyLast,
        Command::PasteLast,
        Command::Subscribe,
        Command::Unload,
        Command::SetVram,
        Command::Meter,
        Command::ReloadConfig,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Command::Status => "status",
            Command::NoteStart => "note_start",
            Command::NoteSave => "note_save",
            Command::NoteCancel => "note_cancel",
            Command::NotesList => "notes_list",
            Command::NotesDelete => "notes_delete",
            Command::NotesCopy => "notes_copy",
            Command::NotesClear => "notes_clear",
            Command::Press => "press",
            Command::Release => "release",
            Command::Toggle => "toggle",
            Command::Cancel => "cancel",
            Command::ShowLast => "show_last",
            Command::CopyLast => "copy_last",
            Command::PasteLast => "paste_last",
            Command::Subscribe => "subscribe",
            Command::Unload => "unload",
            Command::SetVram => "set_vram",
            Command::Meter => "meter",
            Command::ReloadConfig => "reload_config",
        }
    }

    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.as_str() == name)
    }
}

/// Trabalho que o daemon deve executar depois de uma transição.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Abrir a captura do microfone para esta sessão.
    StartCapture(Ticket),
    /// Fechar a captura e processar o áudio (ASR → entrega).
    FinishCapture(Ticket),
    /// Fechar a captura e descartar buffers e resultados desta sessão.
    Abort(Ticket),
}

/// Recusa tipada de um comando.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Ainda processando a sessão anterior. Sem fila invisível no MVP.
    Busy,
}

/// Foto do estado, sem conteúdo (nada de texto, janela ou áudio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub state: StateName,
    pub mode: Option<Mode>,
    pub session_id: Option<SessionId>,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub snapshot: Snapshot,
    pub effect: Option<Effect>,
}

#[derive(Debug, Default)]
pub struct Session {
    state: State,
    generation: u64,
    last_session: u64,
    stale_completions: u64,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Completions descartados por pertencerem a sessão cancelada ou antiga.
    pub fn stale_completions(&self) -> u64 {
        self.stale_completions
    }

    pub fn snapshot(&self) -> Snapshot {
        let (state, mode, ticket) = match self.state {
            State::Idle => (StateName::Idle, None, None),
            State::Recording { ticket, mode } => (StateName::Recording, Some(mode), Some(ticket)),
            State::Transcribing { ticket } => (StateName::Transcribing, None, Some(ticket)),
        };
        Snapshot {
            state,
            mode,
            session_id: ticket.map(|t| t.session_id),
            generation: self.generation,
        }
    }

    pub fn handle(&mut self, command: Command) -> Result<Transition, Rejection> {
        let effect = match (command, self.state) {
            (
                Command::Status
                | Command::ShowLast
                | Command::CopyLast
                | Command::PasteLast
                | Command::Subscribe
                | Command::Unload
                | Command::SetVram
                | Command::Meter
                | Command::ReloadConfig
                | Command::NoteCancel
                | Command::NotesList
                | Command::NotesDelete
                | Command::NotesCopy
                | Command::NotesClear,
                _,
            ) => None,

            (Command::NoteStart, State::Idle) => Some(self.start(Mode::Note)),
            (
                Command::NoteStart,
                State::Recording {
                    mode: Mode::Note, ..
                },
            ) => None,
            (Command::NoteStart, _) => return Err(Rejection::Busy),
            (
                Command::NoteSave,
                State::Recording {
                    ticket,
                    mode: Mode::Note,
                },
            ) => {
                self.state = State::Transcribing { ticket };
                Some(Effect::FinishCapture(ticket))
            }
            (Command::NoteSave, _) => None,
            (
                Command::Release | Command::Toggle,
                State::Recording {
                    mode: Mode::Note, ..
                },
            ) => None,
            (Command::Press, State::Idle) => Some(self.start(Mode::Ptt)),
            (Command::Toggle, State::Idle) => Some(self.start(Mode::Toggle)),
            (Command::Press | Command::Toggle, State::Transcribing { .. }) => {
                return Err(Rejection::Busy);
            }
            // `press` repetido na mesma gravação é idempotente, e um `press`
            // durante hands-free não transforma a sessão em PTT.
            (Command::Press, State::Recording { .. }) => None,

            (
                Command::Release,
                State::Recording {
                    ticket,
                    mode: Mode::Ptt,
                },
            )
            | (
                Command::Toggle,
                State::Recording {
                    ticket,
                    mode: Mode::Toggle,
                },
            ) => {
                self.state = State::Transcribing { ticket };
                Some(Effect::FinishCapture(ticket))
            }
            // Soltar Alt depois de Alt+Shift+Space não pode encerrar o hands-free,
            // e `toggle` não converte uma sessão PTT em outra.
            (
                Command::Release,
                State::Recording {
                    mode: Mode::Toggle, ..
                },
            )
            | (
                Command::Toggle,
                State::Recording {
                    mode: Mode::Ptt, ..
                },
            ) => None,
            // Alt e Space disparam releases duplicados; sem gravação é no-op.
            (Command::Release, State::Idle | State::Transcribing { .. }) => None,

            (Command::Cancel, State::Idle) => None,
            (Command::Cancel, State::Recording { ticket, .. } | State::Transcribing { ticket }) => {
                Some(self.invalidate(ticket))
            }
        };
        Ok(Transition {
            snapshot: self.snapshot(),
            effect,
        })
    }

    /// Um worker terminou o processamento de `ticket`. Devolve `false` (e
    /// descarta) se o ticket não for o da sessão em processamento.
    pub fn complete(&mut self, ticket: Ticket) -> bool {
        match self.state {
            State::Transcribing { ticket: current }
                if current == ticket && ticket.generation == self.generation =>
            {
                self.state = State::Idle;
                true
            }
            _ => {
                self.stale_completions += 1;
                false
            }
        }
    }

    /// Encerra a gravação `ticket` sem processar: a captura não pôde começar
    /// (microfone ausente, ASR indisponível) ou passou do tempo máximo.
    /// Ticket de outra sessão, ou sessão que já saiu de `Recording`, é ignorado.
    pub fn abort_recording(&mut self, ticket: Ticket) -> bool {
        match self.state {
            State::Recording {
                ticket: current, ..
            } if current == ticket => {
                self.invalidate(ticket);
                true
            }
            _ => false,
        }
    }

    /// Encerramento do daemon: invalida a sessão ativa, se houver.
    pub fn shutdown(&mut self) -> Option<Effect> {
        match self.state {
            State::Idle => None,
            State::Recording { ticket, .. } | State::Transcribing { ticket } => {
                Some(self.invalidate(ticket))
            }
        }
    }

    fn start(&mut self, mode: Mode) -> Effect {
        self.generation += 1;
        self.last_session += 1;
        let ticket = Ticket {
            session_id: SessionId(self.last_session),
            generation: self.generation,
        };
        self.state = State::Recording { ticket, mode };
        Effect::StartCapture(ticket)
    }

    fn invalidate(&mut self, ticket: Ticket) -> Effect {
        self.generation += 1;
        self.state = State::Idle;
        Effect::Abort(ticket)
    }
}
