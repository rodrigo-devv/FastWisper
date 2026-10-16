//! Thread dona do main loop do PipeWire. Os comandos chegam por um canal
//! do próprio PipeWire; o callback de áudio roda nessa thread, só copia
//! para um buffer pré-alocado e guarda o RMS do buffer num atômico (sem
//! alocação, I/O, locks ou inferência).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::properties::properties;
use pw::spa;
use pw::stream::{StreamFlags, StreamListener, StreamRc, StreamState};
use pw::types::ObjectType;
use spa::pod::Pod;
use tracing::{debug, warn};

use super::{CaptureError, MicLevel, Recording, SOURCE_CLASSES, SourceInfo, select_source};
use crate::audio::SAMPLE_RATE;
use crate::cues;

const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

enum Cmd {
    Sources(mpsc::Sender<Vec<SourceInfo>>),
    DefaultSource(mpsc::Sender<Option<String>>),
    Start {
        tag: u64,
        query: String,
        max_samples: usize,
        reply: mpsc::Sender<Result<SourceInfo, CaptureError>>,
    },
    Stop {
        tag: u64,
        reply: mpsc::Sender<Result<Recording, CaptureError>>,
    },
    Abort {
        tag: u64,
    },
    /// Som curto (F32 mono a `cues::RATE`) na saída padrão.
    Play(Arc<[f32]>),
    Quit,
}

/// Handle da thread de captura. Uma gravação por vez, identificada por
/// `tag` (a generation da sessão): stop/abort de outra sessão são ignorados.
pub struct Capture {
    tx: pw::channel::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    level: Arc<MicLevel>,
}

impl Capture {
    /// Conecta ao PipeWire e espera a lista inicial de dispositivos.
    pub fn spawn() -> Result<Capture, CaptureError> {
        let (tx, rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let level = Arc::new(MicLevel::default());
        let thread_level = Arc::clone(&level);
        let thread = thread::Builder::new()
            .name("fw-capture".into())
            .spawn(move || {
                if let Err(e) = run(rx, &ready_tx, thread_level) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::PipeWire(e.to_string()))?;
        let capture = Capture {
            tx,
            thread: Some(thread),
            level,
        };
        ready_rx
            .recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| CaptureError::PipeWire("sem resposta do PipeWire".into()))??;
        Ok(capture)
    }

    pub fn sources(&self) -> Result<Vec<SourceInfo>, CaptureError> {
        self.request(Cmd::Sources)
    }

    /// Nó do microfone padrão do sistema (metadado `default.audio.source`
    /// do WirePlumber), se houver.
    pub fn default_source(&self) -> Result<Option<String>, CaptureError> {
        self.request(Cmd::DefaultSource)
    }

    pub fn start(
        &self,
        tag: u64,
        query: &str,
        max_samples: usize,
    ) -> Result<SourceInfo, CaptureError> {
        self.request(|reply| Cmd::Start {
            tag,
            query: query.to_owned(),
            max_samples,
            reply,
        })?
    }

    /// Nível do microfone na gravação em andamento (0..1, escala
    /// perceptual); 0 sem gravação. Só lê um atômico.
    pub fn level(&self) -> f32 {
        self.level.level()
    }

    pub fn stop(&self, tag: u64) -> Result<Recording, CaptureError> {
        self.request(|reply| Cmd::Stop { tag, reply })?
    }

    /// Toca um som curto na saída padrão (substitui o anterior). Não
    /// espera resposta; falha só vai para o log.
    pub fn play(&self, samples: Arc<[f32]>) {
        let _ = self.tx.send(Cmd::Play(samples));
    }

    /// Fecha o microfone e descarta o áudio. Não espera resposta.
    pub fn abort(&self, tag: u64) {
        let _ = self.tx.send(Cmd::Abort { tag });
    }

    fn request<T>(&self, make: impl FnOnce(mpsc::Sender<T>) -> Cmd) -> Result<T, CaptureError> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| CaptureError::PipeWire("thread de captura encerrada".into()))?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| CaptureError::PipeWire("sem resposta da thread de captura".into()))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Estado da gravação, só acessado na thread do PipeWire.
struct Rec {
    samples: Vec<f32>,
    overflow: bool,
    lost: bool,
    error: Option<String>,
    streaming: bool,
    started: Instant,
    first_buffer: Option<Duration>,
    rate: u32,
    channels: u32,
    /// RMS do último buffer, para o stream `meter`.
    level: Arc<MicLevel>,
}

