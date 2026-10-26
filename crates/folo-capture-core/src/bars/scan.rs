//! One frame's border, found the way Hyperion.NG's `BlackBorderDetector` does
//! (the default, letterbox and OSD modes), on rgb8 pixels.
//!
//! A port of Hyperion.NG (Copyright (c) 2014-2026 Hyperion Project, MIT; the
//! notice is in `THIRD_PARTY_NOTICES`, ADR-0023 D4).
//!
//! The scans look at a few lines only: the first pixel of each, from the
//! frame's edge inwards, that isn't black ends it. The cell counts the scans
//! return are how many cells from the edge that pixel is.

use crate::types::BlackBarMode;

/// A frame to scan: `width × height × 3` bytes of rgb8, row-major.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    width: usize,
    height: usize,
    pixels: &'a [u8],
}

impl<'a> Image<'a> {
    /// The image, or `None` when it has no cells or `pixels` isn't
    /// `width × height × 3` bytes.
    #[must_use]
    pub fn new(width: u16, height: u16, pixels: &'a [u8]) -> Option<Self> {
        let (width, height) = (usize::from(width), usize::from(height));
        (width > 0 && height > 0 && pixels.len() == width * height * 3).then_some(Self {
            width,
            height,
            pixels,
        })
    }
}

/// What one frame says (Hyperion.NG's `BlackBorder`): the rows and columns
/// of black from the top-left corner, or that they are unknown, as in a black
/// scene where no line reached a pixel that isn't black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Border {
    pub unknown: bool,
    /// Rows of black at the top and bottom (Hyperion.NG's `horizontalSize`).
    pub rows: usize,
    /// Columns of black at the left and right (`verticalSize`).
    pub columns: usize,
}

impl Border {
    /// Nothing is known; no bars.
    pub const UNKNOWN: Self = Self {
        unknown: true,
        rows: 0,
        columns: 0,
    };

    /// Whether this is the same border as `other`: unknown matches unknown
    /// whatever the sizes, as in Hyperion.NG's `operator==`.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        if self.unknown {
            return other.unknown;
        }
        !other.unknown && self.rows == other.rows && self.columns == other.columns
    }
}

/// How far in from each edge the scans start. A game's HUD sits at the edge
/// and would otherwise end a scan at once; with a margin its cells aren't
/// looked at (display capture, profiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeSkip {
    pub columns: usize,
    pub rows: usize,
}

/// The sampling points of one scan, in cells, from the frame's size.
struct Lines {
    width: usize,
    height: usize,
    skip: EdgeSkip,
}

/// The scanner: the threshold turned into a byte, as Hyperion.NG's
/// `calculateThreshold` does, and the edge margin.
#[derive(Debug, Clone, Copy)]
pub struct Scanner {
    threshold: u8,
    skip_share_percent: usize,
}

impl Scanner {
    /// A scanner for `threshold` (a share of full scale) that leaves out
    /// `skip_share_percent` of the frame at each edge.
    #[must_use]
    pub fn new(threshold: f64, skip_share_percent: usize) -> Self {
        Self {
            threshold: threshold_byte(threshold),
            skip_share_percent,
        }
    }

    /// The border of one frame in `mode`.
    #[must_use]
    pub fn scan(&self, mode: BlackBarMode, image: &Image<'_>) -> Border {
        let lines = Lines {
            width: image.width,
            height: image.height,
            skip: self.skip(image),
        };
        let (rows, columns) = match mode {
            BlackBarMode::Default => self.default_mode(image, &lines),
            BlackBarMode::Letterbox => (self.letterbox(image, &lines), Some(0)),
            BlackBarMode::Osd => self.osd(image, &lines),
        };
        Border {
            unknown: rows.is_none() || columns.is_none(),
            rows: rows.unwrap_or(0),
            columns: columns.unwrap_or(0),
        }
    }

    /// The margin, at most a third of the side so a scan always has cells.
    fn skip(&self, image: &Image<'_>) -> EdgeSkip {
        let share = |side: usize| (side * self.skip_share_percent / 100).min(side / 3);
        EdgeSkip {
            columns: share(image.width),
            rows: share(image.height),
        }
    }

