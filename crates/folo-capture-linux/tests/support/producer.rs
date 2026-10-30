//! A test producer: a PipeWire video source node that shows a picture the
//! test sets, as a screen compositor would (`BGRx`, with padding after each
//! row, at 30 frames a second). It stands in for the compositor's node in the
//! portal's answer.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};

/// Bytes after the pixels of each row, to make the stride differ from the width.
const PADDING: usize = 16;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// A picture of rgb pixels, row-major.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl Image {
    /// A picture of one colour.
    pub fn flat(width: u32, height: u32, rgb: [u8; 3]) -> Self {
        Self {
            width,
            height,
            pixels: vec![rgb; (width * height) as usize],
        }
    }

    /// Four quadrants: red top left, green top right, blue bottom left, white bottom right.
    pub fn quadrants(width: u32, height: u32) -> Self {
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push(match (x < width / 2, y < height / 2) {
                    (true, true) => [255, 0, 0],
                    (false, true) => [0, 255, 0],
                    (true, false) => [0, 0, 255],
                    (false, false) => [255, 255, 255],
                });
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }
}

pub struct LocalProducer {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    image: Arc<Mutex<Image>>,
    node_id: u32,
}

impl LocalProducer {
    /// Connects to the PipeWire daemon at `socket` and offers a video source.
    pub fn start(socket: &Path, image: Image) -> Self {
        let socket = socket.to_owned();
        let shared = Arc::new(Mutex::new(image));
        let (stop, stop_receiver) = pw::channel::channel::<()>();
        let (announce, announced) = channel();
        let frames = Arc::clone(&shared);
        let thread = thread::spawn(move || {
            if let Err(error) = run(&socket, &frames, stop_receiver, &announce) {
                eprintln!("test producer: {error}");
            }
        });
        let node_id = announced
            .recv_timeout(Duration::from_secs(15))
            .expect("the test producer's node never appeared");
        Self {
            stop,
            thread: Some(thread),
            image: shared,
            node_id,
        }
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    /// Shows another picture of the same size from the next frame on.
    pub fn show(&self, image: Image) {
        *self.image.lock().unwrap() = image;
    }
}

impl Drop for LocalProducer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct State {
    image: Arc<Mutex<Image>>,
    announce: Sender<u32>,
    announced: bool,
}

fn run(
    socket: &Path,
    image: &Arc<Mutex<Image>>,
    stop: pw::channel::Receiver<()>,
    announce: &Sender<u32>,
) -> Result<(), String> {
    let (width, height) = {
        let image = image.lock().unwrap();
        (image.width, image.height)
    };
    pw::init();
    let remote: OwnedFd = UnixStream::connect(socket)
        .map_err(|error| format!("connecting to {}: {error}", socket.display()))?
        .into();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context
        .connect_fd_rc(remote, None)
        .map_err(|e| e.to_string())?;
    let stream = pw::stream::StreamRc::new(
        core,
        "folo-test-screen",
        properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::NODE_NAME => "folo-test-screen",
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;
    let state = State {
        image: Arc::clone(image),
        announce: announce.clone(),
        announced: false,
    };
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|stream, state, _old, new| {
            let ready = matches!(
                new,
                pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming
            );
            if ready && !state.announced && stream.node_id() != u32::MAX {
                state.announced = true;
                let _ = state.announce.send(stream.node_id());
            }
        })
        .param_changed(move |stream, _state, id, param| {
            if param.is_some() && id == spa::param::ParamType::Format.as_raw() {
                let buffers = buffers_param(width, height);
                if let Some(pod) = Pod::from_bytes(&buffers) {
                    let _ = stream.update_params(&mut [pod]);
                }
            }
        })
        .process(fill)
        .register()
        .map_err(|e| e.to_string())?;
    let offer = format_param(width, height);
    let mut params = [Pod::from_bytes(&offer).ok_or("the format isn't a pod")?];
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;
    let ticking = stream.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        if ticking.state() == pw::stream::StreamState::Streaming {
            let _ = ticking.trigger_process();
        }
    });
    timer
        .update_timer(Some(FRAME_INTERVAL), Some(FRAME_INTERVAL))
        .into_result()
        .map_err(|e| e.to_string())?;
    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    mainloop.run();
    Ok(())
}

/// libpipewire may call `process` from two threads, and a stream's buffers
/// are for one at a time.
static FILLING: Mutex<()> = Mutex::new(());

