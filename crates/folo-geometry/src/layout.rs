//! Element layouts: where each element of a target is in the zone ([spatial model]).
//!
//! [spatial model]: ../../../docs/architecture/spatial-model.md

use std::borrow::Cow;
use std::num::NonZeroU64;

use folo_protocol::generated::{
    DisplayGeometry, DisplayResolution, Geometry, GridGeometry, LineGeometry, Plane, Target, Warp,
    Wiring, WiringOrder, WiringStart,
};

use crate::math::{self, Polyline, Vec3};

/// The kind of target a layout was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Point,
    Line,
    Grid {
        width: usize,
        height: usize,
    },
    /// Reserved for projection mapping: validated, never executed. It has no elements.
    Display,
}

/// The elements of one target, in the order the provider addresses them.
#[derive(Clone, Debug)]
pub struct Layout {
    shape: Shape,
    count: usize,
    /// One position per element, or empty when the target is unplaced.
    positions: Vec<Vec3>,
    /// One distance in metres per element, or empty. 0 means unknown.
    spacing: Vec<f64>,
}

impl Layout {
    #[must_use]
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// The number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the elements have positions. An unplaced target can only be reached by manual mappings.
    #[must_use]
    pub fn is_placed(&self) -> bool {
        self.count > 0 && self.positions.len() == self.count
    }

    /// The position of element `i` in the zone, in metres.
    #[must_use]
    pub fn position(&self, i: usize) -> Option<Vec3> {
        self.positions.get(i).copied()
    }

    /// How far element `i` is from its neighbours, in metres: what an area kernel's radius multiplies.
    /// `None` for a point, or when the elements are on top of each other.
    #[must_use]
    pub fn spacing(&self, i: usize) -> Option<f64> {
        self.spacing.get(i).copied().filter(|s| *s > 0.0)
    }

    fn unplaced(shape: Shape, count: usize) -> Self {
        Self {
            shape,
            count,
            positions: Vec::new(),
            spacing: Vec::new(),
        }
    }
}

/// The layout of a target's elements.
///
/// - **point:** one element at the position.
/// - **line:** `count` elements. Explicit `positions` win when there are exactly `count` of them.
///   Otherwise element `i` sits at arc length `i / (count − 1)` of the path (a single element sits at
///   its start). A path of fewer than two points leaves the target unplaced.
/// - **grid:** `width × height` elements, numbered in the wiring order. Row 0 and column 0 are at the
///   plane's origin: `top` means `v = 0` and `left` means `u = 0`.
/// - **display:** no elements.
#[must_use]
pub fn element_layout(target: &Target) -> Layout {
    match resolved_geometry(&target.geometry).as_ref() {
        Geometry::PointGeometry(p) => {
            let at = math::from_proto(&p.position);
            Layout {
                shape: Shape::Point,
                count: 1,
                positions: vec![at],
                spacing: vec![0.0],
            }
        }
        Geometry::LineGeometry(l) => line_layout(l),
        Geometry::GridGeometry(g) => grid_layout(g),
        Geometry::DisplayGeometry(_) => Layout::unplaced(Shape::Display, 0),
    }
}

/// The geometry as its own `kind` says.
///
/// The generated `Geometry` is an untagged union whose `kind` is a free value, so an object with
/// only `kind` (and `position`) is read as a point whatever its kind. Such an object is a line,
/// grid or display with every other field at its default.
pub(crate) fn resolved_geometry(geometry: &Geometry) -> Cow<'_, Geometry> {
    let Geometry::PointGeometry(point) = geometry else {
        return Cow::Borrowed(geometry);
    };
    let kind = point.kind.as_str();
    Cow::Owned(match kind {
        Some("line") => Geometry::LineGeometry(LineGeometry {
            count: NonZeroU64::MIN,
            kind: point.kind.clone(),
            path: Vec::new(),
            positions: Vec::new(),
        }),
        Some("grid") => Geometry::GridGeometry(GridGeometry {
            height: NonZeroU64::MIN,
            kind: point.kind.clone(),
            plane: Plane::default(),
            width: NonZeroU64::MIN,
            wiring: Wiring::default(),
        }),
        Some("display") => Geometry::DisplayGeometry(DisplayGeometry {
            kind: point.kind.clone(),
            mask: Vec::new(),
            rect: Plane::default(),
            resolution: DisplayResolution::default(),
            warp: Warp::default(),
        }),
        _ => return Cow::Borrowed(geometry),
    })
}

