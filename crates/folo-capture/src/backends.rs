//! Choosing the backend (`--backend`, ADR-0037 D20) and opening it.

use std::path::PathBuf;
use std::str::FromStr;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::error::CaptureError;
use folo_capture_core::replay::ReplayBackend;
use folo_capture_core::synthetic::{Pattern, SyntheticBackend};

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
    /// [`CaptureError::Unavailable`] for a backend that isn't built yet (the
    /// Windows ones arrive with their crate) or isn't for this platform, and
    /// [`CaptureError::Failed`] for a replay file that can't be used.
    pub fn open(&self) -> Result<Box<dyn CaptureBackend>, CaptureError> {
        let not_built = |name: &str| {
            Err(CaptureError::Unavailable(format!(
                "the {name} backend isn't built yet; use --backend synthetic or --backend replay:<file>"
            )))
        };
        match self {
            Self::Auto => Self::open_platform_default(),
            Self::Dda => not_built("dda"),
            Self::Wgc => not_built("wgc"),
            Self::Gdi => not_built("gdi"),
            Self::Portal => Self::open_portal(),
            Self::Synthetic => Ok(Box::new(SyntheticBackend::new(Pattern::Gradient))),
            Self::Replay(path) => Ok(Box::new(ReplayBackend::open(path)?)),
        }
    }
}

impl BackendChoice {
    /// The portal on Linux. The Windows crate isn't built yet, so there is no
    /// best backend to pick there.
    fn open_platform_default() -> Result<Box<dyn CaptureBackend>, CaptureError> {
        if cfg!(target_os = "linux") {
            Self::open_portal()
        } else {
            Err(CaptureError::Unavailable(
                "the platform's capture backend isn't built yet; use --backend synthetic or --backend replay:<file>"
                    .to_owned(),
            ))
        }
    }

    #[cfg(all(target_os = "linux", feature = "portal"))]
    #[allow(clippy::unnecessary_wraps)] // the other versions can fail
    fn open_portal() -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Ok(Box::new(folo_capture_linux::PortalBackend::new()))
    }

    #[cfg(all(target_os = "linux", not(feature = "portal")))]
    fn open_portal() -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Err(CaptureError::Unavailable(
            "the portal backend isn't built into this binary: it needs PipeWire 1.0's development files, then cargo's --features portal"
                .to_owned(),
        ))
    }

    #[cfg(not(target_os = "linux"))]
    fn open_portal() -> Result<Box<dyn CaptureBackend>, CaptureError> {
        Err(CaptureError::Unavailable(
            "the portal backend is for Linux".to_owned(),
        ))
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
    fn the_unbuilt_backends_say_so_and_the_test_backend_opens() {
        for choice in [BackendChoice::Dda, BackendChoice::Wgc, BackendChoice::Gdi] {
            assert!(
                matches!(choice.open(), Err(CaptureError::Unavailable(_))),
                "{choice:?}"
            );
        }
        assert_eq!(BackendChoice::Synthetic.open().unwrap().name(), "synthetic");
    }

    #[cfg(all(target_os = "linux", feature = "portal"))]
    #[test]
    fn the_portal_is_the_backend_on_linux_and_auto_picks_it() {
        for choice in [BackendChoice::Portal, BackendChoice::Auto] {
            assert_eq!(choice.open().unwrap().name(), "portal", "{choice:?}");
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "portal")))]
    #[test]
    fn without_the_portal_backend_it_and_auto_say_so() {
        for choice in [BackendChoice::Portal, BackendChoice::Auto] {
            assert!(
                matches!(choice.open(), Err(CaptureError::Unavailable(_))),
                "{choice:?}"
            );
        }
    }
}
