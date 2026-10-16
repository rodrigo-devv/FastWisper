//! Microfone virtual para testes: um nó `Audio/Source` do PipeWire
//! que "fala" as amostras dadas (mono 16 kHz) quando alguém o captura.
//! Sem autoconnect: nunca toca na saída de som do usuário. Dropar o
//! [`VirtualMic`] remove o nó, como desplugar o headset.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::pod::Pod;

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct VirtualMic {
    /// Descrição única (use como `audio.device`).
    pub description: String,
    tx: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl VirtualMic {
    /// `samples` tocam uma vez a partir do primeiro buffer; depois, silêncio.
    pub fn new(label: &str, samples: Vec<f32>) -> VirtualMic {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let description = format!("FW Test {label} {}-{n}", std::process::id());
        let node_name = format!("fw-test-{}-{n}", std::process::id());
        let (tx, rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let desc = description.clone();
        let thread = thread::spawn(move || run(&node_name, &desc, samples, rx, ready_tx));
        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("microfone virtual não subiu");
        // Dá tempo do registry dos outros clientes ver o nó.
        thread::sleep(Duration::from_millis(200));
        VirtualMic {
            description,
            tx,
            thread: Some(thread),
        }
    }
}

impl Drop for VirtualMic {
    fn drop(&mut self) {
        let _ = self.tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn run(
    node_name: &str,
    description: &str,
    samples: Vec<f32>,
    rx: pw::channel::Receiver<()>,
    ready: mpsc::Sender<()>,
) {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).expect("mainloop");
    let context = pw::context::ContextRc::new(&mainloop, None).expect("context");
    let core = context.connect_rc(None).expect("core");

    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        // `Audio/Source/Virtual` num stream cliente derruba o audioconvert do
        // PipeWire 1.6.8 (reproduzível com `pw-loopback --playback-props`).
        *pw::keys::MEDIA_CLASS => "Audio/Source",
        *pw::keys::NODE_NAME => node_name,
        *pw::keys::NODE_DESCRIPTION => description,
    };
    let stream = pw::stream::StreamRc::new(core, "fw-test-mic", props).expect("stream");
    let cursor = Rc::new(RefCell::new((samples, 0usize)));
    let _listener = stream
        .add_local_listener_with_user_data(cursor)
        .process(|stream, cursor| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let requested = buffer.requested() as usize;
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let frames = match data.data() {
                Some(slice) => {
                    let max = slice.len() / 4;
                    let frames = if requested > 0 {
                        requested.min(max)
                    } else {
                        max
                    };
                    let mut c = cursor.borrow_mut();
                    for i in 0..frames {
                        let v = c.0.get(c.1).copied().unwrap_or(0.0);
                        c.1 += 1;
                        slice[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                    }
                    frames
                }
                None => 0,
            };
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = 4;
            *chunk.size_mut() = (frames * 4) as u32;
        })
        .register()
        .expect("listener");

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(16_000);
    info.set_channels(1);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .expect("pod")
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&bytes).expect("pod")];
    // Sem AUTOCONNECT: o nó só existe para ser capturado.
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .expect("connect");

    let _rx = rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    let _ = ready.send(());
    mainloop.run();
    let _ = stream.disconnect();
}
