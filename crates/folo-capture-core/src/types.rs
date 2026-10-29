//! The agent's vocabulary: screens, capture requests, its status, and the
//! flags, bars and pictures of its frames. The JSON shapes are those of
//! specs/schema/source-link.json and api.json (a test checks them).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CaptureError;

/// The widest frame the agent sends, in cells.
pub const MAX_WIDTH: u16 = 480;
/// The highest frame the agent sends, in cells.
pub const MAX_HEIGHT: u16 = 270;
/// The most frames a second a capture may ask for.
pub const MAX_RATE_HZ: f64 = 60.0;
/// The longest screen id (`Screen.id`), in characters as the schema counts them.
pub const MAX_SCREEN_ID_CHARS: usize = 128;
/// The longest restore token.
pub const MAX_RESTORE_TOKEN_CHARS: usize = 256;

/// A screen the agent can capture (api.json's `Screen`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub id: String,
    pub name: String,
    /// In pixels; 0 while unknown (the Linux portal entry).
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// The answer to `screens.list` (source-link.json's `ScreensListResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreensListResult {
    pub screens: Vec<Screen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlackBarPreset {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "21:9")]
    Cinema,
    #[serde(rename = "4:3")]
    Classic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlackBarMode {
    Default,
    Letterbox,
    Osd,
}

/// `black_bars` of `capture.start`: how the bar detector looks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlackBarSettings {
    pub preset: BlackBarPreset,
    pub mode: BlackBarMode,
    /// A share of full scale, 0.01 to 0.5.
    pub threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Video,
    Game,
}

/// `capture.start`'s params (source-link.json's `CaptureStartParams`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    pub screen: String,
    /// The box the frames fit, in cells: 1 to 480 by 1 to 270.
    pub width: u16,
    pub height: u16,
    pub rate_hz: f64,
    pub black_bars: BlackBarSettings,
    pub profile: Profile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_token: Option<String>,
}

impl CaptureRequest {
    /// Checks the limits the capture link's schema sets.
    ///
    /// # Errors
    ///
    /// [`CaptureError::InvalidRequest`] naming the first limit broken.
    pub fn validate(&self) -> Result<(), CaptureError> {
        let bad = |why: &str| Err(CaptureError::InvalidRequest(why.to_owned()));
        if self.screen.chars().count() > MAX_SCREEN_ID_CHARS {
            return bad("screen id is longer than 128 characters");
        }
        let long_token = self
            .restore_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_RESTORE_TOKEN_CHARS);
        if long_token {
            return bad("restore_token is longer than 256 characters");
        }
        if !(1..=MAX_WIDTH).contains(&self.width) {
            return bad("width is not 1 to 480");
        }
        if !(1..=MAX_HEIGHT).contains(&self.height) {
            return bad("height is not 1 to 270");
        }
        if !(1.0..=MAX_RATE_HZ).contains(&self.rate_hz) {
            return bad("rate_hz is not 1 to 60");
        }
        if !(0.01..=0.5).contains(&self.black_bars.threshold) {
            return bad("black_bars.threshold is not 0.01 to 0.5");
        }
        Ok(())
    }
}

/// The agent's state (`capture.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Off,
    Starting,
    WaitingConsent,
    Capturing,
    Paused,
    Error,
}

/// Black bars as shares (0 to 1) of the frame, for `capture.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarFractions {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

/// `capture.status`'s params (source-link.json's `CaptureStatusParams`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub backend: String,
    pub screen: String,
    pub fps: f64,
    pub hdr: bool,
    pub protected: bool,
    pub bars: BarFractions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_uri: Option<String>,
}

impl CaptureStatus {
    /// The status of an agent that isn't capturing: state `off`, nothing set.
    #[must_use]
    pub fn off() -> Self {
        Self {
            state: CaptureState::Off,
            backend: String::new(),
            screen: String::new(),
            fps: 0.0,
            hdr: false,
            protected: false,
            bars: BarFractions::default(),
            restore_token: None,
            message: None,
            settings_uri: None,
        }
    }
}

/// The flags of a frame (bits 0 to 2 of its flags byte).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameFlags {
    /// Protected content is on screen.
    pub protected: bool,
    /// The desktop is HDR and the frame was tone-mapped.
    pub hdr: bool,
    /// The agent can't capture now and sends its last picture again.
    pub held: bool,
}

impl FrameFlags {
    const PROTECTED: u8 = 1;
    const HDR: u8 = 1 << 1;
    const HELD: u8 = 1 << 2;

    /// The flags of a byte, or `None` when it has bits beyond the three.
    #[must_use]
    pub fn from_bits(bits: u8) -> Option<Self> {
        if bits & !(Self::PROTECTED | Self::HDR | Self::HELD) != 0 {
            return None;
        }
        Some(Self {
            protected: bits & Self::PROTECTED != 0,
            hdr: bits & Self::HDR != 0,
            held: bits & Self::HELD != 0,
        })
    }

    #[must_use]
    pub fn bits(self) -> u8 {
        (u8::from(self.protected) * Self::PROTECTED)
            | (u8::from(self.hdr) * Self::HDR)
            | (u8::from(self.held) * Self::HELD)
    }
}

/// The black bars found, in cells, counted from each side.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bars {
    pub top: u16,
    pub bottom: u16,
    pub left: u16,
    pub right: u16,
}

impl Bars {
    /// Whether the bars leave at least one row and one column of a picture.
    #[must_use]
    pub fn fit(self, width: u16, height: u16) -> bool {
        u32::from(self.top) + u32::from(self.bottom) < u32::from(height)
            && u32::from(self.left) + u32::from(self.right) < u32::from(width)
    }

    /// The bars as shares of a picture, as `capture.status` reports them.
    #[must_use]
    pub fn fractions(self, width: u16, height: u16) -> BarFractions {
        let share = |cells: u16, of: u16| {
            if of == 0 {
                0.0
            } else {
                f64::from(cells) / f64::from(of)
            }
        };
        BarFractions {
            top: share(self.top, height),
            bottom: share(self.bottom, height),
            left: share(self.left, width),
            right: share(self.right, width),
        }
    }
}

/// What a backend delivers: a downscaled picture, before the pacer gives it
/// a sequence number and a time.
///
/// `Debug` prints the metadata and the length of the pixels, never the pixels:
/// raw frames reach no log or status (ADR-0037 D9), and a stray `{:?}` mustn't
/// change that.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    /// The screen's position in the agent's `screens.list` answer.
    pub screen_index: u16,
    pub width: u16,
    pub height: u16,
    pub flags: FrameFlags,
    pub bars: Bars,
    /// `width × height × 3` bytes of sRGB rgb8, row-major.
    pub pixels: Vec<u8>,
}

impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Picture")
            .field("screen_index", &self.screen_index)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("flags", &self.flags)
            .field("bars", &self.bars)
            .field("pixel_bytes", &self.pixels.len())
            .finish()
    }
}

/// A screen frame as it travels (docs/specs/source-link.md#screen-frames).
/// Its `Debug` hides the pixels, as [`Picture`]'s does.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenFrame {
    pub sequence: u32,
    /// Microseconds on the operating system's monotonic clock.
    pub timestamp_us: u64,
    pub picture: Picture,
}

impl fmt::Debug for ScreenFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenFrame")
            .field("sequence", &self.sequence)
            .field("timestamp_us", &self.timestamp_us)
            .field("picture", &self.picture)
            .finish()
    }
}
