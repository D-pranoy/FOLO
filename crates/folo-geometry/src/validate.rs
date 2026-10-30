//! Checks of a zone's geometry and mappings, for the configuration to report and the projector to skip by.

use std::collections::HashSet;
use std::fmt;

use folo_protocol::generated::{
    Geometry, Mapping, MappingMethod, Placement, Surface, Target, Zone,
};

use crate::layout::{Layout, Shape, element_layout, geometry_kind, resolved_geometry};
use crate::math::{self, Polyline};
use crate::plan::published_size;

/// How serious an [`Issue`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The part can't work and the projector skips it.
    Error,
    /// The part works, but not as it may have been meant.
    Warning,
}

/// One finding of [`validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// Where, like `targets[strip].geometry`, relative to the zone.
    pub path: String,
    pub message: String,
}

impl Issue {
    fn error(path: String, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path,
            message: message.into(),
        }
    }

    fn warning(path: String, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            path,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = if self.is_error() { "error" } else { "warning" };
        write!(f, "{level}: {}: {}", self.path, self.message)
    }
}

/// Checks a zone's targets, surfaces and mappings. Nothing is rejected: the projector skips what
/// has an error, and a zone with errors still runs the rest.
#[must_use]
pub fn validate(zone: &Zone) -> Vec<Issue> {
    let mut issues = Vec::new();
    unique(
        &mut issues,
        "targets",
        zone.targets.iter().map(|t| t.id.as_str()),
    );
    unique(
        &mut issues,
        "surfaces",
        zone.surfaces.iter().map(|s| s.id.as_str()),
    );
    unique(
        &mut issues,
        "mappings",
        zone.mappings.iter().map(|m| m.id.as_str()),
    );
    for target in &zone.targets {
        target_issues(&mut issues, target);
    }
    for surface in &zone.surfaces {
        surface_issues(&mut issues, surface);
    }
    for mapping in &zone.mappings {
        let surface = zone.surfaces.iter().find(|s| *s.id == *mapping.surface);
        let target = zone.targets.iter().find(|t| *t.id == *mapping.target);
        let path = format!("mappings[{}]", mapping.id.as_str());
        if surface.is_none() {
            issues.push(Issue::error(
                path.clone(),
                format!("no surface `{}`", mapping.surface.as_str()),
            ));
        }
        if target.is_none() {
            issues.push(Issue::error(
                path.clone(),
                format!("no target `{}`", mapping.target.as_str()),
            ));
        }
        if let (Some(surface), Some(target)) = (surface, target) {
            issues.extend(mapping_issues(mapping, surface, &element_layout(target)));
        }
    }
    issues
}

fn unique<'a>(issues: &mut Vec<Issue>, what: &str, ids: impl Iterator<Item = &'a str>) {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            issues.push(Issue::error(
                format!("{what}[{id}]"),
                "the id is used more than once",
            ));
        }
    }
}

fn target_issues(issues: &mut Vec<Issue>, target: &Target) {
    let path = format!("targets[{}].geometry", target.id.as_str());
    let geometry = resolved_geometry(&target.geometry);
    let matches = geometry_kind(&geometry) == target.kind.to_string();
    if !matches {
        issues.push(Issue::error(
            path,
            "the geometry is not of the target's kind",
        ));
        return;
    }
    match geometry.as_ref() {
        Geometry::LineGeometry(l) => {
            let count = l.count.get() as usize;
            if !l.positions.is_empty() && l.positions.len() != count {
                issues.push(Issue::warning(
                    path.clone(),
                    format!(
                        "{} positions for {count} elements are ignored: it takes exactly `count`",
                        l.positions.len()
                    ),
                ));
            }
            if !element_layout(target).is_placed() {
                issues.push(Issue::warning(
                    path,
                    "the line has no path of two points or positions, so only manual mappings reach it",
                ));
            }
        }
        Geometry::GridGeometry(g) => {
            if !math::plane_is_valid(math::from_proto(&g.plane.u), math::from_proto(&g.plane.v)) {
                issues.push(Issue::error(
                    path,
                    "the grid's u and v must be two different directions",
                ));
            }
        }
        Geometry::DisplayGeometry(d) => {
            if !math::plane_is_valid(math::from_proto(&d.rect.u), math::from_proto(&d.rect.v)) {
                issues.push(Issue::error(
                    path.clone(),
                    "the rectangle's u and v must be two different directions",
                ));
            }
            if !d.mask.is_empty() && d.mask.len() < 3 {
                issues.push(Issue::warning(path, "a mask needs at least three points"));
            }
        }
        Geometry::PointGeometry(_) => {}
    }
}