impl Rec {
    /// Callback de áudio: cópia limitada à capacidade reservada no start e
    /// o RMS do buffer no atômico do medidor.
    fn push(&mut self, bytes: &[u8]) {
        if self.first_buffer.is_none() {
            self.first_buffer = Some(self.started.elapsed());
        }
        let (frames, _) = bytes.as_chunks::<4>();
        let sum: f32 = frames
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .map(|x| x * x)
            .sum();
        self.level
            .set_rms((sum / frames.len().max(1) as f32).sqrt());
        let n = frames.len();
        if self.overflow || self.samples.len() + n > self.samples.capacity() {
            self.overflow = true;
            return;
        }
        self.samples
            .extend(frames.iter().map(|b| f32::from_le_bytes(*b)));
    }
}

struct Active {
    tag: u64,
    target_id: u32,
    stream: StreamRc,
    _listener: StreamListener<Rc<RefCell<Rec>>>,
    rec: Rc<RefCell<Rec>>,
}

impl Active {
    fn close(self) -> Rec {
        // Listener primeiro: o próprio disconnect emite `Unconnected`, que
        // não pode ser lido como microfone perdido.
        drop(self._listener);
        if let Err(e) = self.stream.disconnect() {
            debug!(error = %e, "falha ao desconectar o stream");
        }
        drop(self.stream);
        let mut rec = self.rec.borrow_mut();
        rec.level.set_rms(0.0);
        Rec {
            samples: std::mem::take(&mut rec.samples),
            error: rec.error.take(),
            level: Arc::clone(&rec.level),
            ..*rec
        }
    }
}

fn pw_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::PipeWire(e.to_string())
}

fn run(
    rx: pw::channel::Receiver<Cmd>,
    ready: &mpsc::Sender<Result<(), CaptureError>>,
    level: Arc<MicLevel>,
) -> Result<(), CaptureError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(pw_err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(pw_err)?;
    let core = context.connect_rc(None).map_err(pw_err)?;
    let registry = core.get_registry_rc().map_err(pw_err)?;

    let sources: Rc<RefCell<BTreeMap<u32, SourceInfo>>> = Rc::default();
    let active: Rc<RefCell<Option<Active>>> = Rc::default();
    let playback: Rc<RefCell<Option<Playback>>> = Rc::default();

    let default_source: Rc<RefCell<Option<String>>> = Rc::default();
    let metadata: Rc<RefCell<Vec<(Metadata, MetadataListener)>>> = Rc::default();

    let _registry_listener = {
        let registry_for_bind = registry.clone();
        let default_for_bind = Rc::clone(&default_source);
        let metadata_for_bind = Rc::clone(&metadata);
        let added = Rc::clone(&sources);
        let removed = Rc::clone(&sources);
        let active = Rc::clone(&active);
        registry
            .add_listener_local()
            .global(move |global| {
                if global.type_ == ObjectType::Metadata {
                    let is_default = global
                        .props
                        .is_some_and(|p| p.get("metadata.name") == Some("default"));
                    if is_default && let Ok(proxy) = registry_for_bind.bind::<Metadata, _>(global) {
                        let target = Rc::clone(&default_for_bind);
                        let listener = proxy
                            .add_listener_local()
                            .property(move |_subject, key, _type, value| {
                                if key == Some("default.audio.source") {
                                    *target.borrow_mut() = value.and_then(default_node_name);
                                }
                                0
                            })
                            .register();
                        metadata_for_bind.borrow_mut().push((proxy, listener));
                    }
                    return;
                }
                if global.type_ != ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                let class = props.get("media.class").unwrap_or_default();
                if !SOURCE_CLASSES.contains(&class) {
                    return;
                }
                let node_name = props.get("node.name").unwrap_or_default().to_owned();
                let info = SourceInfo {
                    id: global.id,
                    description: props
                        .get("node.description")
                        .map_or_else(|| node_name.clone(), str::to_owned),
                    node_name,
                    media_class: class.to_owned(),
                };
                added.borrow_mut().insert(global.id, info);
            })
            .global_remove(move |id| {
                removed.borrow_mut().remove(&id);
                if let Some(a) = active.borrow().as_ref()
                    && a.target_id == id
                {
                    a.rec.borrow_mut().lost = true;
                }
            })
            .register()
    };

    // Segundo roundtrip: as propriedades do metadado chegam depois do bind.
    roundtrip(&mainloop, &core)?;
    roundtrip(&mainloop, &core)?;
    let _ = ready.send(Ok(()));

    let _receiver = rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |cmd| match cmd {
            Cmd::Sources(reply) => {
                let _ = reply.send(sources.borrow().values().cloned().collect());
            }
            Cmd::DefaultSource(reply) => {
                let _ = reply.send(default_source.borrow().clone());
            }
            Cmd::Start {
                tag,
                query,
                max_samples,
                reply,
            } => {
                // Gravação esquecida (sem stop) não pode vazar para a nova.
                if let Some(old) = active.borrow_mut().take() {
                    warn!(tag = old.tag, "descartando gravação anterior sem stop");
                    old.close();
                }
                let result = start(
                    &core,
                    &sources.borrow(),
                    tag,
                    &query,
                    max_samples,
                    Arc::clone(&level),
                )
                .map(|a| {
                    let info = sources.borrow().get(&a.target_id).cloned();
                    *active.borrow_mut() = Some(a);
                    info
                });
                let _ = reply.send(result.and_then(|info| {
                    info.ok_or_else(|| {
                        CaptureError::PipeWire("dispositivo sumiu ao iniciar".into())
                    })
                }));
            }
            Cmd::Stop { tag, reply } => {
                let current = active.borrow().as_ref().map(|a| a.tag);
                let result = if current == Some(tag) {
                    let rec = active.borrow_mut().take().map(Active::close);
                    rec.map_or(Err(CaptureError::NotRecording), finish)
                } else {
                    Err(CaptureError::NotRecording)
                };
                let _ = reply.send(result);
            }
            Cmd::Abort { tag } => {
                let current = active.borrow().as_ref().map(|a| a.tag);
                if current == Some(tag)
                    && let Some(a) = active.borrow_mut().take()
                {
                    a.close();
                }
            }
            Cmd::Play(samples) => {
                // O som anterior (já pausado ou não) sai antes do novo.
                drop(playback.borrow_mut().take());
                match play(&core, samples) {
                    Ok(p) => *playback.borrow_mut() = Some(p),
                    Err(e) => debug!(error = %e, "som não tocou"),
                }
            }
            Cmd::Quit => {
                if let Some(a) = active.borrow_mut().take() {
                    a.close();
                }
                drop(playback.borrow_mut().take());
                mainloop.quit();
            }
        }
    });

    mainloop.run();
    Ok(())
}

