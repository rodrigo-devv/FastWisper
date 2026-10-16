//! Sondagem da GPU num processo filho, antes de abrir o Vulkan no processo
//! principal. Sem driver, o ASR já fica na CPU sem abrir o Vulkan
//! ([`vulkan_driver_present`](crate::asr::vulkan_driver_present)); com um
//! driver instalado mas quebrado (manifesto de ICD presente), abrir o Vulkan
//! pode travar ou abortar o processo (exceção C++ que o Rust não captura,
//! `abort` dentro do driver). Por isso o binário abre o Vulkan primeiro num
//! filho (`<binário> --probe-gpu`) com prazo: só se o filho listar as GPUs e
//! sair com sucesso o processo principal usa a GPU. Se o filho travar, sair
//! com falha ou morrer por sinal, o motivo vira só um código (`timeout`,
//! `exit 1`, `signal 6`...) e o ASR fica na CPU.
//!
//! CPU no mesmo processo não basta: o whisper.cpp registra o backend Vulkan
//! (e abre o driver) ao carregar qualquer modelo, mesmo sem GPU. Então
//! [`prepare`] reexecuta o processo (mesmo PID e argumentos) com o loader do
//! Vulkan sem driver nenhum ([`OFF_ENV`] guarda o motivo). O mesmo vale para
//! `asr.gpu = "cpu"`, que não sonda: CPU de verdade, sem abrir o driver.
//!
//! O resultado fica guardado no processo, por valor de `asr.gpu`: uma nova
//! sondagem só quando o processo reinicia ou `asr.gpu` muda (no daemon,
//! `asr.gpu` exige reinício).

use std::fmt;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Argumento (primeiro, antes do clap) que faz `fastwisperd` e `fastwisper`
/// rodarem só a sondagem e imprimirem o resultado.
pub const CHILD_ARG: &str = "--probe-gpu";
/// Prazo do filho. Abrir o Vulkan e o dispositivo leva ~0,1–0,5 s com o
/// cache do driver quente; o prazo cobre cache frio, disco lento e a
/// máquina ocupada (jogo, compilação) sem deixar um driver travado segurar
/// o primeiro ditado por muito tempo (o ditado espera a carga até 60 s).
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Prazo em milissegundos no lugar de [`TIMEOUT`] (testes, máquinas lentas).
pub const TIMEOUT_ENV: &str = "FASTWISPER_GPU_PROBE_TIMEOUT_MS";
/// No processo reexecutado sem Vulkan: `cpu` ou o motivo da falha.
pub const OFF_ENV: &str = "FASTWISPER_VULKAN_OFF";
/// Notificação do daemon quando a sondagem falha (sem conteúdo).
pub const NOTICE: &str =
    "Driver Vulkan com falha: transcrição na CPU (mais lenta). Veja `fastwisper doctor`.";
/// Intervalo entre as consultas ao filho enquanto ele não termina.
const POLL: Duration = Duration::from_millis(10);
/// Quanto esperar o filho morto no prazo terminar antes de recolhê-lo numa
/// thread à parte (preso no kernel, ele pode demorar a sair).
const REAP: Duration = Duration::from_secs(1);

/// Por que a sondagem não provou o Vulkan utilizável.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// O filho passou do prazo (driver travado) e foi morto.
    Timeout,
    /// O filho saiu com este código de falha.
    Exit(i32),
    /// O filho morreu por este sinal (ex.: 6, `abort` no driver).
    Signal(i32),
    /// O filho saiu bem, mas sem o relatório esperado no stdout.
    Output,
    /// Não deu para iniciar o filho.
    Spawn,
}

impl Failure {
    /// Código estável para log e JSON, sem o número.
    pub fn code(&self) -> &'static str {
        match self {
            Failure::Timeout => "timeout",
            Failure::Exit(_) => "exit",
            Failure::Signal(_) => "signal",
            Failure::Output => "output",
            Failure::Spawn => "spawn",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Exit(code) | Failure::Signal(code) => write!(f, "{} {code}", self.code()),
            other => f.write_str(other.code()),
        }
    }
}

