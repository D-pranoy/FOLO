//! The interface every capture backend implements (ADR-0037 D20): the
//! Windows and Linux backends, and the synthetic and replay ones.

use std::fmt;
use std::time::Duration;

use crate::error::CaptureError;
use crate::types::{CaptureRequest, CaptureState, Picture, Screen};

/// What a backend knows about its capture beyond the pictures, for the
/// agent's `capture.status` (ADR-0037 D3, D14).
#[derive(Clone, PartialEq)]
pub struct BackendReport {
    pub state: CaptureState,
    /// Why the state is `error`, or a warning that doesn't stop the capture.
    pub message: Option<String>,
    /// A restore token the backend got since the last report. It is handed
    /// over once: the agent passes it on in `capture.status` and the node
    /// stores it (D14).
    pub restore_token: Option<String>,
    /// Pictures a second, over the last second or so.
    pub fps: f64,
}

// The restore token stays out of any log line or panic message, as the launch
// token does in `Environment`.
impl fmt::Debug for BackendReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendReport")
            .field("state", &self.state)
            .field("message", &self.message)
            .field(
                "restore_token",
                &self.restore_token.as_ref().map(|_| "<hidden>"),
            )
            .field("fps", &self.fps)
            .finish()
    }
}

/// A source of downscaled pictures of one screen. One capture at a time.
pub trait CaptureBackend: Send {
    /// The name `capture.status` reports as `backend` (`dda`, `wgc`, `gdi`,
    /// `portal`, `synthetic`, `replay`).
    fn name(&self) -> &'static str;

    /// The screens it can capture. It captures nothing and asks for no consent.
    ///
    /// # Errors
    ///
    /// [`CaptureError`] when the operating system can't list them.
    fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError>;

    /// Starts capturing `request.screen`. Called again while capturing, it
    /// applies `black_bars`, `profile`, `rate_hz` and the size in place; a
    /// different screen ends the session and starts another.
    ///
    /// # Errors
    ///
    /// [`CaptureError::UnknownScreen`] for an id the listing doesn't have,
    /// [`CaptureError::InvalidRequest`] for a request outside the link's
    /// limits, and others when the operating system refuses.
    fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError>;

    /// The next picture, waiting up to `timeout` for it; `None` when none came
    /// (nothing on the screen changed, or the backend is paused).
    ///
    /// # Errors
    ///
    /// [`CaptureError::NotStarted`] without a started capture, and others when
    /// the capture failed.
    fn next_picture(&mut self, timeout: Duration) -> Result<Option<Picture>, CaptureError>;

    /// Ends the capture. It does nothing when none is started.
    fn stop(&mut self);

    /// The state of the capture and what changed since the last call, for
    /// backends that have more to say than whether pictures come (the portal
    /// waits for consent and returns restore tokens). `None` when the backend
    /// has nothing to add: the agent then derives the state from the pictures.
    fn report(&mut self) -> Option<BackendReport> {
        None
    }
}