/// `{"name":"alsa_input...."}` → nome do nó.
fn default_node_name(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

fn finish(rec: Rec) -> Result<Recording, CaptureError> {
    if rec.overflow {
        return Err(CaptureError::TooLong);
    }
    if rec.lost || (rec.streaming && rec.error.is_some()) {
        return Err(CaptureError::Lost);
    }
    if let Some(error) = rec.error {
        // Erro antes de começar: o alvo sumiu entre a seleção e o connect.
        return Err(CaptureError::PipeWire(error));
    }
    if rec.rate != SAMPLE_RATE || rec.channels != 1 {
        return Err(CaptureError::PipeWire(format!(
            "formato negociado inesperado: {} Hz, {} canais",
            rec.rate, rec.channels
        )));
    }
    Ok(Recording {
        samples: rec.samples,
        rate: rec.rate,
        channels: rec.channels,
        first_buffer: rec.first_buffer,
    })
}

fn start(
    core: &pw::core::CoreRc,
    sources: &BTreeMap<u32, SourceInfo>,
    tag: u64,
    query: &str,
    max_samples: usize,
    level: Arc<MicLevel>,
) -> Result<Active, CaptureError> {
    let list: Vec<SourceInfo> = sources.values().cloned().collect();
    let target = select_source(&list, query)?.clone();

    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => "fastwisper",
        *pw::keys::NODE_DESCRIPTION => "FastWisper",
        "target.object" => target.node_name.clone(),
        // Sem fallback: se o alvo sumir, o stream falha em vez de mudar de microfone.
        "node.dont-fallback" => "true",
        "node.dont-reconnect" => "true",
        "node.dont-move" => "true",
    };
    let stream = StreamRc::new(core.clone(), "fastwisper-capture", props).map_err(pw_err)?;

    let rec = Rc::new(RefCell::new(Rec {
        samples: Vec::with_capacity(max_samples),
        overflow: false,
        lost: false,
        error: None,
        streaming: false,
        started: Instant::now(),
        first_buffer: None,
        rate: 0,
        channels: 0,
        level,
    }));

    let listener = stream
        .add_local_listener_with_user_data(Rc::clone(&rec))
        .state_changed(|_, rec, _old, new| {
            let mut rec = rec.borrow_mut();
            // Sem buffers chegando, o medidor não fica preso no último nível.
            if !matches!(new, StreamState::Streaming) {
                rec.level.set_rms(0.0);
            }
            match new {
                StreamState::Streaming => rec.streaming = true,
                StreamState::Error(message) => rec.error = Some(message),
                StreamState::Unconnected if rec.streaming => rec.lost = true,
                _ => {}
            }
        })
        .param_changed(|_, rec, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                let mut rec = rec.borrow_mut();
                rec.rate = info.rate();
                rec.channels = info.channels();
            }
        })
        .process(|stream, rec| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            if let Some(bytes) = data.data().and_then(|b| b.get(offset..offset + size)) {
                rec.borrow_mut().push(bytes);
            }
        })
        .register()
        .map_err(pw_err)?;

    let format = format_pod(SAMPLE_RATE, false)?;
    let mut params = [Pod::from_bytes(&format).ok_or_else(|| pw_err("pod de formato inválido"))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(pw_err)?;

    Ok(Active {
        tag,
        target_id: target.id,
        stream,
        _listener: listener,
        rec,
    })
}

