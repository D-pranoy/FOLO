//! Gather plans: for each element of a target, which cells of a surface it reads and how much.

use folo_protocol::generated::{Surface, SurfaceKind};

use crate::kernel::ElementTaps;

/// The most a canvas is published at, per axis ([config model](../../../docs/specs/config-model.md)).
pub const CANVAS_PUBLISHED_MAX: (usize, usize) = (128, 72);

/// The size of the frames a surface is delivered at: a strip's width by 1, and a canvas's size cut
/// to [`CANVAS_PUBLISHED_MAX`] on each axis (so a bigger canvas changes aspect ratio). It is the
/// same rule as the supervisor's, and plans are built over it: a surface's cells, for manual ranges
/// too, are the cells of the frames the engine sends.
#[must_use]
pub fn published_size(surface: &Surface) -> (usize, usize) {
    let width = usize::try_from(surface.resolution.width.get()).unwrap_or(usize::MAX);
    let height = usize::try_from(surface.resolution.height.get()).unwrap_or(usize::MAX);
    match surface.kind {
        SurfaceKind::Canvas => (
            width.min(CANVAS_PUBLISHED_MAX.0),
            height.min(CANVAS_PUBLISHED_MAX.1),
        ),
        SurfaceKind::Strip => (width, 1),
    }
}

/// The reads of one mapping: for each of a target's elements, a few cells of a surface and a weight
/// for each. Built when the configuration changes, so a frame is one gather and weighted sum.
///
/// The taps of every element sum to 1. An element no tap reaches is *uncovered*: the mapping leaves
/// it alone, and it stays black unless another mapping covers it.
#[derive(Clone, Debug)]
pub struct GatherPlan {
    width: usize,
    height: usize,
    /// Element `e`'s taps are `cell[start[e]..start[e + 1]]` (and the same of `weight`).
    start: Vec<u32>,
    cell: Vec<u32>,
    weight: Vec<f32>,
    /// How much of its kernel was on the surface, per element (1 unless the edge policy is `none`).
    alpha: Vec<f32>,
}

impl GatherPlan {
    pub(crate) fn new(width: usize, height: usize, elements: Vec<Option<ElementTaps>>) -> Self {
        let mut plan = Self {
            width,
            height,
            start: Vec::with_capacity(elements.len() + 1),
            cell: Vec::new(),
            weight: Vec::new(),
            alpha: Vec::with_capacity(elements.len()),
        };
        plan.start.push(0);
        for element in elements {
            if let Some(e) = element {
                for (cell, weight) in e.taps {
                    plan.cell.push(cell);
                    plan.weight.push(weight);
                }
                plan.alpha.push(e.alpha);
            } else {
                plan.alpha.push(0.0);
            }
            plan.start.push(plan.cell.len() as u32);
        }
        plan
    }

    /// The number of elements the plan is for.
    #[must_use]
    pub fn elements(&self) -> usize {
        self.alpha.len()
    }

    /// The `(width, height)` of the surface the plan reads. A frame of another size can't use it.
    #[must_use]
    pub fn surface_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Element `e`'s taps: cell indices (row-major, `y * width + x`) and their weights.
    /// Empty when the element is uncovered.
    #[must_use]
    pub fn taps(&self, e: usize) -> (&[u32], &[f32]) {
        let (from, to) = (self.start[e] as usize, self.start[e + 1] as usize);
        (&self.cell[from..to], &self.weight[from..to])
    }

    /// How much of element `e`'s kernel was on the surface: 1, or less at an edge with the `none` policy.
    #[must_use]
    pub fn alpha(&self, e: usize) -> f32 {
        self.alpha[e]
    }

    #[must_use]
    pub fn is_covered(&self, e: usize) -> bool {
        self.start[e + 1] > self.start[e]
    }

    /// The most taps any element has.
    #[must_use]
    pub fn max_taps(&self) -> usize {
        self.start
            .windows(2)
            .map(|w| (w[1] - w[0]) as usize)
            .max()
            .unwrap_or(0)
    }

    /// The weighted sum of the cells element `e` reads, or `None` when it is uncovered.
    ///
    /// `pixels` is the surface, row-major, and must have `width × height` entries.
    #[must_use]
    pub(crate) fn gather(&self, e: usize, pixels: &[[f32; 3]]) -> Option<[f32; 3]> {
        let (cells, weights) = self.taps(e);
        if cells.is_empty() {
            return None;
        }
        let mut sum = [0.0_f32; 3];
        let first = pixels[cells[0] as usize];
        let (mut lo, mut hi) = (first, first);
        for (&cell, &w) in cells.iter().zip(weights) {
            let px = pixels[cell as usize];
            for ch in 0..3 {
                sum[ch] += w * px[ch];
                lo[ch] = lo[ch].min(px[ch]);
                hi[ch] = hi[ch].max(px[ch]);
            }
        }
        // The weights sum to 1 only up to f32 rounding, so a flat surface could come out an ulp
        // off. A weighted mean can't leave the range of what it averages: clamp to it.
        for ch in 0..3 {
            sum[ch] = sum[ch].clamp(lo[ch], hi[ch]);
        }
        Some(sum)
    }
}
