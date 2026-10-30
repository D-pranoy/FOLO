//! The interface every capture backend implements (ADR-0037 D20): the
//! Windows and Linux backends, and the synthetic and replay ones.

use std::time::Duration;

use crate::error::CaptureError;
use crate::types::{CaptureRequest, Picture, Screen};

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
}