/// Writes the current picture into the next buffer as `BGRx`.
fn fill(stream: &pw::stream::Stream, state: &mut State) {
    let _one_at_a_time = FILLING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Some(plane) = buffer.datas_mut().first_mut() else {
        return;
    };
    let image = state.image.lock().unwrap().clone();
    let stride = image.width as usize * 4 + PADDING;
    let size = stride * image.height as usize;
    let Some(bytes) = plane.data() else { return };
    if bytes.len() < size {
        return;
    }
    for (index, [r, g, b]) in image.pixels.iter().copied().enumerate() {
        let (x, y) = (index % image.width as usize, index / image.width as usize);
        let at = y * stride + x * 4;
        bytes[at..at + 4].copy_from_slice(&[b, g, r, 0xFF]);
    }
    let chunk = plane.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.size_mut() = u32::try_from(size).unwrap();
    *chunk.stride_mut() = i32::try_from(stride).unwrap();
}

fn serialize(object: Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner()
}

fn format_param(width: u32, height: u32) -> Vec<u8> {
    serialize(spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Id,
            spa::param::video::VideoFormat::BGRx
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            spa::utils::Rectangle { width, height }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 }
        ),
    ))
}

/// Buffers of shared memory (memfd or plain memory), one plane each.
fn buffers_param(width: u32, height: u32) -> Vec<u8> {
    let stride = i32::try_from(width as usize * 4 + PADDING).unwrap();
    let size = stride * i32::try_from(height).unwrap();
    let property = |key: u32, value: Value| Property {
        key,
        flags: PropertyFlags::empty(),
        value,
    };
    let kinds = (1 << spa::sys::SPA_DATA_MemFd) | (1 << spa::sys::SPA_DATA_MemPtr);
    serialize(Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            property(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 8,
                        min: 2,
                        max: 16,
                    },
                ))),
            ),
            property(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            property(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(size)),
            property(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride)),
            property(spa::sys::SPA_PARAM_BUFFERS_dataType, Value::Int(kinds)),
        ],
    })
}

/// The environment variable that turns a run of the test binary into a
/// producer process.
pub const PRODUCER_VARIABLE: &str = "FOLO_TEST_PRODUCER";

/// A producer in a process of its own, as the compositor is in life. The test
/// binary runs itself again with [`PRODUCER_VARIABLE`] set (the `producer_process`
/// test is the entry), reads commands on stdin and says its node id on stdout.
pub struct Producer {
    child: std::process::Child,
    node_id: u32,
}

impl Producer {
    pub fn start(socket: &Path, image: &Image) -> Self {
        use std::io::{BufRead as _, BufReader};
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "producer_process",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(
                PRODUCER_VARIABLE,
                format!("{}:{}:{}", socket.display(), image.width, image.height),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("starting the producer process");
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let node_id = loop {
            let line = lines
                .next()
                .expect("the producer process ended before it had a node")
                .unwrap();
            // The harness prints "test producer_process ... " in front of it.
            if let Some((_, id)) = line.rsplit_once("NODE ") {
                break id.trim().parse().unwrap();
            }
        };
        // The rest of its output isn't needed; the pipe is read until it closes
        // so the child never blocks on a full pipe.
        std::thread::spawn(move || lines.for_each(drop));
        let mut producer = Self { child, node_id };
        producer.show(image);
        producer
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    fn command(&mut self, line: &str) {
        use std::io::Write as _;
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// Shows another picture of the same size from the next frame on.
    pub fn show(&mut self, image: &Image) {
        let flat = image.pixels.first().copied().unwrap_or_default();
        if image.pixels.iter().all(|pixel| *pixel == flat) {
            self.command(&format!("flat {} {} {}", flat[0], flat[1], flat[2]));
        } else {
            self.command("quadrants");
        }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        let _ = self.command_quit();
        let _ = self.child.wait();
    }
}

impl Producer {
    fn command_quit(&mut self) -> std::io::Result<()> {
        use std::io::Write as _;
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "quit")?;
        stdin.flush()
    }
}

/// The producer process's body: what the `producer_process` test runs when
/// [`PRODUCER_VARIABLE`] is set.
pub fn run_producer_process(spec: &str) {
    use std::io::BufRead as _;
    let mut parts = spec.rsplitn(3, ':');
    let height: u32 = parts.next().unwrap().parse().unwrap();
    let width: u32 = parts.next().unwrap().parse().unwrap();
    let socket = std::path::PathBuf::from(parts.next().unwrap());
    let producer = LocalProducer::start(&socket, Image::quadrants(width, height));
    // Written to the stream itself: `println!` would go into the test harness's
    // capture when the binary isn't run with --nocapture.
    {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "NODE {}", producer.node_id()).unwrap();
        stdout.flush().unwrap();
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("flat") => {
                let rgb: Vec<u8> = words.map(|word| word.parse().unwrap()).collect();
                producer.show(Image::flat(width, height, [rgb[0], rgb[1], rgb[2]]));
            }
            Some("quadrants") => producer.show(Image::quadrants(width, height)),
            _ => break,
        }
    }
}