impl std::str::FromStr for Failure {
    type Err = ();

    /// O inverso de `Display` (o motivo passa pelo [`OFF_ENV`]).
    fn from_str(text: &str) -> Result<Self, ()> {
        let number = |rest: &str| rest.trim().parse::<i32>().map_err(|_| ());
        match text.trim() {
            "timeout" => Ok(Failure::Timeout),
            "output" => Ok(Failure::Output),
            "spawn" => Ok(Failure::Spawn),
            other => match other.split_once(' ') {
                Some(("exit", code)) => number(code).map(Failure::Exit),
                Some(("signal", signal)) => number(signal).map(Failure::Signal),
                _ => Err(()),
            },
        }
    }
}

/// Resultado da sondagem para um valor de `asr.gpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// `asr.gpu = "cpu"` (ou vazio): o Vulkan não é aberto.
    Skipped,
    /// Build sem Vulkan ou nenhum driver (manifesto de ICD): CPU, sem filho.
    NoVulkan,
    /// O Vulkan abriu e listou estas GPUs (pode ser nenhuma).
    Usable(Vec<String>),
    /// O driver travou, falhou ou derrubou o filho: CPU.
    Broken(Failure),
}

impl Probe {
    /// GPUs utilizáveis, na ordem do ggml (vazio fora de [`Probe::Usable`]).
    pub fn gpus(&self) -> &[String] {
        match self {
            Probe::Usable(gpus) => gpus,
            _ => &[],
        }
    }

    pub fn failure(&self) -> Option<&Failure> {
        match self {
            Probe::Broken(failure) => Some(failure),
            _ => None,
        }
    }
}

/// Relatório que o filho imprime no stdout.
#[derive(Debug, Serialize, Deserialize)]
struct Report {
    gpus: Vec<String>,
}

/// Linha JSON do filho com as GPUs vistas.
pub fn child_report(gpus: Vec<String>) -> String {
    serde_json::to_string(&Report { gpus }).unwrap_or_else(|_| "{\"gpus\":[]}".to_owned())
}

/// Prazo efetivo: [`TIMEOUT_ENV`] em ms, se válido e positivo.
pub fn timeout() -> Duration {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .map_or(TIMEOUT, Duration::from_millis)
}

/// Roda o filho e lê as GPUs do relatório. Stdin e stderr ficam fechados
/// (logs do driver não vão para o journal do daemon); passou do prazo, o
/// filho é morto e recolhido (numa thread à parte se não sair em [`REAP`],
/// para um processo preso no kernel não segurar quem sondou).
pub fn run(mut command: Command, timeout: Duration) -> Result<Vec<String>, Failure> {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| Failure::Spawn)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            result => {
                let _ = child.kill();
                // Recolhido aqui no caso comum (o SIGKILL termina na hora):
                // o daemon pode se reexecutar logo depois, e um filho não
                // recolhido antes do `exec` ficaria zumbi até ele sair.
                let reaped = Instant::now() + REAP;
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < reaped {
                    std::thread::sleep(POLL);
                }
                if matches!(child.try_wait(), Ok(None)) {
                    std::thread::spawn(move || child.wait());
                }
                return Err(if result.is_ok() {
                    Failure::Timeout
                } else {
                    Failure::Spawn
                });
            }
        }
    };
    if let Some(signal) = status.signal() {
        return Err(Failure::Signal(signal));
    }
    match status.code() {
        Some(0) => {}
        Some(code) => return Err(Failure::Exit(code)),
        None => return Err(Failure::Output),
    }
    let mut out = String::new();
    child
        .stdout
        .take()
        .ok_or(Failure::Output)?
        .read_to_string(&mut out)
        .map_err(|_| Failure::Output)?;
    out.lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Report>(line).ok())
        .map(|report| report.gpus)
        .ok_or(Failure::Output)
}

