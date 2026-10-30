//! The agent's capture side (ADR-0037 D3, D5, D13): a backend, the pacer in
//! front of it, and the state `capture.status` reports. [`Agent`] is the
//! state machine, driven without a network or a thread; [`spawn`] runs it on
//! a thread of its own, because backends block waiting for pictures.
//!
//! The agent never captures on its own: its state is `off` until
//! `capture.start` and again after `capture.stop` (D3). While the backend
//! can't capture (its [`CaptureBackend::status`] says `paused`), the state is
//! `paused`, the last picture goes out again with `held`, and the status
//! carries the backend's message and settings page (D13).

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::clock::Clock;
use folo_capture_core::codec::{self, MESSAGE_TYPE};
use folo_capture_core::error::CaptureError;
use folo_capture_core::pacer::FramePacer;
use folo_capture_core::types::{CaptureRequest, CaptureState, CaptureStatus, ScreensListResult};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};
use tokio_tungstenite::tungstenite::Bytes;

use crate::logs::{Level, log};

/// JSON-RPC errors the agent answers with (docs/specs/source-link.md#the-capture-link).
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// A request the agent can't carry out, such as an unknown screen.
pub const FAILED: i64 = -32000;

/// How long one step waits for the backend's next picture, so that a command
/// is answered within about this long while capturing.
const STEP: Duration = Duration::from_millis(20);
/// How long an idle worker waits for a command before it looks again.
const IDLE: Duration = Duration::from_millis(100);
/// The frames-per-second figure covers this many microseconds.
const FPS_WINDOW_US: u64 = 1_000_000;

/// A JSON-RPC error answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: i64,
    pub message: String,
}

impl Failure {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<CaptureError> for Failure {
    fn from(error: CaptureError) -> Self {
        let code = match error {
            CaptureError::InvalidRequest(_) => INVALID_PARAMS,
            _ => FAILED,
        };
        Self::new(code, error.to_string())
    }
}

/// The capture that is running.
struct Capture {
    request: CaptureRequest,
    pacer: FramePacer,
    window_start_us: u64,
    sent_in_window: u32,
    /// A picture of this capture has come.
    pictured: bool,
    /// The newest picture had `held` set (a replay of a pause).
    picture_held: bool,
}

/// The capture side of the agent.
pub struct Agent {
    backend: Box<dyn CaptureBackend>,
    clock: Box<dyn Clock>,
    capture: Option<Capture>,
    status: CaptureStatus,
}

impl Agent {
    /// The agent of `backend`, stamping its frames with `clock`: the
    /// operating system's monotonic counter for the real backends (D6), as
    /// [`crate::backends::BackendChoice::clock`] gives it.
    #[must_use]
    pub fn new(backend: Box<dyn CaptureBackend>, clock: Box<dyn Clock>) -> Self {
        Self {
            backend,
            clock,
            capture: None,
            status: CaptureStatus::off(),
        }
    }

    /// What `capture.status` says now.
    #[must_use]
    pub fn status(&self) -> &CaptureStatus {
        &self.status
    }

