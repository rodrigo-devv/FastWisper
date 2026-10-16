//! Saída da CLI. Toda escrita no terminal passa por aqui ([`out!`],
//! [`outln!`], [`eout!`], [`eoutln!`]); `println!` e companhia são
//! proibidos no crate (lint em `main.rs`) porque entram em pânico quando
//! quem lê fecha o pipe (`fastwisper hotkeys keys | head -1`).
//!
//! Pipe fechado na saída padrão é fim normal: a escrita desenrola a pilha
//! até o `main` com [`std::panic::resume_unwind`] (sem gancho de pânico,
//! logo sem mensagem nem stack trace). No caminho, os `Drop` rodam: conexão
//! IPC fechada, temporários apagados. O `main` sai com 0. Outro erro de
//! escrita (disco cheio, E/S) sai com erro e mensagem curta.
//!
//! Um `--follow` parado (daemon fora do ar, nada mudou) não escreve e não
//! veria o pipe fechado: [`FollowLines`] e [`wait`] conferem a cada
//! [`READER_CHECK`] se quem lê ainda está lá e encerram do mesmo jeito.
//!
//! Escrever no stderr nunca interrompe o comando: se ele falhar, não há
//! onde avisar.

use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};

/// De quanto em quanto tempo um `--follow` parado confere a saída.
pub const READER_CHECK: Duration = Duration::from_millis(500);

/// Carga do desenrolar: a saída padrão deixou de aceitar escrita.
struct Stop(io::Error);

/// Escreve na saída padrão e esvazia o buffer: quem lê costuma ser outro
/// processo (barra, pílula, `head`), e o pipe fechado aparece já nesta
/// linha, não no fim do comando.
pub fn stdout(args: fmt::Arguments<'_>) {
    #[allow(clippy::disallowed_methods)]
    let mut out = io::stdout().lock();
    let written = out.write_fmt(args).and_then(|()| out.flush());
    drop(out);
    if let Err(error) = written {
        panic::resume_unwind(Box::new(Stop(error)));
    }
}

/// Encerra como uma escrita em pipe fechado se quem lê a saída padrão
/// saiu. Sem escrever nada: `poll` vê `POLLERR` no pipe sem leitor (ou
/// `POLLHUP` num socket); terminal e arquivo nunca acusam.
fn stop_if_reader_gone() {
    #[allow(clippy::disallowed_methods)]
    let out = io::stdout();
    let mut fds = [PollFd::new(&out, PollFlags::empty())];
    let gone = poll(&mut fds, Some(&rustix::event::Timespec::default())).is_ok()
        && fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP);
    if gone {
        panic::resume_unwind(Box::new(Stop(io::ErrorKind::BrokenPipe.into())));
    }
}

/// `thread::sleep` de um `--follow` (espera para reconectar): acorda a
/// cada [`READER_CHECK`] para ver se quem lê a saída ainda está lá.
pub fn wait(duration: Duration) {
    let deadline = Instant::now() + duration;
    loop {
        stop_if_reader_gone();
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(READER_CHECK));
    }
}

/// Linhas de uma conexão de `--follow` (como `BufRead::lines`). Enquanto
/// o daemon não manda nada, confere a cada [`READER_CHECK`] se quem lê a
/// saída ainda está lá; se não, encerra e a conexão fecha no desenrolar.
pub struct FollowLines {
    reader: BufReader<UnixStream>,
    line: Vec<u8>,
}

impl FollowLines {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_read_timeout(Some(READER_CHECK))?;
        Ok(Self {
            reader: BufReader::new(stream),
            line: Vec::new(),
        })
    }
}

impl Iterator for FollowLines {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // `read_until` guarda os bytes já lidos mesmo com erro: a linha
            // interrompida pelo prazo continua na próxima volta.
            match self.reader.read_until(b'\n', &mut self.line) {
                Ok(0) if self.line.is_empty() => return None,
                Ok(_) => break,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    stop_if_reader_gone();
                }
                Err(e) => return Some(Err(e)),
            }
        }
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        Some(String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)))
    }
}

/// Escreve no stderr, ignorando falhas.
pub fn stderr(args: fmt::Arguments<'_>) {
    #[allow(clippy::disallowed_methods)]
    let mut err = io::stderr().lock();
    let _ = err.write_fmt(args).and_then(|()| err.flush());
}

/// Roda o comando; pipe fechado na saída vira código 0. Pânico de verdade
/// segue o curso normal (mensagem do gancho e código 101).
pub fn run(command: impl FnOnce() -> ExitCode) -> ExitCode {
    match panic::catch_unwind(AssertUnwindSafe(command)) {
        Ok(code) => code,
        Err(payload) => match payload.downcast::<Stop>() {
            Ok(stop) if stop.0.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Ok(stop) => {
                crate::human_error!("fastwisper: falha ao escrever a saída: {}", stop.0);
                ExitCode::from(crate::EXIT_REJECTED)
            }
            Err(payload) => panic::resume_unwind(payload),
        },
    }
}

/// Executa `body` e roda `cleanup` mesmo se a saída fechar no meio
/// (o desenrolar continua depois). Para o que o `Drop` não cobre, como
/// cancelar uma gravação no daemon.
pub fn finally<T, U>(body: impl FnOnce() -> T, cleanup: impl FnOnce() -> U) -> (T, U) {
    let result = panic::catch_unwind(AssertUnwindSafe(body));
    let cleaned = cleanup();
    match result {
        Ok(value) => (value, cleaned),
        Err(payload) => panic::resume_unwind(payload),
    }
}

/// `print!` que trata pipe fechado como fim normal.
macro_rules! out {
    ($($args:tt)*) => { $crate::output::stdout(format_args!($($args)*)) };
}
/// `println!` que trata pipe fechado como fim normal.
macro_rules! outln {
    () => { $crate::output::stdout(format_args!("\n")) };
    ($($args:tt)*) => { $crate::output::stdout(format_args!("{}\n", format_args!($($args)*))) };
}
/// `eprint!` que nunca entra em pânico.
macro_rules! eout {
    ($($args:tt)*) => { $crate::output::stderr(format_args!($($args)*)) };
}
/// `eprintln!` que nunca entra em pânico.
macro_rules! eoutln {
    () => { $crate::output::stderr(format_args!("\n")) };
    ($($args:tt)*) => { $crate::output::stderr(format_args!("{}\n", format_args!($($args)*))) };
}
pub(crate) use {eout, eoutln, out, outln};
