//! The PipeWire side: a stream that consumes the portal's node over the
//! file descriptor the portal opened, with shared-memory buffers only
//! (ADR-0037 D17). Its callbacks copy each buffer into the hand-over and the
//! scaling thread downscales it.
//!
//! The compositor is offered formats without modifiers, so it can't choose
//! DMA-BUF and sends memory it has already copied. The GPU path (DMA-BUF into
//! EGL) is a documented follow-up, not part of this crate yet.
//!
//! libpipewire runs `process` on its data thread as well as the loop's, and
//! `param_changed` and `state_changed` on the loop's, so everything the
//! callbacks share is behind a lock and none of them keeps state of its own.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use folo_capture_core::error::CaptureError;
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::stream::{StreamBox, StreamFlags, StreamState};
use spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
use tokio::sync::mpsc::UnboundedSender;

use crate::handoff::Handoff;
use crate::scale::PixelLayout;
use crate::shared::Shared;
use crate::sink::{self, FrameSink};

/// The largest screen the stream is offered, per side.
const LARGEST_SIDE: u32 = 16_384;
/// The size the offer names as its default, as PipeWire's examples do.
const DEFAULT_SIZE: (u32, u32) = (1920, 1080);

/// What the stream threads tell the session worker.
#[derive(Debug)]
pub enum StreamEvent {
    /// The stream is running: buffers will come.
    Streaming,
    /// The stream is over, with the reason. Not sent after a stop request.
    Ended(String),
}

/// The stream's threads, to stop and join.
pub struct StreamHandle {
    stop: pw::channel::Sender<()>,
    stream_thread: Option<JoinHandle<()>>,
    scaling_thread: Option<JoinHandle<()>>,
    handoff: Arc<Handoff>,
}

