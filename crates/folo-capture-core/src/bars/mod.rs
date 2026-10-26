//! The black-bar detector (RB-14, ADR-0037 D7): finds the black rows and
//! columns around a downscaled screen frame, for the frame's header and for
//! `capture.status`. The Screen effect crops them.
//!
//! This is a port of Hyperion.NG's black-border detector
//! (`libsrc/blackborder`, `include/blackborder`) to safe Rust, adapted to
//! count time where it counts frames. Hyperion.NG is
//! Copyright (c) 2014-2026 Hyperion Project, MIT-licensed; its notice is in
//! `THIRD_PARTY_NOTICES` at the root of the repository
//! (ADR-0023 D4).
//!
//! - [`scan`]: one frame's border in the default, letterbox and OSD modes, and
//!   the threshold (`BlackBorderDetector`)
//! - [`hysteresis`]: when a border becomes the one in force, in time
//!   (`BlackBorderProcessor::updateBorder`)
//! - [`BarDetector`]: those two with the blur margin, the presets and the
//!   profiles, as docs/integrations/display-capture.md describes
//!
//! Unlike Hyperion.NG, which tracks the one border its top-left scan finds,
//! this reports the same rows at the top and bottom and the same columns at
//! the left and right, which is what that border means.

pub mod hysteresis;
pub mod scan;

use hysteresis::{Hysteresis, Timing};
use scan::{Border, Image, Scanner};

use crate::types::{Bars, BlackBarPreset, BlackBarSettings, Picture, Profile};

/// Cells added to a border found, to leave out what the downscale blurred
/// into the picture (Hyperion.NG's `blurRemoveCnt`, 1).
pub const BLUR_MARGIN: usize = 1;

/// The share of each edge a game's HUD may take, which the `game` profile
/// doesn't look at, in percent.
pub const GAME_EDGE_PERCENT: usize = 5;

/// The shape of the picture the `21:9` preset forces, as width over height.
const CINEMA_ASPECT: (u32, u32) = (21, 9);
/// The shape of the picture the `4:3` preset forces.
const CLASSIC_ASPECT: (u32, u32) = (4, 3);

/// Finds the bars of a stream of frames.
#[derive(Debug, Clone)]
pub struct BarDetector {
    settings: BlackBarSettings,
    profile: Profile,
    hysteresis: Hysteresis,
    /// The size of the frames the state is about; bars are cells, so a frame
    /// of another size starts over.
    size: Option<(u16, u16)>,
    bars: Bars,
}

impl BarDetector {
    #[must_use]
    pub fn new(settings: BlackBarSettings, profile: Profile) -> Self {
        Self {
            settings,
            profile,
            hysteresis: Hysteresis::new(Timing::of(profile)),
            size: None,
            bars: Bars::default(),
        }
    }

    /// Applies new settings in place (`capture.start` sent again, D3). What
    /// the detector has seen is kept, except when the threshold or the mode
    /// changed: a border found one way says nothing of the other.
    pub fn reconfigure(&mut self, settings: BlackBarSettings, profile: Profile) {
        let looks_the_same = settings.mode == self.settings.mode
            && (settings.threshold - self.settings.threshold).abs() <= f64::EPSILON;
        self.settings = settings;
        self.profile = profile;
        if looks_the_same {
            self.hysteresis.set_timing(Timing::of(profile));
        } else {
            self.hysteresis = Hysteresis::new(Timing::of(profile));
        }
    }

    /// The bars in force.
    #[must_use]
    pub fn bars(&self) -> Bars {
        self.bars
    }

    /// Looks at a frame seen at `now_us` (microseconds on a monotonic clock)
    /// and returns the bars in force. A frame that isn't `width × height × 3`
    /// bytes, or has no cells, changes nothing.
    pub fn update(&mut self, width: u16, height: u16, pixels: &[u8], now_us: u64) -> Bars {
        if self.size != Some((width, height)) {
            self.hysteresis = Hysteresis::new(Timing::of(self.profile));
            self.size = Some((width, height));
        }
        self.bars = match self.settings.preset {
            BlackBarPreset::Off => Bars::default(),
            BlackBarPreset::Cinema => forced(width, height, CINEMA_ASPECT),
            BlackBarPreset::Classic => forced(width, height, CLASSIC_ASPECT),
            BlackBarPreset::Auto => self.detect(width, height, pixels, now_us),
        };
        self.bars
    }

    /// Sets the bars of a picture, except for one the agent isn't showing
    /// (`held` or `protected`, D13), whose pixels say nothing: it takes the
    /// bars in force.
    pub fn annotate(&mut self, picture: &mut Picture, now_us: u64) {
        if picture.flags.held || picture.flags.protected {
            picture.bars = self.bars;
            return;
        }
        picture.bars = self.update(picture.width, picture.height, &picture.pixels, now_us);
    }

    fn detect(&mut self, width: u16, height: u16, pixels: &[u8], now_us: u64) -> Bars {
        let Some(image) = Image::new(width, height, pixels) else {
            return self.bars;
        };
        let skip = match self.profile {
            Profile::Video => 0,
            Profile::Game => GAME_EDGE_PERCENT,
        };
        let scanner = Scanner::new(self.settings.threshold, skip);
        let seen = with_blur(scanner.scan(self.settings.mode, &image));
        self.hysteresis.update(seen, now_us);
        to_bars(self.hysteresis.current(), width, height)
    }
}

/// Hyperion.NG adds the blur margin to each size that isn't 0.
fn with_blur(mut border: Border) -> Border {
    if border.rows > 0 {
        border.rows += BLUR_MARGIN;
    }
    if border.columns > 0 {
        border.columns += BLUR_MARGIN;
    }
    border
}

/// The border as bars on both sides, leaving at least a row and a column.
fn to_bars(border: Border, width: u16, height: u16) -> Bars {
    if border.unknown {
        return Bars::default();
    }
    let side = |cells: usize, of: u16| {
        let most = usize::from(of.saturating_sub(1)) / 2;
        // At most `of / 2`, so it fits.
        u16::try_from(cells.min(most)).unwrap_or(u16::MAX)
    };
    let (rows, columns) = (side(border.rows, height), side(border.columns, width));
    Bars {
        top: rows,
        bottom: rows,
        left: columns,
        right: columns,
    }
}

/// The bars of a picture of `aspect` (width, height) shown in a frame:
/// rows above and below when the frame is wider, columns when it is taller.
fn forced(width: u16, height: u16, aspect: (u32, u32)) -> Bars {
    let (frame_w, frame_h) = (u32::from(width), u32::from(height));
    let (aspect_w, aspect_h) = aspect;
    let mut border = Border {
        unknown: false,
        rows: 0,
        columns: 0,
    };
    if frame_w * aspect_h > frame_h * aspect_w {
        let picture_w = (frame_h * aspect_w + aspect_h / 2) / aspect_h;
        border.columns = usize::try_from((frame_w - picture_w) / 2).unwrap_or(0);
    } else {
        let picture_h = (frame_w * aspect_h + aspect_w / 2) / aspect_w;
        border.rows = usize::try_from((frame_h - picture_h) / 2).unwrap_or(0);
    }
    to_bars(border, width, height)
}