fn surface_issues(issues: &mut Vec<Issue>, surface: &Surface) {
    let path = format!("surfaces[{}].placement", surface.id.as_str());
    match &surface.placement {
        Placement::NoPlacement(_) => {}
        Placement::PathPlacement(p) => {
            if usable_path(p.points.iter().map(math::from_proto)).is_none() {
                issues.push(Issue::warning(
                    path,
                    "the path needs two points that differ, so only manual mappings reach this surface",
                ));
            }
        }
        Placement::PlanePlacement(p) => {
            if !math::plane_is_valid(math::from_proto(&p.plane.u), math::from_proto(&p.plane.v)) {
                issues.push(Issue::error(
                    path,
                    "the plane's u and v must be two different directions",
                ));
            }
        }
    }
}

/// A polyline of the points, when there are two or more and it has a length.
pub(crate) fn usable_path(points: impl Iterator<Item = math::Vec3>) -> Option<Polyline> {
    let points: Vec<_> = points.collect();
    if points.len() < 2 {
        return None;
    }
    let line = Polyline::new(points);
    (line.length() > 0.0).then_some(line)
}

/// What is wrong with one mapping, given its surface and its target's layout. Display targets are skipped
/// with a warning.
pub(crate) fn mapping_issues(mapping: &Mapping, surface: &Surface, layout: &Layout) -> Vec<Issue> {
    let path = format!("mappings[{}]", mapping.id.as_str());
    let mut issues = Vec::new();
    if layout.shape() == Shape::Display {
        issues.push(Issue::warning(
            path,
            "display targets are validated only: the mapping is skipped",
        ));
        return issues;
    }
    match mapping.method {
        MappingMethod::Manual => {
            let (width, height) = published_size(surface);
            let cells = (width * height) as u64;
            if mapping.ranges.is_empty() {
                issues.push(Issue::warning(
                    path.clone(),
                    "a manual mapping without ranges draws nothing",
                ));
            }
            let mut taken = vec![false; layout.len()];
            for (i, r) in mapping.ranges.iter().enumerate() {
                let at = format!("{path}.ranges[{i}]");
                let (cs, cc) = (u64::from(r.cells.start), r.cells.count.get());
                let (es, ec) = (u64::from(r.elements.start), r.elements.count.get());
                if cs + cc > cells {
                    issues.push(Issue::error(
                        at.clone(),
                        format!("cells {cs}..{} are beyond the surface's {cells}", cs + cc),
                    ));
                }
                if es + ec > layout.len() as u64 {
                    issues.push(Issue::error(
                        at,
                        format!(
                            "elements {es}..{} are beyond the target's {}",
                            es + ec,
                            layout.len()
                        ),
                    ));
                    continue;
                }
                let span = &mut taken[es as usize..(es + ec) as usize];
                if span.iter().any(|t| *t) {
                    issues.push(Issue::warning(
                        at,
                        "some elements are in an earlier range too: this range wins",
                    ));
                }
                span.fill(true);
            }
        }
        MappingMethod::Path => {
            let ok = matches!(&surface.placement, Placement::PathPlacement(p)
                if usable_path(p.points.iter().map(math::from_proto)).is_some());
            if !ok {
                issues.push(Issue::error(
                    path.clone(),
                    "a path mapping needs a surface with a path placement",
                ));
            }
            if !layout.is_placed() {
                issues.push(Issue::error(path, "a path mapping needs a placed target"));
            }
        }
        MappingMethod::Plane => {
            let ok = matches!(&surface.placement, Placement::PlanePlacement(p)
                if math::plane_is_valid(math::from_proto(&p.plane.u), math::from_proto(&p.plane.v)));
            if !ok {
                issues.push(Issue::error(
                    path.clone(),
                    "a plane mapping needs a surface with a plane placement",
                ));
            }
            if !layout.is_placed() {
                issues.push(Issue::error(path, "a plane mapping needs a placed target"));
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use folo_protocol::generated::{
        DisplayGeometry, DisplayResolution, LineGeometry, ManualRange, Plane, Span, TargetKind,
        Warp,
    };

    fn line_target(id: &str, count: u64) -> Target {
        Target {
            id: id.parse().unwrap(),
            geometry: Geometry::LineGeometry(LineGeometry {
                count: count.try_into().unwrap(),
                kind: "line".into(),
                path: vec![],
                positions: vec![],
            }),
            ..Target::default()
        }
    }

    fn strip(id: &str, width: u64) -> Surface {
        let mut s = Surface {
            id: id.parse().unwrap(),
            ..Surface::default()
        };
        s.resolution.width = width.try_into().unwrap();
        s
    }

    fn manual(id: &str, surface: &str, target: &str, ranges: Vec<ManualRange>) -> Mapping {
        Mapping {
            id: id.parse().unwrap(),
            surface: surface.parse().unwrap(),
            target: target.parse().unwrap(),
            ranges,
            ..Mapping::default()
        }
    }

    fn range(cells: (u16, u64), elements: (u16, u64)) -> ManualRange {
        ManualRange {
            cells: Span {
                start: cells.0,
                count: cells.1.try_into().unwrap(),
            },
            elements: Span {
                start: elements.0,
                count: elements.1.try_into().unwrap(),
            },
            reversed: false,
        }
    }

    fn zone(mappings: Vec<Mapping>) -> Zone {
        Zone {
            targets: vec![line_target("strip", 10)],
            surfaces: vec![strip("main", 10)],
            mappings,
            ..Zone::default()
        }
    }

    fn errors(zone: &Zone) -> Vec<String> {
        validate(zone)
            .into_iter()
            .filter(Issue::is_error)
            .map(|i| i.to_string())
            .collect()
    }

    #[test]
    fn a_sound_zone_has_no_errors() {
        let z = zone(vec![manual(
            "m",
            "main",
            "strip",
            vec![range((0, 10), (0, 10))],
        )]);
        assert_eq!(errors(&z), Vec::<String>::new());
    }

    #[test]
    fn unknown_references_and_duplicate_ids_are_errors() {
        let mut z = zone(vec![
            manual("m", "nope", "strip", vec![]),
            manual("m", "main", "gone", vec![]),
        ]);
        z.targets.push(line_target("strip", 3));
        let e = errors(&z);
        assert!(e.iter().any(|e| e.contains("no surface `nope`")), "{e:?}");
        assert!(e.iter().any(|e| e.contains("no target `gone`")), "{e:?}");
        assert!(
            e.iter()
                .any(|e| e.contains("targets[strip]") && e.contains("more than once")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|e| e.contains("mappings[m]") && e.contains("more than once")),
            "{e:?}"
        );
    }

    #[test]
    fn ranges_beyond_the_surface_or_target_are_errors() {
        let z = zone(vec![manual(
            "m",
            "main",
            "strip",
            vec![range((5, 10), (0, 10)), range((0, 5), (8, 5))],
        )]);
        let e = errors(&z);
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e[0].contains("ranges[0]") && e[0].contains("beyond the surface"));
        assert!(e[1].contains("ranges[1]") && e[1].contains("beyond the target"));
    }

    #[test]
    fn path_and_plane_mappings_need_placements() {
        let mut path = manual("p", "main", "strip", vec![]);
        path.method = MappingMethod::Path;
        let mut plane = manual("q", "main", "strip", vec![]);
        plane.method = MappingMethod::Plane;
        let e = errors(&zone(vec![path, plane]));
        // Each is missing the surface's placement and the unplaced target.
        assert_eq!(e.len(), 4, "{e:?}");
    }

    #[test]
    fn an_unplaced_line_and_a_display_target_are_warnings() {
        let mut z = zone(vec![manual(
            "m",
            "main",
            "strip",
            vec![range((0, 10), (0, 10))],
        )]);
        let all = validate(&z);
        assert!(
            all.iter()
                .any(|i| !i.is_error() && i.message.contains("only manual mappings")),
            "{all:?}"
        );
        z.targets[0].kind = TargetKind::Display;
        z.targets[0].geometry = display_geometry();
        let all = validate(&z);
        assert!(all.iter().all(|i| !i.is_error()), "{all:?}");
        assert!(
            all.iter().any(|i| i.message.contains("validated only")),
            "{all:?}"
        );
    }

    #[test]
    fn a_geometry_of_another_kind_is_an_error() {
        let mut z = zone(vec![]);
        z.targets[0].kind = TargetKind::Grid;
        assert!(
            errors(&z)
                .iter()
                .any(|e| e.contains("not of the target's kind"))
        );
    }

    fn display_geometry() -> Geometry {
        Geometry::DisplayGeometry(DisplayGeometry {
            kind: "display".into(),
            mask: vec![],
            rect: Plane::default(),
            resolution: DisplayResolution::default(),
            warp: Warp::default(),
        })
    }
}
