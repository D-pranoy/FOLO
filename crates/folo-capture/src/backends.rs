//! Choosing the backend (`--backend`, ADR-0037 D20) and opening it.

use std::path::PathBuf;
use std::str::FromStr;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::clock::{Clock, ProcessClock};
use folo_capture_core::error::CaptureError;
use folo_capture_core::replay::ReplayBackend;
use folo_capture_core::synthetic::{Pattern, SyntheticBackend};
use folo_capture_windows::recovery::BackendKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendChoice {
    /// The platform's best: Desktop Duplication, then Windows.Graphics.Capture, then GDI; the portal on Linux.
    Auto,
    /// DXGI Desktop Duplication.
    Dda,
    /// Windows.Graphics.Capture.
    Wgc,
    Gdi,
    /// The `xdg-desktop-portal` `ScreenCast` interface.
    Portal,
    Synthetic,
    Replay(PathBuf),
}

impl FromStr for BackendChoice {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        if let Some(path) = text.strip_prefix("replay:") {
            return if path.is_empty() {
                Err("replay: needs a file, as replay:<file>".to_owned())
            } else {
                Ok(Self::Replay(PathBuf::from(path)))
            };
        }
        match text {
            "auto" => Ok(Self::Auto),
            "dda" => Ok(Self::Dda),
            "wgc" => Ok(Self::Wgc),
            "gdi" => Ok(Self::Gdi),
            "portal" => Ok(Self::Portal),
            "synthetic" => Ok(Self::Synthetic),
            _ => Err(format!(
                "{text:?} is not auto, dda, wgc, gdi, portal, synthetic or replay:<file>"
            )),
        }
    }
}

impl BackendChoice {
    /// Opens the backend.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Unavailable`] for a backend that isn't built for this
    /// platform (the Windows ones on other systems, the Linux one until its
    /// crate arrives), and [`CaptureError::Failed`] for a replay file that
    /// can't be used.
    pub fn open(&self) -> Result<Box<dyn CaptureBackend>, CaptureError> {
        match self {
            Self::Auto => Self::open_platform(),
            Self::Dda => Self::open_windows(&[BackendKind::Dda]),
            Self::Wgc => Self::open_windows(&[BackendKind::Wgc]),
            Self::Gdi => Self::open_windows(&[BackendKind::Gdi]),
            Self::Portal => Self::not_built("portal"),
            Self::Synthetic => Ok(Box::new(SyntheticBackend::new(Pattern::Gradient))),
            Self::Replay(path) => Ok(Box::new(ReplayBackend::open(path)?)),
        }
    }

    /// The clock that stamps this backend's frames (ADR-0037 D6): the
    /// operating system's monotonic counter for the real backends, and the
    /// process clock of the synthetic and replay ones.
    #[must_use]
    pub fn clock(&self) -> Box<dyn Clock> {
        #[cfg(windows)]
        if matches!(self, Self::Auto | Self::Dda | Self::Wgc | Self::Gdi) {
            return Box::new(folo_capture_windows::clock::QpcClock::new());
        }
        Box::new(ProcessClock::new())
    }

    fn not_built(name: &str) -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Err(CaptureError::Unavailable(format!(
            "the {name} backend isn't built yet; use --backend synthetic or --backend replay:<file>"
        )))
    }

    /// `auto`: Desktop Duplication, then Windows.Graphics.Capture, then GDI on Windows. The
    /// Linux backend has its own crate, which comes later.
    fn open_platform() -> Result<Box<dyn CaptureBackend>, CaptureError> {
        if cfg!(windows) {
            Self::open_windows(&BackendKind::auto_order())
        } else {
            Self::not_built("platform")
        }
    }

    #[cfg(windows)]
    #[allow(clippy::unnecessary_wraps)] // the other platform's version can fail
    fn open_windows(order: &[BackendKind]) -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Ok(Box::new(folo_capture_windows::factory::backend(
            order.to_vec(),
        )))
    }

    #[cfg(not(windows))]
    fn open_windows(order: &[BackendKind]) -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Self::not_built(order.first().map_or("windows", |kind| kind.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backend_of_the_command_line_parses() {
        for (text, choice) in [
            ("auto", BackendChoice::Auto),
            ("dda", BackendChoice::Dda),
            ("wgc", BackendChoice::Wgc),
            ("gdi", BackendChoice::Gdi),
            ("portal", BackendChoice::Portal),
            ("synthetic", BackendChoice::Synthetic),
            (
                "replay:a b/c.folr",
                BackendChoice::Replay(PathBuf::from("a b/c.folr")),
            ),
        ] {
            assert_eq!(text.parse::<BackendChoice>().unwrap(), choice);
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for text in ["", "Auto", "replay", "replay:", "synthetic:x", "dxgi"] {
            assert!(text.parse::<BackendChoice>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn the_unbuilt_backends_say_so_and_the_others_open() {
        // Opening a Windows backend touches nothing of the system: it captures on `start`.
        let windows = cfg!(windows);
        for (choice, built) in [
            (BackendChoice::Auto, windows),
            (BackendChoice::Dda, windows),
            (BackendChoice::Wgc, windows),
            (BackendChoice::Gdi, windows),
            (BackendChoice::Portal, false),
        ] {
            let opened = choice.open();
            if built {
                assert!(opened.is_ok(), "{choice:?}");
            } else {
                assert!(
                    matches!(opened, Err(CaptureError::Unavailable(_))),
                    "{choice:?}"
                );
            }
        }
        assert_eq!(BackendChoice::Synthetic.open().unwrap().name(), "synthetic");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_backend_names_itself_and_auto_starts_with_desktop_duplication() {
        for (choice, name) in [
            (BackendChoice::Auto, "dda"),
            (BackendChoice::Dda, "dda"),
            (BackendChoice::Wgc, "wgc"),
            (BackendChoice::Gdi, "gdi"),
        ] {
            assert_eq!(choice.open().unwrap().name(), name, "{choice:?}");
        }
    }

    #[test]
    fn the_clock_runs_forward_for_every_backend() {
        for choice in [
            BackendChoice::Auto,
            BackendChoice::Dda,
            BackendChoice::Synthetic,
        ] {
            let clock = choice.clock();
            let first = clock.now_us();
            std::thread::sleep(std::time::Duration::from_millis(20));
            assert!(clock.now_us() >= first + 15_000, "{choice:?}");
        }
    }
}