// Dropping the handle stops the stream's loop and the scaling thread and waits
// for both, so no path out of the session can leave them running.
impl Drop for StreamHandle {
    fn drop(&mut self) {
        // If the loop is already gone, there is nothing left to stop.
        let _ = self.stop.send(());
        if let Some(thread) = self.stream_thread.take() {
            let _ = thread.join();
        }
        self.handoff.close();
        if let Some(thread) = self.scaling_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the threads for the portal's node.
///
/// # Errors
///
/// [`CaptureError::Failed`] when a thread can't be started.
pub fn spawn(
    remote: OwnedFd,
    node_id: u32,
    shared: &Arc<Shared>,
    events: &UnboundedSender<StreamEvent>,
) -> Result<StreamHandle, CaptureError> {
    let handoff = Arc::new(Handoff::default());
    let unstarted = |what: &str, error: std::io::Error| {
        CaptureError::Failed(format!("can't start the {what} thread: {error}"))
    };
    let scaling_thread = thread::Builder::new()
        .name("folo-capture-scale".to_owned())
        .spawn({
            let (handoff, events) = (Arc::clone(&handoff), events.clone());
            let sink = FrameSink::new(Arc::clone(shared));
            move || sink::run(&handoff, sink, &events)
        })
        .map_err(|error| unstarted("scaling", error))?;
    let (stop, stop_receiver) = pw::channel::channel::<()>();
    let arguments = Arguments {
        remote,
        node_id,
        rate_hz: shared.target().rate_hz,
        shared: Arc::clone(shared),
        handoff: Arc::clone(&handoff),
        events: events.clone(),
    };
    let stream_events = events.clone();
    let stream_thread = thread::Builder::new()
        .name("folo-capture-pipewire".to_owned())
        .spawn(move || {
            if let Err(error) = run(arguments, stop_receiver) {
                let _ = stream_events.send(StreamEvent::Ended(error));
            }
        });
    match stream_thread {
        Ok(stream_thread) => Ok(StreamHandle {
            stop,
            stream_thread: Some(stream_thread),
            scaling_thread: Some(scaling_thread),
            handoff,
        }),
        Err(error) => {
            handoff.close();
            let _ = scaling_thread.join();
            Err(unstarted("PipeWire", error))
        }
    }
}

struct Arguments {
    remote: OwnedFd,
    node_id: u32,
    rate_hz: f64,
    shared: Arc<Shared>,
    handoff: Arc<Handoff>,
    events: UnboundedSender<StreamEvent>,
}

/// What the compositor settled on.
#[derive(Debug, Default, Clone, Copy)]
struct Negotiated {
    layout: Option<PixelLayout>,
    width: u32,
    height: u32,
}

fn layout_of(format: VideoFormat) -> Option<PixelLayout> {
    [
        (VideoFormat::BGRx, PixelLayout::Bgrx),
        (VideoFormat::BGRA, PixelLayout::Bgra),
        (VideoFormat::RGBx, PixelLayout::Rgbx),
        (VideoFormat::RGBA, PixelLayout::Rgba),
        (VideoFormat::RGB, PixelLayout::Rgb),
        (VideoFormat::BGR, PixelLayout::Bgr),
    ]
    .into_iter()
    .find_map(|(spa_format, layout)| (spa_format == format).then_some(layout))
}

fn run(arguments: Arguments, stop: pw::channel::Receiver<()>) -> Result<(), String> {
    let Arguments {
        remote,
        node_id,
        rate_hz,
        shared,
        handoff,
        events,
    } = arguments;
    let failed = |what: &str, error: pw::Error| format!("PipeWire: {what}: {error}");
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| failed("main loop", e))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| failed("context", e))?;
    let core = context
        .connect_fd_rc(remote, None)
        .map_err(|e| failed("connecting to the portal's remote", e))?;
    let _core_listener = core
        .add_listener_local()
        .error({
            let events = events.clone();
            let mainloop = mainloop.clone();
            move |id, _sequence, result, message| {
                // Errors of the core itself (id 0) mean the connection is gone.
                if id == 0 {
                    let _ = events.send(StreamEvent::Ended(format!(
                        "the PipeWire connection failed ({result}): {message}"
                    )));
                    mainloop.quit();
                }
            }
        })
        .register();
    let stream = StreamBox::new(
        &core,
        "folo-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| failed("stream", e))?;
    let _listener = register_callbacks(&stream, shared, handoff, events, &mainloop)?;
    let offer = format_offer(rate_hz)?;
    let mut params = [Pod::from_bytes(&offer).ok_or("PipeWire: the format offer is not a pod")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| failed("connecting the stream", e))?;
    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    mainloop.run();
    Ok(())
}

fn register_callbacks(
    stream: &StreamBox<'_>,
    shared: Arc<Shared>,
    handoff: Arc<Handoff>,
    events: UnboundedSender<StreamEvent>,
    mainloop: &pw::main_loop::MainLoopRc,
) -> Result<pw::stream::StreamListener<()>, String> {
    let negotiated = Arc::new(Mutex::new(Negotiated::default()));
    let unreadable = Arc::new(AtomicBool::new(false));
    let (state_events, quit) = (events.clone(), mainloop.clone());
    let format_state = Arc::clone(&negotiated);
    stream
        .add_local_listener::<()>()
        .state_changed(move |_, (), _old, new| match new {
            StreamState::Streaming => {
                let _ = state_events.send(StreamEvent::Streaming);
            }
            StreamState::Error(reason) => {
                let _ = state_events.send(StreamEvent::Ended(format!(
                    "the PipeWire stream failed: {reason}"
                )));
                quit.quit();
            }
            _ => {}
        })
        .param_changed(move |stream, (), id, param| {
            on_format(stream, &format_state, &shared, id, param);
        })
        .process(move |stream, ()| {
            on_buffer(stream, &negotiated, &handoff, &unreadable, &events);
        })
        .register()
        .map_err(|error| format!("PipeWire: registering the stream's callbacks: {error}"))
}

/// The compositor settled on a format: remember its layout and size, and
/// say which buffers the agent can read.
fn on_format(
    stream: &pw::stream::Stream,
    negotiated: &Mutex<Negotiated>,
    shared: &Shared,
    id: u32,
    param: Option<&Pod>,
) {
    let Some(param) = param else { return };
    if id != spa::param::ParamType::Format.as_raw() {
        return;
    }
    let is_raw_video =
        spa::param::format_utils::parse_format(param).is_ok_and(|(kind, subtype)| {
            kind == spa::param::format::MediaType::Video
                && subtype == spa::param::format::MediaSubtype::Raw
        });
    let mut format = VideoInfoRaw::default();
    if !is_raw_video || format.parse(param).is_err() {
        return;
    }
    let size = format.size();
    let layout = layout_of(format.format());
    *negotiated.lock().unwrap_or_else(PoisonError::into_inner) = Negotiated {
        layout,
        width: size.width,
        height: size.height,
    };
    shared.set_screen_size((size.width, size.height));
    if let Some(layout) = layout
        && let Ok(offer) = buffer_offer(size.width, size.height, layout)
        && let Some(pod) = Pod::from_bytes(&offer)
    {
        // A refused update leaves the compositor's own buffers, which is fine.
        let _ = stream.update_params(&mut [pod]);
    }
}

/// A buffer is ready: copy it into the hand-over and give it back.
fn on_buffer(
    stream: &pw::stream::Stream,
    negotiated: &Mutex<Negotiated>,
    handoff: &Handoff,
    unreadable: &AtomicBool,
    events: &UnboundedSender<StreamEvent>,
) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Some(plane) = buffer.datas_mut().first_mut() else {
        return;
    };
    let (used, stride, offset) = {
        let chunk = plane.chunk();
        (
            chunk.size() as usize,
            usize::try_from(chunk.stride()).unwrap_or(0),
            chunk.offset() as usize,
        )
    };
    // The compositor sends empty buffers when only the cursor moved, which
    // isn't part of the stream.
    if used == 0
        || plane
            .chunk()
            .flags()
            .contains(spa::buffer::ChunkFlags::CORRUPTED)
    {
        return;
    }
    let format = *negotiated.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(layout) = format.layout else { return };
    let (width, height) = (format.width as usize, format.height as usize);
    // A plane that can't be mapped is a DMA-BUF the agent can't read.
    let Some(bytes) = plane.data() else {
        if !unreadable.swap(true, Ordering::Relaxed) {
            let _ = events.send(StreamEvent::Ended(
                "the compositor sent DMA-BUF buffers, which this agent can't read yet".to_owned(),
            ));
        }
        return;
    };
    let stride = if stride == 0 {
        width * layout.bytes_per_pixel()
    } else {
        stride
    };
    if width == 0 || height == 0 || stride < width * layout.bytes_per_pixel() {
        return;
    }
    let needed = (height - 1) * stride + width * layout.bytes_per_pixel();
    // A chunk shorter than the picture is a partial write: show the last whole one.
    if used < needed {
        return;
    }
    if let Some(pixels) = bytes.get(offset..offset + needed) {
        handoff.put(layout, (width, height), stride, pixels);
    }
}