    /// 3 lines on 4 sides: the left and top at a third and two thirds, the
    /// right and bottom through the centre (`process`).
    fn default_mode(&self, image: &Image<'_>, lines: &Lines) -> (Option<usize>, Option<usize>) {
        let (width, height) = (lines.width, lines.height);
        let (width33, height33) = (width / 3, height / 3);
        let (width66, height66) = (width33 * 2, height33 * 2);
        let (x_center, y_center) = (width / 2, height / 2);
        let (last_x, last_y) = (width - 1, height - 1);

        let column = (lines.skip.columns..width33).find(|&x| {
            !self.black(image, last_x - x, y_center)
                || !self.black(image, x, height33)
                || !self.black(image, x, height66)
        });
        let row = (lines.skip.rows..height33).find(|&y| {
            !self.black(image, x_center, last_y - y)
                || !self.black(image, width33, y)
                || !self.black(image, width66, y)
        });
        (
            row.map(|y| edge(y, lines.skip.rows)),
            column.map(|x| edge(x, lines.skip.columns)),
        )
    }

    /// The columns first, then the rows at that column, so an overlay
    /// (an on-screen display) in the bars doesn't count (`process_osd`).
    fn osd(&self, image: &Image<'_>, lines: &Lines) -> (Option<usize>, Option<usize>) {
        let (width, height) = (lines.width, lines.height);
        let (width33, height33) = (width / 3, height / 3);
        let height66 = height33 * 2;
        let y_center = height / 2;
        let (last_x, last_y) = (width - 1, height - 1);

        let column = (lines.skip.columns..width33).find(|&x| {
            !self.black(image, last_x - x, y_center)
                || !self.black(image, x, height33)
                || !self.black(image, x, height66)
        });
        // Hyperion.NG leaves its loop counter at the end when nothing was
        // found, and the row scan runs at that column.
        let at = column.unwrap_or(width33.max(lines.skip.columns));
        let row = (lines.skip.rows..height33).find(|&y| {
            !self.black(image, at, y)
                || !self.black(image, at, last_y - y)
                || !self.black(image, last_x - at, y)
                || !self.black(image, last_x - at, last_y - y)
        });
        (
            row.map(|y| edge(y, lines.skip.rows)),
            column.map(|x| edge(x, lines.skip.columns)),
        )
    }

    /// 5 lines, top and bottom only: the centre only at the top, so captions
    /// at the bottom of the picture don't count (`process_letterbox`).
    fn letterbox(&self, image: &Image<'_>, lines: &Lines) -> Option<usize> {
        let (width, height) = (lines.width, lines.height);
        let height33 = height / 3;
        let width25 = width / 4;
        let width75 = width25 * 3;
        let x_center = width / 2;
        let last_y = height - 1;

        (lines.skip.rows..height33)
            .find(|&y| {
                !self.black(image, x_center, y)
                    || !self.black(image, width25, y)
                    || !self.black(image, width75, y)
                    || !self.black(image, width25, last_y - y)
                    || !self.black(image, width75, last_y - y)
            })
            .map(|y| edge(y, lines.skip.rows))
    }

    fn black(&self, image: &Image<'_>, x: usize, y: usize) -> bool {
        let at = (y * image.width + x) * 3;
        image.pixels[at..at + 3]
            .iter()
            .all(|&channel| channel < self.threshold)
    }
}

/// A scan that ended at its first cell found nothing of a bar; one that
/// started past an edge margin counts the margin as bar.
fn edge(found: usize, start: usize) -> usize {
    if found == start { 0 } else { found }
}

/// Hyperion.NG's `calculateThreshold`: `ceil(share × 255)`, clamped to a byte.
fn threshold_byte(threshold: f64) -> u8 {
    // Clamped to 0 to 255 first, so the cast can't truncate or change sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = (threshold * 255.0).ceil().clamp(0.0, 255.0) as u8;
    byte
}