    /// Whether a capture is running, so that frames are to be polled.
    #[must_use]
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Answers a request of the capture link, except `restart`, which the
    /// link ends the agent with.
    ///
    /// # Errors
    ///
    /// A [`Failure`]: `-32601` for a method it doesn't know, `-32602` for
    /// params it can't take, `-32000` for a request it can't carry out.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        match method {
            "screens.list" => self.list_screens(),
            "capture.start" => self.start(params),
            "capture.stop" => {
                self.stop();
                Ok(json!({}))
            }
            _ => Err(Failure::new(
                METHOD_NOT_FOUND,
                format!("no method {method}"),
            )),
        }
    }

    fn list_screens(&mut self) -> Result<Value, Failure> {
        let screens = self.backend.list_screens()?;
        serde_json::to_value(ScreensListResult { screens })
            .map_err(|error| Failure::new(FAILED, error.to_string()))
    }

    /// `capture.start`: starts, or applies the changes in place while
    /// capturing the same screen. Answers `{}` once the backend accepted it;
    /// the progress is in the status.
    fn start(&mut self, params: Value) -> Result<Value, Failure> {
        let request: CaptureRequest = serde_json::from_value(params)
            .map_err(|error| Failure::new(INVALID_PARAMS, format!("capture.start: {error}")))?;
        request.validate()?;
        // From off a new session may ask for consent; the backend decides
        if let Err(error) = self.backend.start(&request) {
            if self.capture.is_some() {
                self.stop();
            }
            return Err(error.into());
        }
        match &mut self.capture {
            Some(capture) if capture.request.screen == request.screen => {
                capture.pacer.set_rate(request.rate_hz);
                capture.request = request;
            }
            _ => {
                self.status = CaptureStatus {
                    state: CaptureState::Starting,
                    backend: self.backend.name().to_owned(),
                    screen: request.screen.clone(),
                    ..CaptureStatus::off()
                };
                self.capture = Some(Capture {
                    pacer: FramePacer::new(request.rate_hz),
                    request,
                    window_start_us: self.clock.now_us(),
                    sent_in_window: 0,
                    pictured: false,
                    picture_held: false,
                });
            }
        }
        Ok(json!({}))
    }

    /// Ends the capture. The state is `off`, and nothing is left of it.
    pub fn stop(&mut self) {
        if self.capture.take().is_some() {
            self.backend.stop();
        }
        self.status = CaptureStatus::off();
    }

    /// Waits up to `timeout` for the next picture and gives the message to
    /// send, if one is due: the type byte `0x04` and the frame.
    pub fn step(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        let capture = self.capture.as_mut()?;
        match self.backend.next_picture(timeout) {
            Ok(Some(picture)) => {
                capture.pictured = true;
                capture.picture_held = picture.flags.held;
                self.status.hdr = picture.flags.hdr;
                self.status.protected = picture.flags.protected;
                self.status.bars = picture.bars.fractions(picture.width, picture.height);
                capture.pacer.push(picture);
            }
            Ok(None) => {}
            Err(error) => {
                self.fail(&error);
                return None;
            }
        }
        // While the backend can't capture, the repeats of the last picture
        // carry `held` and the status says why (D13)
        let backend = self.backend.status();
        let held = backend.paused || capture.picture_held;
        capture.pacer.set_held(held);
        self.status.state = if held {
            CaptureState::Paused
        } else if capture.pictured {
            CaptureState::Capturing
        } else {
            CaptureState::Starting
        };
        self.status.message = backend.message;
        self.status.settings_uri = backend.settings_uri;
        let now = self.clock.now_us();
        let frame = capture.pacer.poll(now)?;
        capture.sent_in_window += 1;
        let elapsed = now.saturating_sub(capture.window_start_us);
        if elapsed >= FPS_WINDOW_US {
            self.status.fps =
                f64::from(capture.sent_in_window) / Duration::from_micros(elapsed).as_secs_f64();
            capture.window_start_us = now;
            capture.sent_in_window = 0;
        }
        match codec::encode(&frame) {
            Ok(bytes) => {
                let mut message = Vec::with_capacity(bytes.len() + 1);
                message.push(MESSAGE_TYPE);
                message.extend_from_slice(&bytes);
                Some(message)
            }
            Err(error) => {
                self.fail(&error.into());
                None
            }
        }
    }

    /// The backend failed: the state is `error` and the capture ends. The node
    /// may start it again.
    fn fail(&mut self, error: &CaptureError) {
        log(Level::Error, &format!("capture failed: {error}"));
        self.capture = None;
        self.backend.stop();
        self.status = CaptureStatus {
            state: CaptureState::Error,
            backend: self.backend.name().to_owned(),
            message: Some(error.to_string()),
            ..CaptureStatus::off()
        };
    }
}