/// The formats the agent takes, as the stream's `EnumFormat`: opaque 8-bit
/// colour in the usual byte orders, any size, at most `rate_hz` frames a
/// second. No modifier is named, which keeps the compositor on shared memory.
fn format_offer(rate_hz: f64) -> Result<Vec<u8>, String> {
    // The rate is 1 to 60 (CaptureRequest::validate), so the cast is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rate = rate_hz.clamp(1.0, 60.0).round() as u32;
    let object = spa::pod::object!(
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
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::RGB,
            VideoFormat::BGR
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: DEFAULT_SIZE.0,
                height: DEFAULT_SIZE.1
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: LARGEST_SIDE,
                height: LARGEST_SIDE
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: rate,
                denom: 1
            }
        ),
    );
    serialize(object)
}

/// The buffers the agent can read: one plane of shared memory (`MemFd` or
/// `MemPtr`, not DMA-BUF), at least as large as the picture. The compositor
/// may use more than that, for stride padding.
fn buffer_offer(width: u32, height: u32, layout: PixelLayout) -> Result<Vec<u8>, String> {
    let too_big = |_| "the screen is too large for PipeWire's buffer sizes".to_owned();
    let stride = i32::try_from(width as usize * layout.bytes_per_pixel()).map_err(too_big)?;
    let size = stride
        .checked_mul(i32::try_from(height).map_err(too_big)?)
        .ok_or("the screen is too large for PipeWire's buffer sizes")?;
    let range = |least: i32| {
        Value::Choice(ChoiceValue::Int(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: least,
                min: least,
                max: i32::MAX,
            },
        )))
    };
    let property = |key: u32, value: Value| Property {
        key,
        flags: PropertyFlags::empty(),
        value,
    };
    let kinds = (1 << spa::sys::SPA_DATA_MemFd) | (1 << spa::sys::SPA_DATA_MemPtr);
    let object = Object {
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
            property(spa::sys::SPA_PARAM_BUFFERS_size, range(size)),
            property(spa::sys::SPA_PARAM_BUFFERS_stride, range(stride)),
            property(spa::sys::SPA_PARAM_BUFFERS_dataType, Value::Int(kinds)),
        ],
    };
    serialize(object)
}

fn serialize(object: Object) -> Result<Vec<u8>, String> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|error| format!("PipeWire: a parameter can't be built: {error:?}"))
}