/// Sondagem feita agora ou guardada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ensured {
    pub probe: Probe,
    /// `true` se esta chamada sondou (um filho rodou ou o driver foi
    /// procurado); `false` se o resultado veio do cache.
    pub fresh: bool,
}

/// Resultado guardado por valor de `asr.gpu`, com o portão do Vulkan no
/// processo: fechado depois de uma sondagem que falhou, até uma que passe.
#[derive(Debug, Default)]
pub struct ProbeCache {
    entry: Mutex<Option<(String, Probe)>>,
    blocked: AtomicBool,
    probes: AtomicUsize,
}

impl ProbeCache {
    pub const fn new() -> Self {
        Self {
            entry: Mutex::new(None),
            blocked: AtomicBool::new(false),
            probes: AtomicUsize::new(0),
        }
    }

    /// Resultado para `gpu` (`asr.gpu`): o guardado se `gpu` não mudou;
    /// senão `probe` roda agora. Quem chega durante uma sondagem espera por
    /// ela (o lock fica com quem sonda). `cpu`/vazio não sonda.
    pub fn ensure(&self, gpu: &str, probe: impl FnOnce() -> Probe) -> Ensured {
        let key = gpu.trim();
        let mut entry = self.entry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached, result)) = entry.as_ref()
            && cached == key
        {
            return Ensured {
                probe: result.clone(),
                fresh: false,
            };
        }
        let result = if key.is_empty() || key.eq_ignore_ascii_case("cpu") {
            Probe::Skipped
        } else {
            self.probes.fetch_add(1, Ordering::Relaxed);
            let result = probe();
            self.blocked
                .store(matches!(result, Probe::Broken(_)), Ordering::Release);
            result
        };
        *entry = Some((key.to_owned(), result.clone()));
        Ensured {
            probe: result,
            fresh: true,
        }
    }

    /// A última sondagem falhou: o processo não deve abrir o Vulkan.
    pub fn blocked(&self) -> bool {
        self.blocked.load(Ordering::Acquire)
    }

    /// Quantas sondagens rodaram (sem contar `cpu`).
    pub fn probes(&self) -> usize {
        self.probes.load(Ordering::Relaxed)
    }
}

static CACHE: ProbeCache = ProbeCache::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Liga a sondagem no filho para este processo. Só os binários chamam (eles
/// tratam [`CHILD_ARG`]); sem isto (testes, bibliotecas), [`ensure`] abre o
/// Vulkan no próprio processo, como antes, e [`prepare`] não reexecuta.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// O Vulkan não deve ser aberto neste processo (a sondagem falhou).
pub fn vulkan_blocked() -> bool {
    CACHE.blocked()
}

/// Sondagem para `asr.gpu = gpu`, guardada no processo. Não abre o Vulkan
/// aqui (só no filho) e não reexecuta: basta para listar GPUs (`doctor`,
/// `gpus`, `setup`). Antes de carregar o Whisper, use [`prepare`].
#[cfg(feature = "whisper")]
pub fn ensure(gpu: &str) -> Ensured {
    CACHE.ensure(gpu, probe_now)
}

/// Antes de carregar o Whisper neste processo: sonda (se `asr.gpu` não for
/// `cpu`) e, se o Vulkan não deve ser aberto (driver quebrado ou `cpu`),
/// reexecuta o processo com o loader do Vulkan sem drivers. Só volta depois
/// disso, ou se o Vulkan pode ser usado, ou se o `exec` falhou (aí o erro vai
/// para o log e o processo segue; a carga continua na CPU pelo portão).
#[cfg(feature = "whisper")]
pub fn prepare(gpu: &str) -> Ensured {
    let ensured = ensure(gpu);
    let off = match &ensured.probe {
        Probe::Skipped => Some("cpu".to_owned()),
        Probe::Broken(failure) => Some(failure.to_string()),
        Probe::NoVulkan | Probe::Usable(_) => None,
    };
    if let Some(off) = off
        && crate::asr::whisper::VULKAN
        && ENABLED.load(Ordering::Acquire)
        && std::env::var_os(OFF_ENV).is_none()
    {
        let error = reexec_without_vulkan(&off);
        tracing::error!("gpu: falha ao reiniciar sem Vulkan: {error}");
    }
    ensured
}

/// Troca este processo por ele mesmo (mesmo PID, argumentos e ambiente),
/// com o loader do Vulkan sem driver nenhum: o whisper.cpp vê o Vulkan
/// indisponível e fica na CPU. Só volta com o erro do `exec`.
#[cfg(feature = "whisper")]
fn reexec_without_vulkan(off: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let mut command = Command::new(self_exe());
    if let Some(arg0) = args.next() {
        command.arg0(arg0);
    }
    command
        .args(args)
        .env(OFF_ENV, off)
        .env("VK_DRIVER_FILES", "/dev/null")
        .env("VK_ICD_FILENAMES", "/dev/null")
        .env_remove("VK_ADD_DRIVER_FILES")
        .exec()
}

/// O binário em execução: pelo caminho (nome do processo intacto) ou, se ele
/// foi trocado no disco (atualização com o daemon rodando), pelo `/proc`.
#[cfg(feature = "whisper")]
fn self_exe() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .filter(|path| path.is_file())
        .unwrap_or_else(|| "/proc/self/exe".into())
}