/// What the link asks of the worker.
pub enum Command {
    /// A request of the capture link.
    Call {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, Failure>>,
    },
    /// Stops capturing (the link is gone, or the agent ends); answered when done.
    Stop { done: oneshot::Sender<()> },
}

/// The link's end of the worker.
#[derive(Clone)]
pub struct WorkerHandle {
    pub commands: mpsc::Sender<Command>,
    /// The newest frame message (type byte and frame): newest wins.
    pub frames: watch::Receiver<Option<Bytes>>,
    pub status: watch::Receiver<CaptureStatus>,
}

/// Runs the agent on a thread of its own. The thread ends when every
/// [`WorkerHandle`] is dropped.
#[must_use]
pub fn spawn(mut agent: Agent) -> (WorkerHandle, JoinHandle<()>) {
    let (commands, inbox) = mpsc::channel();
    let (frames_out, frames) = watch::channel(None);
    let (status_out, status) = watch::channel(agent.status().clone());
    let thread = thread::spawn(move || {
        loop {
            let wait = if agent.capturing() {
                Duration::ZERO
            } else {
                IDLE
            };
            match inbox.recv_timeout(wait) {
                Ok(command) => handle(&mut agent, command, &frames_out),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(command) = inbox.try_recv() {
                handle(&mut agent, command, &frames_out);
            }
            if let Some(message) = agent.step(STEP) {
                frames_out.send_replace(Some(Bytes::from(message)));
            } else if !agent.capturing() {
                // Nothing of a capture that ended is left to send
                frames_out.send_if_modified(|frame| frame.take().is_some());
            }
            let status = agent.status().clone();
            status_out.send_if_modified(|shown| {
                if *shown == status {
                    false
                } else {
                    *shown = status;
                    true
                }
            });
        }
        agent.stop();
    });
    (
        WorkerHandle {
            commands,
            frames,
            status,
        },
        thread,
    )
}

fn handle(agent: &mut Agent, command: Command, frames: &watch::Sender<Option<Bytes>>) {
    match command {
        Command::Call {
            method,
            params,
            reply,
        } => {
            let before = agent.status().screen.clone();
            let answer = agent.call(&method, params);
            if agent.status().screen != before {
                // The old screen's last frame must not follow a start of another
                frames.send_replace(None);
            }
            let _ = reply.send(answer);
        }
        Command::Stop { done } => {
            agent.stop();
            // A frame from the capture that ended must not go out afterwards
            frames.send_replace(None);
            let _ = done.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use folo_capture_core::backend::BackendStatus;
    use folo_capture_core::clock::ProcessClock;
    use folo_capture_core::replay::{self, ReplayBackend};
    use folo_capture_core::synthetic::{Pattern, SyntheticBackend};
    use folo_capture_core::types::{Bars, FrameFlags, Picture, Screen, ScreenFrame};

    use super::*;

    fn start_params(screen: &str) -> Value {
        json!({
            "screen": screen, "width": 64, "height": 36, "rate_hz": 60,
            "black_bars": {"preset": "auto", "mode": "default", "threshold": 0.05},
            "profile": "video",
        })
    }

    fn synthetic() -> Agent {
        Agent::new(
            Box::new(SyntheticBackend::unpaced(Pattern::Letterbox)),
            Box::new(ProcessClock::new()),
        )
    }

    /// What [`FakeBackend`] gives: the pictures to come, and its status.
    #[derive(Default)]
    struct Script {
        pictures: VecDeque<Picture>,
        status: BackendStatus,
    }

    /// A backend the test drives: pictures come when the script has them, and
    /// the status is whatever the script says.
    struct FakeBackend(Arc<Mutex<Script>>);

    impl CaptureBackend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
            Ok(vec![Screen {
                id: "fake-0".to_owned(),
                name: "Fake".to_owned(),
                width: 16,
                height: 9,
                primary: true,
            }])
        }

        fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError> {
            if request.screen == "fake-0" {
                Ok(())
            } else {
                Err(CaptureError::UnknownScreen(request.screen.clone()))
            }
        }

        fn next_picture(&mut self, _timeout: Duration) -> Result<Option<Picture>, CaptureError> {
            Ok(self.0.lock().unwrap().pictures.pop_front())
        }

        fn stop(&mut self) {}

        fn status(&self) -> BackendStatus {
            self.0.lock().unwrap().status.clone()
        }
    }

    /// A clock the test moves.
    struct FakeClock(Arc<AtomicU64>);

    impl Clock for FakeClock {
        fn now_us(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// An agent on a [`FakeBackend`] and a [`FakeClock`] at 5 s, capturing.
    fn scripted() -> (Agent, Arc<Mutex<Script>>, Arc<AtomicU64>) {
        let script = Arc::new(Mutex::new(Script::default()));
        let time = Arc::new(AtomicU64::new(5_000_000));
        let mut agent = Agent::new(
            Box::new(FakeBackend(Arc::clone(&script))),
            Box::new(FakeClock(Arc::clone(&time))),
        );
        agent.call("capture.start", start_params("fake-0")).unwrap();
        (agent, script, time)
    }

    fn picture() -> Picture {
        Picture {
            screen_index: 0,
            width: 16,
            height: 9,
            flags: FrameFlags::default(),
            bars: Bars::default(),
            pixels: vec![40; 16 * 9 * 3],
        }
    }

    fn decoded(message: &[u8]) -> ScreenFrame {
        assert_eq!(message[0], MESSAGE_TYPE);
        codec::decode(&message[1..]).expect("a frame the codec reads")
    }

    /// Steps until a frame comes, at most a second.
    fn next_frame(agent: &mut Agent) -> Vec<u8> {
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            if let Some(message) = agent.step(Duration::from_millis(5)) {
                return message;
            }
        }
        panic!("no frame in a second");
    }

    #[test]
    fn the_agent_is_off_until_it_is_told_to_capture_and_off_again_after_stop() {
        let mut agent = synthetic();
        assert_eq!(agent.status().state, CaptureState::Off);
        assert!(
            agent.step(Duration::ZERO).is_none(),
            "nothing without a start"
        );
        assert_eq!(
            agent.call("capture.start", start_params("synthetic-0")),
            Ok(json!({}))
        );
        assert_eq!(agent.status().state, CaptureState::Starting);
        assert_eq!(agent.status().screen, "synthetic-0");
        assert_eq!(agent.status().backend, "synthetic");
        next_frame(&mut agent);
        assert_eq!(agent.status().state, CaptureState::Capturing);
        assert!(
            agent.status().bars.top > 0.0,
            "the letterbox's bars are reported"
        );
        assert_eq!(agent.call("capture.stop", json!({})), Ok(json!({})));
        assert_eq!(agent.status(), &CaptureStatus::off());
        assert!(!agent.capturing());
        // Stopping again is fine
        assert_eq!(agent.call("capture.stop", json!({})), Ok(json!({})));
    }

    #[test]
    fn frames_are_type_four_messages_that_fit_the_box_and_count_up() {
        let mut agent = synthetic();
        agent
            .call("capture.start", start_params("synthetic-0"))
            .unwrap();
        let first = next_frame(&mut agent);
        assert_eq!(first[0], MESSAGE_TYPE);
        let frame = codec::decode(&first[1..]).expect("a frame the codec reads");
        let (width, height) = (frame.picture.width, frame.picture.height);
        assert!(width <= 64 && height <= 36, "{width}x{height}");
        // 16:9 into 64 x 36 is 64 x 36
        assert_eq!((width, height), (64, 36));
        let second = codec::decode(&next_frame(&mut agent)[1..]).unwrap();
        assert_eq!(second.sequence, frame.sequence.wrapping_add(1));
        assert!(second.timestamp_us >= frame.timestamp_us);
    }

    #[test]
    fn a_changed_request_applies_in_place() {
        let mut agent = synthetic();
        agent
            .call("capture.start", start_params("synthetic-0"))
            .unwrap();
        let before = codec::decode(&next_frame(&mut agent)[1..]).unwrap();
        let mut bigger = start_params("synthetic-0");
        bigger["width"] = json!(128);
        bigger["height"] = json!(72);
        agent.call("capture.start", bigger).unwrap();
        // No new session: the state doesn't go back to starting, the sequence goes on
        assert_eq!(agent.status().state, CaptureState::Capturing);
        let mut after = codec::decode(&next_frame(&mut agent)[1..]).unwrap();
        assert!(after.sequence > before.sequence);
        // The size follows the box once the backend delivers it
        while after.picture.width != 128 {
            after = codec::decode(&next_frame(&mut agent)[1..]).unwrap();
        }
        assert_eq!(after.picture.height, 72);
    }

    #[test]
    fn refusals_have_the_codes_of_the_link() {
        let mut agent = synthetic();
        let unknown = agent
            .call("capture.start", start_params("nowhere"))
            .unwrap_err();
        assert_eq!(unknown.code, FAILED);
        assert!(!agent.capturing());
        let mut wide = start_params("synthetic-0");
        wide["width"] = json!(481);
        assert_eq!(
            agent.call("capture.start", wide).unwrap_err().code,
            INVALID_PARAMS
        );
        assert_eq!(
            agent
                .call("capture.start", json!({"screen": "synthetic-0"}))
                .unwrap_err()
                .code,
            INVALID_PARAMS
        );
        assert_eq!(
            agent.call("capture.start", json!("x")).unwrap_err().code,
            INVALID_PARAMS
        );
        assert_eq!(
            agent.call("no.such.method", json!({})).unwrap_err().code,
            METHOD_NOT_FOUND
        );
        // A refusal of a restart while capturing ends that capture, not the agent
        agent
            .call("capture.start", start_params("synthetic-0"))
            .unwrap();
        agent
            .call("capture.start", start_params("nowhere"))
            .unwrap_err();
        assert_eq!(agent.status().state, CaptureState::Off);
    }

    #[test]
    fn screens_are_listed_without_a_capture() {
        let mut agent = synthetic();
        let listed = agent.call("screens.list", json!({})).unwrap();
        assert_eq!(listed["screens"][0]["id"], "synthetic-0");
        assert!(!agent.capturing());
        assert_eq!(agent.status().state, CaptureState::Off);
    }

    #[test]
    fn flags_and_bars_of_the_pictures_reach_the_status() {
        let picture = |flags: FrameFlags| Picture {
            screen_index: 0,
            width: 16,
            height: 9,
            flags,
            bars: Bars {
                top: 1,
                bottom: 1,
                left: 0,
                right: 0,
            },
            pixels: vec![7; 16 * 9 * 3],
        };
        let frames: Vec<ScreenFrame> = [
            FrameFlags::default(),
            FrameFlags {
                protected: true,
                hdr: true,
                held: false,
            },
            FrameFlags {
                protected: false,
                hdr: false,
                held: true,
            },
        ]
        .into_iter()
        .zip(0..)
        .map(|(flags, sequence)| ScreenFrame {
            sequence,
            timestamp_us: u64::from(sequence) * 1000,
            picture: picture(flags),
        })
        .collect();
        let file =
            std::env::temp_dir().join(format!("folo-capture-worker-{}.folr", std::process::id()));
        std::fs::write(&file, replay::write(&frames).unwrap()).unwrap();
        let backend = ReplayBackend::open(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        let mut agent = Agent::new(Box::new(backend), Box::new(ProcessClock::new()));
        agent.call("capture.start", start_params("replay")).unwrap();
        // The pacer keeps the newest picture of those that come together, so the
        // status is watched at every step: the replay loops through its frames
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            agent.step(Duration::from_millis(5));
            seen.push(agent.status().clone());
        }
        assert!(seen.iter().any(|s| s.state == CaptureState::Capturing
            && !s.hdr
            && !s.protected
            && (s.bars.top - 1.0 / 9.0).abs() < 1e-9));
        assert!(seen.iter().any(|s| s.hdr && s.protected));
        assert!(seen.iter().any(|s| s.state == CaptureState::Paused));
    }

    #[test]
    fn a_paused_backend_pauses_the_capture_and_its_repeats_are_held() {
        let (mut agent, script, time) = scripted();
        script.lock().unwrap().pictures.push_back(picture());
        let first = decoded(&agent.step(Duration::ZERO).expect("the first picture"));
        assert!(!first.picture.flags.held);
        assert_eq!(
            first.timestamp_us, 5_000_000,
            "stamped on the agent's clock"
        );
        assert_eq!(agent.status().state, CaptureState::Capturing);

        // The secure desktop: no pictures, and the backend says why
        script.lock().unwrap().status = BackendStatus {
            paused: true,
            message: Some("the secure desktop is shown".to_owned()),
            settings_uri: None,
        };
        time.store(5_100_000, Ordering::SeqCst);
        assert!(
            agent.step(Duration::ZERO).is_none(),
            "no repeat before 0.25 s"
        );
        assert_eq!(agent.status().state, CaptureState::Paused);
        assert_eq!(
            agent.status().message.as_deref(),
            Some("the secure desktop is shown")
        );
        time.store(5_260_000, Ordering::SeqCst);
        let repeat = decoded(&agent.step(Duration::ZERO).expect("a repeat after 0.25 s"));
        assert!(repeat.picture.flags.held, "a repeat while paused is held");
        assert_eq!(repeat.picture.pixels, first.picture.pixels);
        assert_eq!(repeat.sequence, first.sequence.wrapping_add(1));
        assert_eq!(repeat.timestamp_us, 5_260_000);

        // Capturing again: the new picture isn't held, and the message is gone
        {
            let mut script = script.lock().unwrap();
            script.status = BackendStatus::default();
            script.pictures.push_back(picture());
        }
        time.store(5_300_000, Ordering::SeqCst);
        let back = decoded(&agent.step(Duration::ZERO).expect("the new picture"));
        assert!(!back.picture.flags.held);
        assert_eq!(back.timestamp_us, 5_300_000);
        assert_eq!(agent.status().state, CaptureState::Capturing);
        assert_eq!(agent.status().message, None);
    }

    #[test]
    fn a_capture_the_privacy_setting_denies_is_paused_with_the_settings_page() {
        let (mut agent, script, time) = scripted();
        assert!(agent.step(Duration::ZERO).is_none());
        assert_eq!(
            agent.status().state,
            CaptureState::Starting,
            "starting until a picture comes"
        );
        script.lock().unwrap().status = BackendStatus {
            paused: true,
            message: Some("Windows' privacy setting denies screen capture".to_owned()),
            settings_uri: Some("ms-settings:privacy-graphicscaptureprogrammatic".to_owned()),
        };
        for second in 1..=3 {
            time.store(5_000_000 + second * 1_000_000, Ordering::SeqCst);
            assert!(
                agent.step(Duration::ZERO).is_none(),
                "no picture came, so none is repeated"
            );
            assert_eq!(agent.status().state, CaptureState::Paused);
        }
        let sent = serde_json::to_value(agent.status()).unwrap();
        assert_eq!(sent["state"], "paused");
        assert_eq!(
            sent["settings_uri"],
            "ms-settings:privacy-graphicscaptureprogrammatic"
        );
        assert_eq!(
            sent["message"],
            "Windows' privacy setting denies screen capture"
        );
        // A stop leaves nothing of it
        agent.stop();
        assert_eq!(agent.status(), &CaptureStatus::off());
    }
}
