//! The Linux capture backend: the `CaptureBackend` for the desktop portal's
//! ScreenCast interface and PipeWire (ADR-0037 D15 to D17).

use std::sync::Arc;
use std::time::Duration;

use folo_capture_core::backend::{BackendReport, CaptureBackend};
use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{CaptureRequest, CaptureState, Picture, Screen};

use crate::session::{self, PortalSettings, Worker};
use crate::shared::{Shared, Target, Wait};

/// The id of the one screen: the portal names none before the user chooses.
pub const SCREEN_ID: &str = "portal";

/// The name `capture.status` reports as `backend`.
pub const BACKEND_NAME: &str = "portal";

const SCREEN_NAME: &str = "Screen chosen in the desktop's dialog";

pub struct PortalBackend {
    settings: PortalSettings,
    shared: Arc<Shared>,
    worker: Option<Worker>,
}

impl PortalBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::with_settings(PortalSettings::default())
    }

    #[must_use]
    pub fn with_settings(settings: PortalSettings) -> Self {
        let target = Target {
            box_width: folo_capture_core::types::MAX_WIDTH,
            box_height: folo_capture_core::types::MAX_HEIGHT,
            rate_hz: 30.0,
        };
        Self {
            settings,
            shared: Arc::new(Shared::new(target)),
            worker: None,
        }
    }

    fn target_of(request: &CaptureRequest) -> Target {
        Target {
            box_width: request.width,
            box_height: request.height,
            rate_hz: request.rate_hz,
        }
    }

    /// Whether a capture is running or waiting for the user.
    fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
}

impl Default for PortalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for PortalBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        // Listing never talks to the portal, so it never shows a dialog (D15).
        let (width, height) = self.shared.screen_size();
        Ok(vec![Screen {
            id: SCREEN_ID.to_owned(),
            name: SCREEN_NAME.to_owned(),
            width,
            height,
            primary: true,
        }])
    }

    fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError> {
        request.validate()?;
        if request.screen != SCREEN_ID {
            return Err(CaptureError::UnknownScreen(request.screen.clone()));
        }
        let target = Self::target_of(request);
        if self.is_running() {
            // Size and rate apply in place, with no new session or consent (D3).
            self.shared.set_target(target);
            return Ok(());
        }
        // A capture that ended on its own is cleaned up before the next one.
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        self.shared.reset(target);
        let worker = session::spawn(
            self.settings.clone(),
            Arc::clone(&self.shared),
            request.restore_token.clone(),
        )
        .map_err(CaptureError::Failed)?;
        self.worker = Some(worker);
        Ok(())
    }

    fn next_picture(&mut self, timeout: Duration) -> Result<Option<Picture>, CaptureError> {
        if self.worker.is_none() {
            return Err(CaptureError::NotStarted);
        }
        match self.shared.wait_for_picture(timeout) {
            Wait::Picture(picture) => Ok(Some(picture)),
            Wait::Nothing => Ok(None),
            Wait::Failed(why) => Err(CaptureError::Failed(why)),
        }
    }

    fn stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        self.shared.set_state(CaptureState::Off);
    }

    fn report(&mut self) -> Option<BackendReport> {
        Some(self.shared.report())
    }
}

impl Drop for PortalBackend {
    fn drop(&mut self) {
        self.stop();
    }
}
