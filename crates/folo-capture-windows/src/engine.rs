//! The Windows backend as a [`CaptureBackend`]: it opens the grabbers of a
//! [`GrabberFactory`] in the order of the chosen backend, and applies the
//! [`Recovery`] state machine to what they report (ADR-0037 D13). It holds no
//! operating-system objects, so the factory and the timer are replaced in
//! tests, and the recovery paths are tested with injected faults.

use std::thread;
use std::time::{Duration, Instant};

use folo_capture_core::backend::{BackendStatus, CaptureBackend};
use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{CaptureRequest, Picture, Screen};

use crate::fault::Fault;
use crate::recovery::{BackendKind, Policy, Recovery, Step};

/// One way of capturing one screen, opened by a [`GrabberFactory`]. It is dropped to close it.
pub trait Grabber: Send {
    /// The next picture, waiting up to `timeout`; `None` when the screen didn't change.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the capture failed. The grabber is dropped after it.
    fn grab(&mut self, timeout: Duration) -> Result<Option<Picture>, Fault>;

    /// Applies the request's size and rate in place.
    fn apply(&mut self, request: &CaptureRequest);
}

/// The operating system's side: the screens, and the grabbers that capture them.
pub trait GrabberFactory: Send {
    /// The screens now connected, in the order `screens.list` answers them.
    ///
    /// # Errors
    ///
    /// [`CaptureError`] when the operating system can't list them.
    fn screens(&mut self) -> Result<Vec<Screen>, CaptureError>;

    /// Opens `kind` for the screen with this id, looking the screen up again,
    /// because a mode change moves screens around.
    ///
    /// # Errors
    ///
    /// A [`Fault`]; [`Fault::ScreenGone`] when no screen has the id.
    fn open(
        &mut self,
        kind: BackendKind,
        screen_id: &str,
        request: &CaptureRequest,
    ) -> Result<Box<dyn Grabber>, Fault>;
}

/// The clock and the waiting of the backend, replaced in tests.
pub trait Timer: Send {
    fn now_us(&self) -> u64;
    fn sleep(&self, duration: Duration);
}

/// Time as the process sees it.
#[derive(Debug, Clone, Copy)]
pub struct SystemTimer {
    start: Instant,
}

