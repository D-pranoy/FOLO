//! The sampling kernels: from a sample position and size in a surface's cells to weighted taps.
//!
//! Every kernel is separable: it gives weights per cell along x and along y, and the taps are
//! their outer product. Cell `i` spans `[i, i + 1)` along its axis, so a coordinate `c` is in cell
//! `floor(c)` and cell `i`'s centre is `i + 0.5`.

use folo_protocol::generated::{KernelType, MappingEdge};

/// The most taps one element reads.
pub const MAX_TAPS: usize = 64;

/// Samples per axis of the grid an area box larger than [`MAX_TAPS`] cells is read on.
const SUB_GRID: usize = 8;

/// Boxes narrower than this (in cells) are read as points.
const MIN_BOX: f64 = 1e-9;

/// One element's reads of the surface: normalised weights and how much of the kernel was on it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ElementTaps {
    /// `(cell index, weight)`, weights summing to 1.
    pub taps: Vec<(u32, f32)>,
    /// The share of the kernel's weight inside the surface (1 with the clamp policy).
    pub alpha: f32,
}

/// The weights along one axis.
#[derive(Debug)]
struct Axis {
    /// `(cell, weight)` in ascending cell order, weights summing to 1.
    taps: Vec<(usize, f64)>,
    /// The share of the weight that fell inside `0..n` before outside taps were dropped.
    inside: f64,
}

/// Turns raw `(cell, weight)` pairs in ascending cell order into an axis over `n` cells.
///
/// `clamp` moves outside taps onto the nearest edge cell and merges them. `none` drops them and
/// renormalises what is left; `inside` says how much was kept.
fn resolve(raw: &[(i64, f64)], n: usize, edge: MappingEdge) -> Axis {
    let last = n as i64 - 1;
    let mut taps: Vec<(usize, f64)> = Vec::with_capacity(raw.len());
    let (mut total, mut kept) = (0.0, 0.0);
    for &(cell, w) in raw {
        total += w;
        let cell = match edge {
            MappingEdge::Clamp => cell.clamp(0, last),
            MappingEdge::None if (0..=last).contains(&cell) => cell,
            MappingEdge::None => continue,
        };
        kept += w;
        let cell = cell as usize;
        match taps.last_mut() {
            Some((c, acc)) if *c == cell => *acc += w,
            _ => taps.push((cell, w)),
        }
    }
    if kept <= 0.0 {
        return Axis {
            taps: Vec::new(),
            inside: 0.0,
        };
    }
    for (_, w) in &mut taps {
        *w /= kept;
    }
    Axis {
        taps,
        inside: kept / total,
    }
}

#[allow(clippy::float_cmp)] // exactly on the far edge is what is meant
fn nearest(c: f64, n: usize, edge: MappingEdge) -> Axis {
    // The surface's far edge belongs to its last cell, so a point on the end of a path is on it.
    let cell = if c == n as f64 {
        n as i64 - 1
    } else {
        c.floor() as i64
    };
    resolve(&[(cell, 1.0)], n, edge)
}

fn bilinear(c: f64, n: usize, edge: MappingEdge) -> Axis {
    let c = c - 0.5;
    let i0 = c.floor();
    let f = c - i0;
    let i0 = i0 as i64;
    let mut raw = vec![(i0, 1.0 - f)];
    if f > 0.0 {
        raw.push((i0 + 1, f));
    }
    resolve(&raw, n, edge)
}

/// The cells a box `[lo, hi]` touches.
fn cells_touched(lo: f64, hi: f64) -> usize {
    ((hi.ceil() - lo.floor()) as usize).max(1)
}

/// Exact overlap of the box with each cell, cells beyond the surface included.
fn area_exact(lo: f64, hi: f64, n: usize, edge: MappingEdge) -> Axis {
    let first = lo.floor() as i64;
    let last = hi.ceil() as i64 - 1;
    let raw: Vec<(i64, f64)> = (first..=last)
        .map(|i| (i, hi.min(i as f64 + 1.0) - lo.max(i as f64)))
        .filter(|&(_, w)| w > 0.0)
        .collect();
    resolve(&raw, n, edge)
}

/// `k` equally weighted samples at the centres of `k` equal parts of the box.
fn area_sampled(lo: f64, hi: f64, k: usize, n: usize, edge: MappingEdge) -> Axis {
    let step = (hi - lo) / k as f64;
    let raw: Vec<(i64, f64)> = (0..k)
        .map(|j| ((lo + (j as f64 + 0.5) * step).floor() as i64, 1.0))
        .collect();
    resolve(&raw, n, edge)
}