/// Som em reprodução. O stream pausa sozinho no fim; sai quando outro som
/// começa ou a thread encerra.
struct Playback {
    _stream: StreamRc,
    _listener: StreamListener<Tone>,
}

/// Depois da última amostra, quanto o stream ainda roda (escrevendo
/// silêncio) antes de pausar: pausar logo descarta os buffers na fila.
const DRAIN: Duration = Duration::from_millis(300);

struct Tone {
    samples: Arc<[f32]>,
    pos: usize,
    /// Quando a última amostra foi escrita.
    finished: Option<Instant>,
}

fn play(core: &pw::core::CoreRc, samples: Arc<[f32]>) -> Result<Playback, CaptureError> {
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Notification",
        *pw::keys::NODE_NAME => "fastwisper-sons",
        *pw::keys::NODE_DESCRIPTION => "FastWisper (sons)",
    };
    let stream = StreamRc::new(core.clone(), "fastwisper-sons", props).map_err(pw_err)?;
    let tone = Tone {
        samples,
        pos: 0,
        finished: None,
    };
    let listener = stream
        .add_local_listener_with_user_data(tone)
        .process(|stream, tone| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let requested = buffer.requested() as usize;
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let mut written = 0;
            if let Some(bytes) = data.data() {
                let frames = bytes.as_chunks_mut::<4>().0;
                let n = match requested {
                    0 => frames.len(),
                    r => r.min(frames.len()),
                };
                for frame in &mut frames[..n] {
                    let sample = tone.samples.get(tone.pos).copied().unwrap_or(0.0);
                    *frame = sample.to_le_bytes();
                    tone.pos += 1;
                    written += 1;
                }
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = 4;
            *chunk.size_mut() = (written * 4) as u32;
            drop(buffer);
            // Pausa só depois que a fila tocou: libera o dispositivo sem
            // cortar o som.
            if tone.pos >= tone.samples.len() {
                let finished = *tone.finished.get_or_insert_with(Instant::now);
                if finished.elapsed() >= DRAIN {
                    let _ = stream.set_active(false);
                }
            }
        })
        .register()
        .map_err(pw_err)?;
    let format = format_pod(cues::RATE, true)?;
    let mut params = [Pod::from_bytes(&format).ok_or_else(|| pw_err("pod de formato inválido"))?];
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(pw_err)?;
    Ok(Playback {
        _stream: stream,
        _listener: listener,
    })
}

/// Pede F32 mono em `rate`: o PipeWire converte de/para o formato do
/// dispositivo. `mono_position`: canal marcado como MONO, para a saída
/// tocar nos dois lados (sem posição, só o canal esquerdo é ligado).
fn format_pod(rate: u32, mono_position: bool) -> Result<Vec<u8>, CaptureError> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(1);
    if mono_position {
        let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
        info.set_position(position);
    }
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map_err(|e| pw_err(format!("{e:?}")))?
    .0
    .into_inner())
}

fn roundtrip(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::CoreRc,
) -> Result<(), CaptureError> {
    let done = Rc::new(RefCell::new(false));
    let pending = core.sync(0).map_err(pw_err)?;
    let _listener = core
        .add_listener_local()
        .done({
            let done = Rc::clone(&done);
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    *done.borrow_mut() = true;
                    mainloop.quit();
                }
            }
        })
        .register();
    while !*done.borrow() {
        mainloop.run();
    }
    Ok(())
}