/// Detalhe humano de uma sondagem que falhou (doctor, CLI).
pub fn broken_detail(failure: &Failure) -> String {
    format!("driver Vulkan falhou na sondagem ({failure}): transcrição na CPU")
}

#[cfg(feature = "whisper")]
fn probe_now() -> Probe {
    // Processo reexecutado por [`prepare`]: o motivo veio junto, sem sondar.
    if let Some(off) = std::env::var_os(OFF_ENV) {
        return off
            .to_str()
            .and_then(|off| off.parse().ok())
            .map_or(Probe::NoVulkan, Probe::Broken);
    }
    if !crate::asr::whisper::VULKAN || !crate::asr::vulkan_driver_present(|n| std::env::var_os(n)) {
        return Probe::NoVulkan;
    }
    if !ENABLED.load(Ordering::Acquire) {
        return Probe::Usable(crate::asr::whisper::probe_gpu_names());
    }
    let mut command = Command::new(self_exe());
    command.arg(CHILD_ARG);
    let started = Instant::now();
    let result = run(command, timeout());
    tracing::info!(
        probe_ms = started.elapsed().as_millis() as u64,
        ok = result.is_ok(),
        "gpu: sondagem do Vulkan"
    );
    match result {
        Ok(gpus) => Probe::Usable(gpus),
        Err(failure) => Probe::Broken(failure),
    }
}

