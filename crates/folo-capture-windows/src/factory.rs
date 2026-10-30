//! The Windows grabbers behind [`GrabberFactory`]: the monitors of the
//! desktop, and the three ways of capturing one.

use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{CaptureRequest, Screen};

use crate::com;
use crate::dda::DdaGrabber;
use crate::engine::{Engine, Grabber, GrabberFactory, SystemTimer};
use crate::fault::Fault;
use crate::gdi::GdiGrabber;
use crate::monitors;
use crate::recovery::{BackendKind, Policy};
use crate::wgc::WgcGrabber;

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsFactory;

impl GrabberFactory for WindowsFactory {
    fn screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        com::ensure_initialized();
        let monitors =
            monitors::enumerate().map_err(|fault| CaptureError::Failed(fault.to_string()))?;
        Ok(monitors.into_iter().map(|monitor| monitor.screen).collect())
    }

    fn open(
        &mut self,
        kind: BackendKind,
        screen_id: &str,
        request: &CaptureRequest,
    ) -> Result<Box<dyn Grabber>, Fault> {
        com::ensure_initialized();
        let monitor = monitors::find(screen_id)?;
        Ok(match kind {
            BackendKind::Dda => Box::new(DdaGrabber::open(&monitor, request)?),
            BackendKind::Wgc => Box::new(WgcGrabber::open(&monitor, request)?),
            BackendKind::Gdi => Box::new(GdiGrabber::open(&monitor, request)?),
        })
    }
}

/// The Windows backend: it tries `order` for each screen it captures, and recovers as
/// [`crate::recovery::Recovery`] says.
pub type WindowsBackend = Engine<WindowsFactory, SystemTimer>;

/// A backend that tries `order` (never empty). `--backend auto` passes
/// [`BackendKind::auto_order`], and a chosen backend passes only its own.
///
/// # Panics
///
/// When `order` is empty: that is a bug of the caller.
#[must_use]
pub fn backend(order: Vec<BackendKind>) -> WindowsBackend {
    Engine::new(WindowsFactory, SystemTimer::new(), order, Policy::default())
}