/// The taps of a kernel at `(cx, cy)` on a `width × height` surface.
///
/// `(hx, hy)` is the area box's half-size in cells and is ignored by the other kernels.
///
/// An area box covering more than [`MAX_TAPS`] cells is read at equally spaced sub-samples instead
/// of cell by cell: 8 × 8 of them, or 64 along the axis when the box is one cell across the other.
/// So an element never reads more than 64 cells.
///
/// With the `none` edge policy, taps outside the surface are dropped and the rest renormalised,
/// and `alpha` is the share that was inside. `None` when nothing was inside.
pub(crate) fn sample(
    kernel: KernelType,
    edge: MappingEdge,
    (cx, cy): (f64, f64),
    (hx, hy): (f64, f64),
    (width, height): (usize, usize),
) -> Option<ElementTaps> {
    if !(cx.is_finite() && cy.is_finite()) {
        return None;
    }
    let (ax, ay) = match kernel {
        KernelType::Nearest => (nearest(cx, width, edge), nearest(cy, height, edge)),
        KernelType::Bilinear => (bilinear(cx, width, edge), bilinear(cy, height, edge)),
        KernelType::Area if hx.max(hy) < MIN_BOX => {
            (nearest(cx, width, edge), nearest(cy, height, edge))
        }
        KernelType::Area => {
            // A zero-width side of the box is read as a point on that axis.
            let (hx, hy) = (hx.max(MIN_BOX), hy.max(MIN_BOX));
            let (xl, xh, yl, yh) = (cx - hx, cx + hx, cy - hy, cy + hy);
            let (nx, ny) = (cells_touched(xl, xh), cells_touched(yl, yh));
            if nx * ny <= MAX_TAPS {
                (
                    area_exact(xl, xh, width, edge),
                    area_exact(yl, yh, height, edge),
                )
            } else if ny == 1 {
                (
                    area_sampled(xl, xh, MAX_TAPS, width, edge),
                    area_exact(yl, yh, height, edge),
                )
            } else if nx == 1 {
                (
                    area_exact(xl, xh, width, edge),
                    area_sampled(yl, yh, MAX_TAPS, height, edge),
                )
            } else {
                (
                    area_sampled(xl, xh, SUB_GRID, width, edge),
                    area_sampled(yl, yh, SUB_GRID, height, edge),
                )
            }
        }
    };
    let alpha = ax.inside * ay.inside;
    if ax.taps.is_empty() || ay.taps.is_empty() || alpha <= 0.0 {
        return None;
    }
    let mut taps = Vec::with_capacity(ax.taps.len() * ay.taps.len());
    for &(y, wy) in &ay.taps {
        for &(x, wx) in &ax.taps {
            taps.push(((y * width + x) as u32, (wx * wy) as f32));
        }
    }
    Some(ElementTaps {
        taps,
        alpha: alpha as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use KernelType::{Area, Bilinear, Nearest};
    use MappingEdge::{Clamp, None as Drop};

    fn taps(t: Option<&ElementTaps>) -> Vec<(u32, f32)> {
        t.map(|t| t.taps.clone()).unwrap_or_default()
    }

    fn close(a: &[(u32, f32)], b: &[(u32, f32)]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() < 1e-6)
    }

    #[test]
    fn nearest_takes_the_cell_the_sample_is_in() {
        let t = sample(Nearest, Clamp, (2.9, 0.5), (0.0, 0.0), (4, 1));
        assert_eq!(taps(t.as_ref()), [(2, 1.0)]);
        // A coordinate exactly on a boundary belongs to the cell that starts there.
        let t = sample(Nearest, Clamp, (3.0, 0.5), (0.0, 0.0), (4, 1));
        assert_eq!(taps(t.as_ref()), [(3, 1.0)]);
    }

    #[test]
    fn bilinear_blends_between_cell_centres() {
        // Halfway between the centres of cells 1 and 2 is the boundary at 2.0.
        let t = sample(Bilinear, Clamp, (2.0, 0.5), (0.0, 0.0), (4, 1));
        assert!(close(&taps(t.as_ref()), &[(1, 0.5), (2, 0.5)]), "{t:?}");
        // On a cell's centre it is that cell alone.
        let t = sample(Bilinear, Clamp, (1.5, 0.5), (0.0, 0.0), (4, 1));
        assert_eq!(taps(t.as_ref()), [(1, 1.0)]);
        // A quarter of the way from centre 1 to centre 2.
        let t = sample(Bilinear, Clamp, (1.75, 0.5), (0.0, 0.0), (4, 1));
        assert!(close(&taps(t.as_ref()), &[(1, 0.75), (2, 0.25)]), "{t:?}");
    }

    #[test]
    fn bilinear_in_two_dimensions_is_the_outer_product() {
        let t = sample(Bilinear, Clamp, (1.0, 1.0), (0.0, 0.0), (3, 3));
        assert!(
            close(
                &taps(t.as_ref()),
                &[(0, 0.25), (1, 0.25), (3, 0.25), (4, 0.25)]
            ),
            "{t:?}"
        );
    }

    #[test]
    fn area_weights_are_exact_overlaps() {
        // A box [0.5, 2.5] over cells 0..3: half of cell 0, all of cell 1, half of cell 2.
        let t = sample(Area, Clamp, (1.5, 0.5), (1.0, 0.5), (4, 1));
        assert!(
            close(&taps(t.as_ref()), &[(0, 0.25), (1, 0.5), (2, 0.25)]),
            "{t:?}"
        );
        // A box inside one cell reads that cell alone.
        let t = sample(Area, Clamp, (1.5, 0.5), (0.25, 0.5), (4, 1));
        assert_eq!(taps(t.as_ref()), [(1, 1.0)]);
    }

    #[test]
    fn area_in_two_dimensions() {
        // A 2×2 box on the corner shared by four cells weighs each a quarter.
        let t = sample(Area, Clamp, (1.0, 1.0), (1.0, 1.0), (4, 4));
        assert!(
            close(
                &taps(t.as_ref()),
                &[(0, 0.25), (1, 0.25), (4, 0.25), (5, 0.25)]
            ),
            "{t:?}"
        );
        // A box [0.5, 3.5]: the two cells in the middle are whole and the outer ones a half.
        let t = sample(Area, Clamp, (2.0, 0.5), (1.5, 0.5), (4, 1));
        let (third, sixth) = (1.0 / 3.0, 1.0 / 6.0);
        assert!(
            close(
                &taps(t.as_ref()),
                &[(0, sixth), (1, third), (2, third), (3, sixth)]
            ),
            "{t:?}"
        );
    }

    #[test]
    fn a_box_of_one_cell_on_a_cell_is_one_tap_of_weight_one() {
        // radius 0.5 × a spacing of one cell, centred on the cell: the parity box.
        for i in 0..150u32 {
            let t = sample(Area, Clamp, (f64::from(i) + 0.5, 0.5), (0.5, 0.5), (150, 1)).unwrap();
            assert_eq!(t.taps, [(i, 1.0)]);
            assert_eq!(t.alpha, 1.0);
        }
    }

    #[test]
    fn area_never_reads_more_than_64_cells() {
        // 20×20 cells would be 400 taps exactly; the 8×8 sub-grid is read instead.
        let t = sample(Area, Clamp, (10.0, 10.0), (10.0, 10.0), (20, 20)).unwrap();
        assert!(t.taps.len() <= MAX_TAPS, "{}", t.taps.len());
        let sum: f32 = t.taps.iter().map(|t| t.1).sum();
        assert!((sum - 1.0).abs() < 1e-5, "{sum}");
        // A box 200 cells long on a strip is read at 64 samples along it.
        let t = sample(Area, Clamp, (500.0, 0.5), (100.0, 0.5), (1000, 1)).unwrap();
        assert_eq!(t.taps.len(), MAX_TAPS);
        // The first sample is in the middle of the first 1/64 of the box: 400 + 3.125 / 2.
        assert!(close(&t.taps[..1], &[(401, 1.0 / 64.0)]), "{:?}", t.taps[0]);
        // Exactly 64 cells is still read cell by cell.
        let t = sample(Area, Clamp, (32.0, 0.5), (32.0, 0.5), (64, 1)).unwrap();
        assert_eq!(t.taps.len(), 64);
        assert!(t.taps.iter().all(|t| (t.1 - 1.0 / 64.0).abs() < 1e-7));
    }

    #[test]
    fn a_sampled_box_has_one_tap_per_cell_it_falls_on() {
        // 16 × 16 cells read at 8 × 8 samples, each in a different cell: 1/64 each.
        let t = sample(Area, Clamp, (8.0, 8.0), (8.0, 8.0), (16, 16)).unwrap();
        assert_eq!(t.taps.len(), 64);
        assert!(t.taps.iter().all(|t| (t.1 - 1.0 / 64.0).abs() < 1e-7));
        // Samples that land in one cell share it: a box of 9 × 9 cells, over a surface 3 cells wide
        // in x (clamped), has fewer taps than samples and the same total weight.
        let t = sample(Area, Clamp, (1.5, 4.5), (4.5, 4.5), (3, 9)).unwrap();
        assert!(t.taps.len() < 64);
        let sum: f32 = t.taps.iter().map(|t| t.1).sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn clamp_moves_what_is_outside_onto_the_edge() {
        // A box [-1, 1]: the half left of the surface counts for cell 0, which has the other half.
        let t = sample(Area, Clamp, (0.0, 0.5), (1.0, 0.5), (4, 1)).unwrap();
        assert_eq!(t.taps, [(0, 1.0)]);
        // A box [-0.5, 1.5]: half a cell outside plus all of cell 0, then half of cell 1.
        let t = sample(Area, Clamp, (0.5, 0.5), (1.0, 0.5), (4, 1)).unwrap();
        assert!(close(&t.taps, &[(0, 0.75), (1, 0.25)]), "{t:?}");
        assert_eq!(t.alpha, 1.0);
        let t = sample(Nearest, Clamp, (-5.0, 0.5), (0.0, 0.0), (4, 1)).unwrap();
        assert_eq!(t.taps, [(0, 1.0)]);
        let t = sample(Nearest, Clamp, (9.0, 0.5), (0.0, 0.0), (4, 1)).unwrap();
        assert_eq!(t.taps, [(3, 1.0)]);
    }

    #[test]
    fn none_drops_what_is_outside_and_renormalises() {
        // A box [-1, 1] on four cells: half is outside, cell 0 gets all of the rest.
        let t = sample(Area, Drop, (0.0, 0.5), (1.0, 0.5), (4, 1)).unwrap();
        assert_eq!(t.taps, [(0, 1.0)]);
        assert!((t.alpha - 0.5).abs() < 1e-6, "{}", t.alpha);
        // A box [0.5, 2.5] is wholly inside.
        let t = sample(Area, Drop, (1.5, 0.5), (1.0, 0.5), (4, 1)).unwrap();
        assert_eq!(t.alpha, 1.0);
        // Nothing inside: no taps.
        assert_eq!(sample(Nearest, Drop, (-0.5, 0.5), (0.0, 0.0), (4, 1)), None);
        assert_eq!(sample(Area, Drop, (-3.0, 0.5), (1.0, 0.5), (4, 1)), None);
        // Bilinear at the left edge keeps the inside cell and the share inside.
        let t = sample(Bilinear, Drop, (0.0, 0.5), (0.0, 0.0), (4, 1)).unwrap();
        assert_eq!(t.taps, [(0, 1.0)]);
        assert!((t.alpha - 0.5).abs() < 1e-6, "{}", t.alpha);
    }

    #[test]
    fn a_zero_radius_area_is_a_point_sample() {
        let t = sample(Area, Clamp, (2.4, 0.5), (0.0, 0.0), (4, 1));
        assert_eq!(taps(t.as_ref()), [(2, 1.0)]);
    }

    #[test]
    fn weights_always_sum_to_one() {
        for kernel in [Nearest, Bilinear, Area] {
            for edge in [Clamp, Drop] {
                for k in 0..200 {
                    let c = -3.0 + f64::from(k) * 0.093;
                    if let Some(t) = sample(kernel, edge, (c, c * 0.7), (1.3, 2.1), (12, 9)) {
                        let sum: f32 = t.taps.iter().map(|t| t.1).sum();
                        assert!((sum - 1.0).abs() < 1e-5, "{kernel:?} {edge:?} {c}: {sum}");
                        assert!(t.alpha > 0.0 && t.alpha <= 1.0);
                        assert!(t.taps.len() <= MAX_TAPS);
                        assert!(t.taps.iter().all(|t| (t.0 as usize) < 12 * 9));
                    }
                }
            }
        }
    }
}