/// O que o filho faz: abre o Vulkan, lista as GPUs e imprime o relatório.
/// Se o driver travar ou abortar, é aqui, longe do processo principal.
#[cfg(feature = "whisper")]
pub fn child_main() -> std::process::ExitCode {
    use std::io::Write;
    // Um driver que aborta não deixa core dump do filho: o motivo já vai
    // para o log e a notificação do daemon.
    let _ = rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable);
    // Quem sondou morreu (SIGTERM durante a sondagem, CLI morta pelo hub):
    // o filho preso no driver morre junto, em vez de ficar órfão.
    let _ = rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL));
    let report = child_report(crate::asr::whisper::probe_gpu_names());
    let mut stdout = std::io::stdout().lock();
    match writeln!(stdout, "{report}").and_then(|()| stdout.flush()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_good_child_reports_the_gpus() {
        let report = child_report(vec!["Example GPU".into(), "Other \"GPU\"".into()]);
        let script = format!("echo 'ruído do driver'; printf '%s\\n' '{report}'");
        assert_eq!(
            run(sh(&script), Duration::from_secs(10)),
            Ok(vec!["Example GPU".into(), "Other \"GPU\"".into()])
        );
        assert_eq!(
            run(sh("echo '{\"gpus\":[]}'"), Duration::from_secs(10)),
            Ok(vec![])
        );
    }

    /// Driver que trava: o filho é morto no prazo, sem esperar o `sleep`.
    #[test]
    fn a_hung_child_times_out() {
        let started = Instant::now();
        assert_eq!(
            run(sh("exec sleep 30"), Duration::from_millis(300)),
            Err(Failure::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Filho morto por sinal, que falha ou não imprime o relatório. Sinais
    /// sem core dump: o abort de um driver de verdade fica no teste com o
    /// ICD falso (`fastwisperd/tests/gpu_probe.rs`).
    #[test]
    fn a_crashing_or_failing_child_is_a_failure() {
        let timeout = Duration::from_secs(10);
        assert_eq!(run(sh("kill -TERM $$"), timeout), Err(Failure::Signal(15)));
        assert_eq!(run(sh("kill -KILL $$"), timeout), Err(Failure::Signal(9)));
        assert_eq!(run(sh("exit 3"), timeout), Err(Failure::Exit(3)));
        assert_eq!(run(sh("echo pronto"), timeout), Err(Failure::Output));
        assert_eq!(
            run(Command::new("/nonexistent/fastwisperd"), timeout),
            Err(Failure::Spawn)
        );
    }

    #[test]
    fn failure_codes_are_stable_and_carry_no_text() {
        assert_eq!(Failure::Timeout.to_string(), "timeout");
        assert_eq!(Failure::Signal(6).to_string(), "signal 6");
        assert_eq!(Failure::Exit(1).to_string(), "exit 1");
        assert_eq!(Failure::Output.to_string(), "output");
        assert_eq!(Failure::Spawn.code(), "spawn");
        // Ida e volta pelo ambiente do processo reexecutado.
        for failure in [
            Failure::Timeout,
            Failure::Exit(1),
            Failure::Signal(6),
            Failure::Output,
            Failure::Spawn,
        ] {
            assert_eq!(failure.to_string().parse(), Ok(failure));
        }
        for text in ["cpu", "", "signal", "exit x", "signal 6 7"] {
            assert_eq!(text.parse::<Failure>(), Err(()), "{text}");
        }
    }

    /// Uma sondagem por valor de `asr.gpu`: o segundo ditado usa a guardada,
    /// trocar `asr.gpu` sonda de novo e `cpu` não sonda.
    #[test]
    fn the_result_is_cached_per_gpu_setting() {
        let cache = ProbeCache::new();
        let broken = || Probe::Broken(Failure::Signal(6));
        let first = cache.ensure("auto", broken);
        assert_eq!(first.probe, Probe::Broken(Failure::Signal(6)));
        assert!(first.fresh);
        assert!(cache.blocked());
        let second = cache.ensure(" auto ", || unreachable!("não sonda de novo"));
        assert_eq!((second.probe, second.fresh), (first.probe.clone(), false));
        assert_eq!(cache.probes(), 1);

        // `cpu` não sonda e não reabre o portão.
        let cpu = cache.ensure("cpu", || unreachable!("cpu não sonda"));
        assert_eq!(cpu.probe, Probe::Skipped);
        assert_eq!(cache.probes(), 1);
        assert!(cache.blocked());

        // Outro valor sonda de novo; uma sondagem boa reabre o portão.
        let good = cache.ensure("Example GPU", || Probe::Usable(vec!["Example GPU".into()]));
        assert!(good.fresh);
        assert_eq!(good.probe.gpus(), ["Example GPU".to_owned()]);
        assert!(!cache.blocked());
        assert_eq!(cache.probes(), 2);

        // Voltar para `auto` também conta como mudança.
        assert!(cache.ensure("auto", broken).fresh);
        assert_eq!(cache.probes(), 3);
        assert!(cache.blocked());
    }

    #[test]
    fn timeout_comes_from_the_environment_only_when_valid() {
        // Sem a variável no ambiente dos testes, o padrão.
        if std::env::var_os(TIMEOUT_ENV).is_none() {
            assert_eq!(timeout(), TIMEOUT);
        }
    }
}
