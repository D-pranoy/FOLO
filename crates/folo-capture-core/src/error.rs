//! The errors of the core: a frame that fails the codec's checks, and a
//! backend that can't do what it was asked.

use std::fmt;

/// Why a screen frame is refused. The messages name the field and never the
/// pixels (docs/specs/source-link.md#the-capture-link: logs hold no pixels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The message is longer than the link allows.
    MessageTooLong {
        length: usize,
    },
    /// The message is shorter than the header.
    TooShort {
        length: usize,
    },
    BadMagic,
    BadVersion(u8),
    BadPixelFormat(u8),
    /// Flag bits other than protected, hdr and held.
    UnknownFlags(u8),
    BadWidth(u16),
    BadHeight(u16),
    /// The message isn't `36 + 3 × width × height` bytes long.
    BadLength {
        expected: usize,
        actual: usize,
    },
    /// A black bar that leaves no row (or column) of the picture.
    BadBars,
    /// The pixels don't match the width and height of the frame to encode.
    BadPixels {
        expected: usize,
        actual: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageTooLong { length } => {
                write!(f, "frame message of {length} bytes is too long")
            }
            Self::TooShort { length } => write!(
                f,
                "frame message of {length} bytes is shorter than its header"
            ),
            Self::BadMagic => f.write_str("frame magic is not FOLC"),
            Self::BadVersion(v) => write!(f, "frame format version {v} is not 1"),
            Self::BadPixelFormat(p) => write!(f, "pixel format {p} is not rgb8 (1)"),
            Self::UnknownFlags(bits) => write!(f, "frame flags {bits:#04x} have unknown bits"),
            Self::BadWidth(w) => write!(f, "frame width {w} is not 1 to 480"),
            Self::BadHeight(h) => write!(f, "frame height {h} is not 1 to 270"),
            Self::BadLength { expected, actual } => {
                write!(f, "frame message is {actual} bytes, expected {expected}")
            }
            Self::BadBars => f.write_str("frame black bars leave no picture"),
            Self::BadPixels { expected, actual } => {
                write!(f, "frame has {actual} pixel bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// What a backend (or the choice of one) can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// No screen has this id (the JSON-RPC error `-32000`).
    UnknownScreen(String),
    /// The request breaks a limit of the capture link (`-32602`).
    InvalidRequest(String),
    /// `next_frame` or `stop` without a capture that was started.
    NotStarted,
    /// The backend isn't built into this binary, or not for this platform.
    Unavailable(String),
    /// The operating system or a file refused.
    Failed(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownScreen(id) => write!(f, "no screen has the id {id:?}"),
            Self::InvalidRequest(why) => write!(f, "invalid capture request: {why}"),
            Self::NotStarted => f.write_str("no capture is started"),
            Self::Unavailable(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<FrameError> for CaptureError {
    fn from(error: FrameError) -> Self {
        Self::Failed(error.to_string())
    }
}