impl SystemTimer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for SystemTimer {
    fn now_us(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

enum Active {
    Grabbing(Box<dyn Grabber>),
    /// Recovering: open again at this time.
    Waiting {
        retry_at_us: u64,
    },
    /// Nothing is left to try; every call says why.
    Failed(String),
}

struct Session {
    screen_id: String,
    screen_index: u16,
    request: CaptureRequest,
    recovery: Recovery,
    active: Active,
    status: BackendStatus,
}

/// A [`CaptureBackend`] over a factory of grabbers.
pub struct Engine<F: GrabberFactory, T: Timer> {
    factory: F,
    timer: T,
    order: Vec<BackendKind>,
    policy: Policy,
    session: Option<Session>,
}

impl<F: GrabberFactory, T: Timer> Engine<F, T> {
    /// A backend that tries `order` (never empty) for each screen it captures.
    ///
    /// # Panics
    ///
    /// When `order` is empty: that is a bug of the caller.
    #[must_use]
    pub fn new(factory: F, timer: T, order: Vec<BackendKind>, policy: Policy) -> Self {
        assert!(
            !order.is_empty(),
            "a backend order needs at least one backend"
        );
        Self {
            factory,
            timer,
            order,
            policy,
            session: None,
        }
    }

    /// Opens the session's current backend, following the state machine
    /// through switches, until a grabber is open, a retry is due, or nothing
    /// is left.
    fn open_current(&mut self, session: &mut Session) -> Result<(), CaptureError> {
        loop {
            let kind = session.recovery.current();
            match self
                .factory
                .open(kind, &session.screen_id, &session.request)
            {
                Ok(grabber) => {
                    session.active = Active::Grabbing(grabber);
                    // The pause (and its message) stays until the first picture: a reopen
                    // that fails again at once must not flap to "capturing".
                    if let Ok(screens) = self.factory.screens() {
                        session.screen_index = screens
                            .iter()
                            .position(|screen| screen.id == session.screen_id)
                            .map_or(session.screen_index, |index| {
                                u16::try_from(index).unwrap_or(u16::MAX)
                            });
                    }
                    return Ok(());
                }
                Err(fault) => {
                    if self.after_fault(session, &fault)? == Next::Wait {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Applies the state machine's step for `fault`.
    fn after_fault(&self, session: &mut Session, fault: &Fault) -> Result<Next, CaptureError> {
        match session.recovery.on_fault(fault) {
            Step::Retry { after, paused } => {
                let after_us = u64::try_from(after.as_micros()).unwrap_or(u64::MAX);
                session.active = Active::Waiting {
                    retry_at_us: self.timer.now_us().saturating_add(after_us),
                };
                session.status = Self::status_for(fault, paused);
                Ok(Next::Wait)
            }
            Step::Switch(_) => Ok(Next::Open),
            Step::GiveUp(why) => {
                session.active = Active::Failed(why.clone());
                Err(CaptureError::Failed(why))
            }
        }
    }

    fn status_for(fault: &Fault, paused: bool) -> BackendStatus {
        BackendStatus {
            paused,
            message: Some(fault.to_string()),
            settings_uri: (*fault == Fault::PrivacyDenied).then(|| PRIVACY_SETTINGS_URI.to_owned()),
        }
    }

    fn wait_for_retry(&self, retry_at_us: u64, timeout: Duration) -> bool {
        let now = self.timer.now_us();
        let remaining = Duration::from_micros(retry_at_us.saturating_sub(now));
        if remaining.is_zero() {
            return true;
        }
        self.timer.sleep(remaining.min(timeout));
        self.timer.now_us() >= retry_at_us
    }
}

/// What follows a fault.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Try the (new) current backend now.
    Open,
    /// A retry is scheduled.
    Wait,
}

/// The page of Windows' settings that holds "Screenshots and screen recording" (graphics capture); `capture.status`'s `settings_uri`.
pub const PRIVACY_SETTINGS_URI: &str = "ms-settings:privacy-graphicscaptureprogrammatic";

impl<F: GrabberFactory, T: Timer> CaptureBackend for Engine<F, T> {
    fn name(&self) -> &'static str {
        self.session
            .as_ref()
            .map_or(self.order[0], |session| session.recovery.current())
            .name()
    }

    fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        self.factory.screens()
    }

    fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError> {
        request.validate()?;
        let screens = self.factory.screens()?;
        let index = screens
            .iter()
            .position(|screen| screen.id == request.screen)
            .ok_or_else(|| CaptureError::UnknownScreen(request.screen.clone()))?;
        let failed = self
            .session
            .as_ref()
            .is_some_and(|session| matches!(session.active, Active::Failed(_)));
        if failed {
            // The same `capture.start` again is a new try, not the old error.
            self.stop();
        }
        if let Some(session) = self
            .session
            .as_mut()
            .filter(|session| session.screen_id == request.screen)
        {
            request.clone_into(&mut session.request);
            if let Active::Grabbing(grabber) = &mut session.active {
                grabber.apply(request);
            }
            return Ok(());
        }
        self.stop();
        let mut session = Session {
            screen_id: request.screen.clone(),
            // A listing holds at most a few dozen screens.
            screen_index: u16::try_from(index).unwrap_or(u16::MAX),
            request: request.clone(),
            recovery: Recovery::new(self.order.clone(), self.policy),
            active: Active::Waiting { retry_at_us: 0 },
            status: BackendStatus::default(),
        };
        self.open_current(&mut session)?;
        self.session = Some(session);
        Ok(())
    }

    fn next_picture(&mut self, timeout: Duration) -> Result<Option<Picture>, CaptureError> {
        let mut session = self.session.take().ok_or(CaptureError::NotStarted)?;
        let result = self.step(&mut session, timeout);
        self.session = Some(session);
        result
    }

    fn stop(&mut self) {
        self.session = None;
    }

    fn status(&self) -> BackendStatus {
        self.session
            .as_ref()
            .map(|session| session.status.clone())
            .unwrap_or_default()
    }
}

impl<F: GrabberFactory, T: Timer> Engine<F, T> {
    fn step(
        &mut self,
        session: &mut Session,
        timeout: Duration,
    ) -> Result<Option<Picture>, CaptureError> {
        match &mut session.active {
            Active::Failed(why) => Err(CaptureError::Failed(why.clone())),
            Active::Waiting { retry_at_us } => {
                if !self.wait_for_retry(*retry_at_us, timeout) {
                    return Ok(None);
                }
                self.open_current(session)?;
                Ok(None)
            }
            Active::Grabbing(grabber) => match grabber.grab(timeout) {
                Ok(Some(mut picture)) => {
                    session.recovery.on_picture();
                    session.status = BackendStatus::default();
                    picture.screen_index = session.screen_index;
                    Ok(Some(picture))
                }
                Ok(None) => Ok(None),
                Err(fault) => {
                    // The grabber is dropped (closed) before anything is opened again.
                    session.active = Active::Waiting { retry_at_us: 0 };
                    if self.after_fault(session, &fault)? == Next::Open {
                        self.open_current(session)?;
                    }
                    Ok(None)
                }
            },
        }
    }
}
