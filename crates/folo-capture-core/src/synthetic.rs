//! The synthetic backend (ADR-0037 D22): one fake 1920 × 1080 screen showing
//! a deterministic test pattern, so the agent and the node are tested without
//! a real screen. Pictures depend only on the pattern, the frame number and
//! the size.

use std::thread;
use std::time::{Duration, Instant};

use crate::backend::CaptureBackend;
use crate::error::CaptureError;
use crate::sizing::fit_in_box;
use crate::types::{Bars, CaptureRequest, FrameFlags, Picture, Screen};

/// The id of the one screen.
pub const SCREEN_ID: &str = "synthetic-0";
const SCREEN_WIDTH: u32 = 1920;
const SCREEN_HEIGHT: u32 = 1080;

/// Colour bars, as white, yellow, cyan, green, magenta, red, blue and black.
const BARS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Red runs across and moves with the frame number, green runs down, blue follows the frame number.
    Gradient,
    /// Eight colour bars that don't move.
    ColorBars,
    /// The colour bars between black bars an eighth of the height deep, reported in the frame's bars.
    Letterbox,
}

#[derive(Debug)]
struct Running {
    width: u16,
    height: u16,
    interval: Duration,
    next_index: u32,
    next_due: Instant,
}

#[derive(Debug)]
pub struct SyntheticBackend {
    pattern: Pattern,
    /// Whether `next_picture` waits for the frame's time, as a screen would.
    paced: bool,
    running: Option<Running>,
}

impl SyntheticBackend {
    /// A backend that delivers pictures at the rate asked for.
    #[must_use]
    pub fn new(pattern: Pattern) -> Self {
        Self {
            pattern,
            paced: true,
            running: None,
        }
    }

    /// A backend that delivers the next picture at once, for tests.
    #[must_use]
    pub fn unpaced(pattern: Pattern) -> Self {
        Self {
            pattern,
            paced: false,
            running: None,
        }
    }

    /// The picture with this number, `width` × `height` cells.
    #[must_use]
    pub fn render(pattern: Pattern, index: u32, width: u16, height: u16) -> Picture {
        let (width, height) = (width.max(1), height.max(1));
        let bar_rows = if pattern == Pattern::Letterbox {
            height / 8
        } else {
            0
        };
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 3);
        for y in 0..height {
            for x in 0..width {
                let inside = y >= bar_rows && y < height - bar_rows;
                pixels.extend_from_slice(&if inside {
                    Self::pixel(pattern, index, x, y, width, height)
                } else {
                    [0, 0, 0]
                });
            }
        }
        Picture {
            screen_index: 0,
            width,
            height,
            flags: FrameFlags::default(),
            bars: Bars {
                top: bar_rows,
                bottom: bar_rows,
                left: 0,
                right: 0,
            },
            pixels,
        }
    }

    fn pixel(pattern: Pattern, index: u32, x: u16, y: u16, width: u16, height: u16) -> [u8; 3] {
        match pattern {
            Pattern::Gradient => {
                let across = u32::from(x) * 255 / u32::from(width.max(2) - 1);
                let down = u32::from(y) * 255 / u32::from(height.max(2) - 1);
                // Each is below 256 after the modulo.
                let byte = |value: u32| u8::try_from(value % 256).unwrap_or(u8::MAX);
                [
                    byte(across + index.wrapping_mul(4)),
                    byte(down),
                    byte(index.wrapping_mul(8)),
                ]
            }
            Pattern::ColorBars | Pattern::Letterbox => {
                BARS[usize::from(x) * BARS.len() / usize::from(width)]
            }
        }
    }
}

impl CaptureBackend for SyntheticBackend {
    fn name(&self) -> &'static str {
        "synthetic"
    }

    fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        Ok(vec![Screen {
            id: SCREEN_ID.to_owned(),
            name: "Synthetic screen".to_owned(),
            width: SCREEN_WIDTH,
            height: SCREEN_HEIGHT,
            primary: true,
        }])
    }

    fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError> {
        request.validate()?;
        if request.screen != SCREEN_ID {
            return Err(CaptureError::UnknownScreen(request.screen.clone()));
        }
        let (width, height) =
            fit_in_box(SCREEN_WIDTH, SCREEN_HEIGHT, request.width, request.height);
        let interval = Duration::from_secs_f64(1.0 / request.rate_hz);
        // Asked again, the capture goes on from its frame number.
        let next_index = self.running.as_ref().map_or(0, |r| r.next_index);
        self.running = Some(Running {
            width,
            height,
            interval,
            next_index,
            next_due: Instant::now(),
        });
        Ok(())
    }

    fn next_picture(&mut self, timeout: Duration) -> Result<Option<Picture>, CaptureError> {
        let (pattern, paced) = (self.pattern, self.paced);
        let running = self.running.as_mut().ok_or(CaptureError::NotStarted)?;
        if paced {
            let wait = running.next_due.saturating_duration_since(Instant::now());
            if wait > timeout {
                thread::sleep(timeout);
                return Ok(None);
            }
            thread::sleep(wait);
            running.next_due = Instant::now() + running.interval;
        }
        let picture = Self::render(pattern, running.next_index, running.width, running.height);
        running.next_index = running.next_index.wrapping_add(1);
        Ok(Some(picture))
    }

    fn stop(&mut self) {
        self.running = None;
    }
}