/// The `kind` a geometry carries.
pub(crate) fn geometry_kind(geometry: &Geometry) -> &str {
    let kind = match geometry {
        Geometry::PointGeometry(g) => &g.kind,
        Geometry::LineGeometry(g) => &g.kind,
        Geometry::GridGeometry(g) => &g.kind,
        Geometry::DisplayGeometry(g) => &g.kind,
    };
    kind.as_str().unwrap_or_default()
}

fn line_layout(l: &LineGeometry) -> Layout {
    let count = usize::try_from(l.count.get()).unwrap_or(usize::MAX);
    let explicit: Vec<Vec3> = l.positions.iter().map(math::from_proto).collect();
    let positions = if explicit.len() == count {
        explicit
    } else if l.path.len() >= 2 {
        let line = Polyline::new(l.path.iter().map(math::from_proto).collect());
        let total = line.length();
        (0..count)
            .map(|i| {
                let t = if count > 1 {
                    i as f64 / (count - 1) as f64
                } else {
                    0.0
                };
                line.point_at(t * total)
            })
            .collect()
    } else {
        return Layout::unplaced(Shape::Line, count);
    };
    let spacing = (0..count)
        .map(|i| {
            let mut sum = 0.0;
            let mut n = 0.0;
            if i > 0 {
                sum += math::length(math::sub(positions[i], positions[i - 1]));
                n += 1.0;
            }
            if i + 1 < count {
                sum += math::length(math::sub(positions[i + 1], positions[i]));
                n += 1.0;
            }
            if n > 0.0 { sum / n } else { 0.0 }
        })
        .collect();
    Layout {
        shape: Shape::Line,
        count,
        positions,
        spacing,
    }
}

fn grid_layout(grid: &GridGeometry) -> Layout {
    let width = usize::try_from(grid.width.get()).unwrap_or(usize::MAX);
    let height = usize::try_from(grid.height.get()).unwrap_or(usize::MAX);
    let count = width * height;
    let origin = math::from_proto(&grid.plane.origin);
    let u = math::from_proto(&grid.plane.u);
    let v = math::from_proto(&grid.plane.v);
    let positions = (0..count)
        .map(|index| {
            let (cx, cy) = wiring_cell(index, width, height, &grid.wiring);
            let along_u = (cx as f64 + 0.5) / width as f64;
            let along_v = (cy as f64 + 0.5) / height as f64;
            math::add(
                origin,
                math::add(math::scale(u, along_u), math::scale(v, along_v)),
            )
        })
        .collect();
    let pitch = f64::midpoint(
        math::length(u) / width as f64,
        math::length(v) / height as f64,
    );
    Layout {
        shape: Shape::Grid { width, height },
        count,
        positions,
        spacing: vec![pitch; count],
    }
}

