//! What the backend's threads share: the capture's state, the newest picture
//! and the numbers the agent reports. A mutex and a condition variable carry
//! it; the lock is held for a few assignments, never across a wait on the
//! portal or PipeWire.

use std::fmt;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use folo_capture_core::backend::BackendReport;
use folo_capture_core::types::{CaptureState, Picture};

/// What the node last asked for, as the downscale needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub box_width: u16,
    pub box_height: u16,
    pub rate_hz: f64,
}

struct Inner {
    state: CaptureState,
    message: Option<String>,
    restore_token: Option<String>,
    screen_size: (u32, u32),
    picture: Option<Picture>,
    target: Target,
    frames: u32,
    window_start: Instant,
}

/// What a wait for the next picture ended with.
pub enum Wait {
    Picture(Picture),
    Nothing,
    Failed(String),
}

pub struct Shared {
    inner: Mutex<Inner>,
    arrived: Condvar,
}

// The state and the numbers only: never the restore token or the pixels, so
// that a stray `{:?}` can't put them in a log.
impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.lock();
        f.debug_struct("Shared")
            .field("state", &inner.state)
            .field("message", &inner.message)
            .field("screen_size", &inner.screen_size)
            .field("has_picture", &inner.picture.is_some())
            .field("target", &inner.target)
            .finish_non_exhaustive()
    }
}

impl Shared {
    pub fn new(target: Target) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: CaptureState::Off,
                message: None,
                restore_token: None,
                screen_size: (0, 0),
                picture: None,
                target,
                frames: 0,
                window_start: Instant::now(),
            }),
            arrived: Condvar::new(),
        }
    }

    // A thread that panicked while holding the lock left plain values behind.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets the last capture's picture, size and failure; the restore
    /// token it left stays until it is reported.
    pub fn reset(&self, target: Target) {
        let mut inner = self.lock();
        inner.state = CaptureState::Starting;
        inner.message = None;
        inner.screen_size = (0, 0);
        inner.picture = None;
        inner.target = target;
        inner.frames = 0;
        inner.window_start = Instant::now();
    }

    pub fn set_state(&self, state: CaptureState) {
        let mut inner = self.lock();
        inner.state = state;
        drop(inner);
        self.arrived.notify_all();
    }

    /// Records a failure: state `error` with the reason.
    pub fn fail(&self, message: String) {
        let mut inner = self.lock();
        inner.state = CaptureState::Error;
        inner.message = Some(message);
        drop(inner);
        self.arrived.notify_all();
    }

    /// A warning that doesn't stop the capture.
    pub fn warn(&self, message: String) {
        self.lock().message = Some(message);
    }

    pub fn set_restore_token(&self, token: String) {
        self.lock().restore_token = Some(token);
    }

    pub fn set_screen_size(&self, size: (u32, u32)) {
        self.lock().screen_size = size;
    }

    pub fn screen_size(&self) -> (u32, u32) {
        self.lock().screen_size
    }

    pub fn target(&self) -> Target {
        self.lock().target
    }

    pub fn set_target(&self, target: Target) {
        self.lock().target = target;
    }

    pub fn state(&self) -> CaptureState {
        self.lock().state
    }

    /// Takes a new picture in; of two the reader hasn't taken, the newer stays.
    pub fn publish(&self, picture: Picture) {
        let mut inner = self.lock();
        inner.picture = Some(picture);
        inner.frames = inner.frames.saturating_add(1);
        drop(inner);
        self.arrived.notify_all();
    }

    /// Waits up to `timeout` for a picture. A picture that is there wins over
    /// a failure, so the last one isn't lost.
    pub fn wait_for_picture(&self, timeout: Duration) -> Wait {
        let guard = self.lock();
        let (mut inner, _) = self
            .arrived
            .wait_timeout_while(guard, timeout, |inner| {
                inner.picture.is_none() && inner.state != CaptureState::Error
            })
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(picture) = inner.picture.take() {
            return Wait::Picture(picture);
        }
        if inner.state == CaptureState::Error {
            let why = inner
                .message
                .clone()
                .unwrap_or_else(|| "the capture failed".to_owned());
            return Wait::Failed(why);
        }
        Wait::Nothing
    }

    /// The state and what changed since the last report. The restore token is
    /// handed over once.
    pub fn report(&self) -> BackendReport {
        let mut inner = self.lock();
        let elapsed = inner.window_start.elapsed().as_secs_f64();
        let fps = if elapsed > 0.0 {
            f64::from(inner.frames) / elapsed
        } else {
            0.0
        };
        // A window shorter than this would make the rate jump around.
        if elapsed >= 0.5 {
            inner.frames = 0;
            inner.window_start = Instant::now();
        }
        BackendReport {
            state: inner.state,
            message: inner.message.clone(),
            restore_token: inner.restore_token.take(),
            fps,
        }
    }
}
