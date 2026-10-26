//! Building a gather plan from a mapping: the sample position of each element, then the kernel.

use folo_protocol::generated::{Mapping, MappingMethod, Placement, Surface};

use crate::kernel::{self, ElementTaps};
use crate::layout::Layout;
use crate::math;
use crate::plan::GatherPlan;
use crate::validate::usable_path;

/// The plan of `mapping` over `surface` for a target with this `layout`, or `None` when the mapping
/// can't work (see [`crate::validate`]) or reaches nothing.
///
/// - **Manual:** each range reads the surface as one row of `width × height` cells, in order.
///   Element `j` of the range samples at `(j + 0.5) × cells / elements` (from the far end when
///   reversed). Equal counts read one cell each with weight 1, whatever the kernel.
/// - **Path:** the element's position goes to the nearest point of the surface's path, and its
///   arc-length fraction times the width is the sample.
/// - **Plane:** the element's position is projected orthographically onto the surface's plane, and
///   its `(u, v)` times the size is the sample.
///
/// An area kernel's box is `radius` × the element's spacing on each side of the sample. Where the
/// spacing is unknown (a point, or coincident elements) it is one cell.
pub(crate) fn build(mapping: &Mapping, surface: &Surface, layout: &Layout) -> Option<GatherPlan> {
    let width = usize::try_from(surface.resolution.width.get()).ok()?;
    let height = usize::try_from(surface.resolution.height.get()).ok()?;
    let elements = match mapping.method {
        MappingMethod::Manual => manual(mapping, width * height, layout.len()),
        MappingMethod::Path => path(mapping, surface, layout, (width, height))?,
        MappingMethod::Plane => plane(mapping, surface, layout, (width, height))?,
    };
    Some(GatherPlan::new(width, height, elements))
}

fn radius(mapping: &Mapping) -> f64 {
    if mapping.kernel.radius.is_finite() {
        mapping.kernel.radius.max(0.0)
    } else {
        0.0
    }
}

fn manual(mapping: &Mapping, cells: usize, elements: usize) -> Vec<Option<ElementTaps>> {
    let mut out = vec![None; elements];
    for r in &mapping.ranges {
        let cell_start = usize::from(r.cells.start);
        let cell_count = usize::try_from(r.cells.count.get()).unwrap_or(usize::MAX);
        let start = usize::from(r.elements.start);
        let count = usize::try_from(r.elements.count.get()).unwrap_or(usize::MAX);
        if cell_start.saturating_add(cell_count) > cells || start.saturating_add(count) > elements {
            continue;
        }
        let per_element = cell_count as f64 / count as f64;
        for j in 0..count {
            out[start + j] = if cell_count == count {
                let cell = if r.reversed {
                    cell_start + cell_count - 1 - j
                } else {
                    cell_start + j
                };
                Some(ElementTaps {
                    taps: vec![(cell as u32, 1.0)],
                    alpha: 1.0,
                })
            } else {
                let along = (j as f64 + 0.5) * per_element;
                let at = cell_start as f64
                    + if r.reversed {
                        cell_count as f64 - along
                    } else {
                        along
                    };
                // The cells are one row, so the box is `radius` cell-spacings either side of `at`.
                kernel::sample(
                    mapping.kernel.type_,
                    mapping.edge,
                    (at, 0.5),
                    (radius(mapping) * per_element, 0.5),
                    (cells, 1),
                )
            };
        }
    }
    out
}

fn path(
    mapping: &Mapping,
    surface: &Surface,
    layout: &Layout,
    (width, height): (usize, usize),
) -> Option<Vec<Option<ElementTaps>>> {
    let Placement::PathPlacement(p) = &surface.placement else {
        return None;
    };
    let line = usable_path(p.points.iter().map(math::from_proto))?;
    if !layout.is_placed() {
        return None;
    }
    let total = line.length();
    let cell_len = total / width as f64;
    let radius = radius(mapping);
    Some(
        (0..layout.len())
            .map(|e| {
                let at = line.nearest_arc_length(layout.position(e)?) / cell_len;
                let half = radius * layout.spacing(e).map_or(1.0, |s| s / cell_len);
                kernel::sample(
                    mapping.kernel.type_,
                    mapping.edge,
                    (at, height as f64 / 2.0),
                    (half, height as f64 / 2.0),
                    (width, height),
                )
            })
            .collect(),
    )
}

fn plane(
    mapping: &Mapping,
    surface: &Surface,
    layout: &Layout,
    (width, height): (usize, usize),
) -> Option<Vec<Option<ElementTaps>>> {
    let Placement::PlanePlacement(placement) = &surface.placement else {
        return None;
    };
    let origin = math::from_proto(&placement.plane.origin);
    let (edge_u, edge_v) = (
        math::from_proto(&placement.plane.u),
        math::from_proto(&placement.plane.v),
    );
    if !math::plane_is_valid(edge_u, edge_v) || !layout.is_placed() {
        return None;
    }
    // A cell's size in metres along u and along v.
    let cell_u = math::length(edge_u) / width as f64;
    let cell_v = math::length(edge_v) / height as f64;
    let radius = radius(mapping);
    Some(
        (0..layout.len())
            .map(|element| {
                let (along_u, along_v) =
                    math::plane_coordinates(layout.position(element)?, origin, edge_u, edge_v)?;
                let half = match layout.spacing(element) {
                    Some(s) => (radius * s / cell_u, radius * s / cell_v),
                    None => (radius, radius),
                };
                kernel::sample(
                    mapping.kernel.type_,
                    mapping.edge,
                    (along_u * width as f64, along_v * height as f64),
                    half,
                    (width, height),
                )
            })
            .collect(),
    )
}