/// The `(column, row)` of the grid cell that wiring index `k` lights.
///
/// The wire runs along rows (`row`) or along columns (`column`), from the start corner. A serpentine
/// wire reverses direction on every other run.
#[must_use]
pub fn wiring_cell(k: usize, width: usize, height: usize, wiring: &Wiring) -> (usize, usize) {
    let run_len = match wiring.order {
        WiringOrder::Row => width,
        WiringOrder::Column => height,
    };
    let (run, mut along) = (k / run_len, k % run_len);
    if wiring.serpentine && run % 2 == 1 {
        along = run_len - 1 - along;
    }
    let (mut cx, mut cy) = match wiring.order {
        WiringOrder::Row => (along, run),
        WiringOrder::Column => (run, along),
    };
    if matches!(
        wiring.start,
        WiringStart::TopRight | WiringStart::BottomRight
    ) {
        cx = width - 1 - cx;
    }
    if matches!(
        wiring.start,
        WiringStart::BottomLeft | WiringStart::BottomRight
    ) {
        cy = height - 1 - cy;
    }
    (cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use folo_protocol::generated::{
        DisplayGeometry, DisplayResolution, Plane, PointGeometry, TargetKind, Vec3 as P, Warp,
    };

    fn p(x: f64, y: f64, z: f64) -> P {
        P { x, y, z }
    }

    fn line(count: u64, path: &[P], positions: &[P]) -> Target {
        Target {
            geometry: Geometry::LineGeometry(LineGeometry {
                count: count.try_into().unwrap(),
                kind: "line".into(),
                path: path.to_vec(),
                positions: positions.to_vec(),
            }),
            ..Target::default()
        }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn a_line_spaces_elements_evenly_by_arc_length() {
        // An L: 2 m along x, then 2 m along y. Five elements are 1 m apart along the path.
        let t = line(
            5,
            &[p(0.0, 0.0, 0.0), p(2.0, 0.0, 0.0), p(2.0, 2.0, 0.0)],
            &[],
        );
        let layout = element_layout(&t);
        assert!(layout.is_placed());
        let want = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [2.0, 2.0, 0.0],
        ];
        for (i, w) in want.iter().enumerate() {
            assert!(
                near(layout.position(i).unwrap(), *w),
                "element {i}: {:?}",
                layout.position(i)
            );
        }
        assert!((layout.spacing(2).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn explicit_positions_win_only_with_exactly_count_entries() {
        let path = [p(0.0, 0.0, 0.0), p(3.0, 0.0, 0.0)];
        let exact = element_layout(&line(2, &path, &[p(0.0, 1.0, 0.0), p(0.0, 2.0, 0.0)]));
        assert!(near(exact.position(1).unwrap(), [0.0, 2.0, 0.0]));
        // Three positions for two elements are ignored: the path is used.
        let extra = element_layout(&line(
            2,
            &path,
            &[p(0.0, 1.0, 0.0), p(0.0, 2.0, 0.0), p(0.0, 3.0, 0.0)],
        ));
        assert!(near(extra.position(1).unwrap(), [3.0, 0.0, 0.0]));
    }

    #[test]
    fn a_short_path_leaves_the_line_unplaced() {
        for path in [vec![], vec![p(1.0, 1.0, 1.0)]] {
            let layout = element_layout(&line(150, &path, &[]));
            assert_eq!(layout.len(), 150);
            assert!(!layout.is_placed());
            assert_eq!(layout.position(0), None);
        }
    }

    #[test]
    fn one_element_sits_at_the_start_of_the_path() {
        let layout = element_layout(&line(1, &[p(1.0, 2.0, 3.0), p(5.0, 2.0, 3.0)], &[]));
        assert!(near(layout.position(0).unwrap(), [1.0, 2.0, 3.0]));
    }

    #[test]
    fn a_path_without_length_puts_every_element_on_its_point() {
        let layout = element_layout(&line(3, &[p(1.0, 1.0, 0.0), p(1.0, 1.0, 0.0)], &[]));
        assert!(layout.is_placed());
        assert!(near(layout.position(2).unwrap(), [1.0, 1.0, 0.0]));
        assert_eq!(layout.spacing(1), None);
    }

    #[test]
    fn a_point_is_one_element() {
        let t = Target {
            kind: TargetKind::Point,
            geometry: Geometry::PointGeometry(PointGeometry {
                kind: "point".into(),
                position: p(1.0, 2.0, 3.0),
            }),
            ..Target::default()
        };
        let layout = element_layout(&t);
        assert_eq!((layout.len(), layout.shape()), (1, Shape::Point));
        assert!(near(layout.position(0).unwrap(), [1.0, 2.0, 3.0]));
        assert_eq!(layout.spacing(0), None);
    }

    fn wiring(order: WiringOrder, serpentine: bool, start: WiringStart) -> Wiring {
        Wiring {
            order,
            serpentine,
            start,
        }
    }

    fn cells(w: usize, h: usize, wiring: &Wiring) -> Vec<(usize, usize)> {
        (0..w * h).map(|k| wiring_cell(k, w, h, wiring)).collect()
    }

    #[test]
    fn wiring_orders() {
        use WiringOrder::{Column, Row};
        use WiringStart::{BottomLeft, BottomRight, TopLeft, TopRight};
        assert_eq!(
            cells(3, 2, &wiring(Row, false, TopLeft)),
            [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]
        );
        assert_eq!(
            cells(3, 2, &wiring(Row, true, TopLeft)),
            [(0, 0), (1, 0), (2, 0), (2, 1), (1, 1), (0, 1)]
        );
        assert_eq!(
            cells(3, 2, &wiring(Column, false, TopLeft)),
            [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]
        );
        assert_eq!(
            cells(3, 2, &wiring(Column, true, TopLeft)),
            [(0, 0), (0, 1), (1, 1), (1, 0), (2, 0), (2, 1)]
        );
        assert_eq!(
            cells(3, 2, &wiring(Row, false, TopRight)),
            [(2, 0), (1, 0), (0, 0), (2, 1), (1, 1), (0, 1)]
        );
        assert_eq!(
            cells(3, 2, &wiring(Row, true, BottomLeft)),
            [(0, 1), (1, 1), (2, 1), (2, 0), (1, 0), (0, 0)]
        );
        assert_eq!(
            cells(2, 2, &wiring(Column, true, BottomRight)),
            [(1, 1), (1, 0), (0, 0), (0, 1)]
        );
    }

    #[test]
    fn every_wiring_visits_each_cell_once() {
        for order in [WiringOrder::Row, WiringOrder::Column] {
            for serpentine in [false, true] {
                for start in [
                    WiringStart::TopLeft,
                    WiringStart::TopRight,
                    WiringStart::BottomLeft,
                    WiringStart::BottomRight,
                ] {
                    let mut seen = cells(5, 3, &wiring(order, serpentine, start));
                    seen.sort_unstable();
                    seen.dedup();
                    assert_eq!(seen.len(), 15, "{order:?} {serpentine} {start:?}");
                }
            }
        }
    }

    #[test]
    fn a_grid_places_cell_centres_on_its_plane() {
        let t = Target {
            kind: TargetKind::Grid,
            geometry: Geometry::GridGeometry(GridGeometry {
                height: 2.try_into().unwrap(),
                kind: "grid".into(),
                plane: Plane {
                    origin: p(1.0, 0.0, 0.0),
                    u: p(4.0, 0.0, 0.0),
                    v: p(0.0, 2.0, 0.0),
                },
                width: 4.try_into().unwrap(),
                wiring: wiring(WiringOrder::Row, true, WiringStart::TopLeft),
            }),
            ..Target::default()
        };
        let layout = element_layout(&t);
        assert_eq!(
            (layout.len(), layout.shape()),
            (
                8,
                Shape::Grid {
                    width: 4,
                    height: 2
                }
            )
        );
        assert!(near(layout.position(0).unwrap(), [1.5, 0.5, 0.0]));
        // Serpentine: the second row runs back, so index 4 is the right end of row 1.
        assert!(near(layout.position(4).unwrap(), [4.5, 1.5, 0.0]));
        assert!((layout.spacing(0).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_display_has_no_elements() {
        let t = Target {
            kind: TargetKind::Display,
            geometry: Geometry::DisplayGeometry(DisplayGeometry {
                kind: "display".into(),
                mask: vec![],
                rect: Plane::default(),
                resolution: DisplayResolution::default(),
                warp: Warp::default(),
            }),
            ..Target::default()
        };
        let layout = element_layout(&t);
        assert_eq!((layout.len(), layout.shape()), (0, Shape::Display));
    }
}
