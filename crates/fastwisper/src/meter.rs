//! `fastwisper meter [--follow]`: nível do microfone enquanto grava, uma
//! linha JSON por quadro (`{"level":0.42}`, ~25 Hz) para a pílula ler via
//! `Process`. Só o número: o daemon nunca manda áudio.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use clap::Args;
use fw_core::daemon::DaemonPaths;
use fw_core::ipc::{MeterEvent, Request, Response};
use fw_core::session::{Command, StateName};

const RECONNECT: Duration = Duration::from_secs(2);

#[derive(Debug, Args)]
pub struct MeterArgs {
    /// Continua aberto entre gravações e reconecta se o daemon cair.
    /// Sem ele, sai quando a gravação atual (ou a próxima) termina.
    #[arg(long)]
    follow: bool,
}

/// Como uma conexão terminou.
enum End {
    /// A gravação acabou (só sem `--follow`).
    Recording,
    /// O daemon fechou a conexão ou caiu.
    Closed,
    /// O daemon recusou o comando (versão sem `meter`, por exemplo).
    Rejected(String),
}

pub fn run(args: MeterArgs) -> ExitCode {
    let Ok(paths) = DaemonPaths::from_env() else {
        crate::human_error!("fastwisper: XDG_RUNTIME_DIR não definido");
        return ExitCode::from(crate::EXIT_UNREACHABLE);
    };
    if !args.follow {
        let Ok(stream) = UnixStream::connect(&paths.socket) else {
            crate::human_error!("fastwisper: o fastwisperd não está rodando");
            return ExitCode::from(crate::EXIT_UNREACHABLE);
        };
        return match follow(stream, false) {
            End::Recording => ExitCode::SUCCESS,
            End::Closed => ExitCode::from(crate::EXIT_UNREACHABLE),
            End::Rejected(message) => rejected(&message),
        };
    }
    // Daemon fora do ar: um nível 0 (a pílula zera as barras) e nova
    // tentativa a cada 2 s, como o `status --follow`.
    let mut reset_shown = false;
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            reset_shown = false;
            match follow(stream, true) {
                End::Rejected(message) => return rejected(&message),
                End::Recording | End::Closed => {}
            }
        }
        if !reset_shown {
            print_level(0.0);
            reset_shown = true;
        }
        crate::output::wait(RECONNECT);
    }
}

fn rejected(message: &str) -> ExitCode {
    crate::human_error!("fastwisper: {message}");
    ExitCode::from(crate::EXIT_REJECTED)
}

/// Uma conexão: imprime cada nível até o daemon fechar (ou, sem
/// `follow`, até a gravação terminar).
fn follow(mut stream: UnixStream, follow: bool) -> End {
    if stream
        .write_all(&Request::new("meter", Command::Meter).to_line())
        .is_err()
    {
        return End::Closed;
    }
    let Ok(lines) = crate::output::FollowLines::new(stream) else {
        return End::Closed;
    };
    let mut recorded = false;
    for line in lines {
        let Ok(line) = line else { return End::Closed };
        if let Ok(event) = serde_json::from_str::<MeterEvent>(&line) {
            print_level(event.level);
            if event.state == StateName::Recording {
                recorded = true;
            } else if recorded && !follow {
                return End::Recording;
            }
        } else if let Ok(response) = serde_json::from_str::<Response>(&line)
            && let Some(error) = response.error
        {
            return End::Rejected(error.message);
        }
    }
    End::Closed
}

/// `{"level":0.42}` com flush por linha (o leitor é outro processo). Pipe
/// fechado (a pílula saiu) encerra o comando e a conexão: ver
/// [`crate::output`].
fn print_level(level: f32) {
    crate::outln!("{{\"level\":{level}}}");
}
