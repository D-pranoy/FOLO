//! The projector: plans per revision, mapping priority and blending, edges and errors.

// The test values are small whole numbers, so casts are exact and float comparisons are meant.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

mod common;

use common::{Map, bare_strip, canvas, close, line, mappings, pixels, plane, strip, view, zone};
use folo_geometry::{CpuProjector, ProjectError, Projector, Severity};
use folo_protocol::generated::MappingBlend;
use serde_json::json;

const RED: [f32; 3] = [200.0, 0.0, 0.0];
const BLUE: [f32; 3] = [0.0, 100.0, 50.0];

fn solid(cells: usize, colour: [f32; 3]) -> Vec<[f32; 3]> {
    vec![colour; cells]
}

fn project_to(
    projector: &CpuProjector,
    surfaces: &[(&str, &[[f32; 3]])],
    n: usize,
) -> Vec<[f32; 3]> {
    let views: Vec<_> = surfaces
        .iter()
        .map(|(id, px)| (*id, view((px.len(), 1), px)))
        .collect();
    let mut out = vec![[f32::NAN; 3]; n];
    projector.project("z", "t", &views, &mut out).unwrap();
    out
}

/// Two 4-cell strips mapped onto a 4-element line: `a` on all of it, `b` on elements 1 and 2.
fn two_strips(a: Map<'_>, b: Map<'_>) -> folo_protocol::generated::Zone {
    zone(
        json!([bare_strip("a", 4), bare_strip("b", 4)]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[
            a.ranges(&[((0, 4), (0, 4), false)]),
            b.ranges(&[((0, 2), (1, 2), false)]),
        ]),
    )
}

#[test]
fn plans_are_kept_for_a_revision_and_rebuilt_for_the_next() {
    let make = |blend: &str| {
        two_strips(
            Map::new("ma", "a", "t", "manual").blend("alpha"),
            Map::new("mb", "b", "t", "manual").blend(blend),
        )
    };
    let mut projector = CpuProjector::new();
    assert_eq!(projector.revision("z"), None);

    assert!(projector.prepare(1, &make("max")).rebuilt);
    assert_eq!(projector.revision("z"), Some(1));
    let blends = |p: &CpuProjector| {
        p.plans("z", "t")
            .unwrap()
            .iter()
            .map(|m| m.blend)
            .collect::<Vec<_>>()
    };
    assert_eq!(blends(&projector), [MappingBlend::Alpha, MappingBlend::Max]);

    // The same revision is the same configuration: nothing is rebuilt, even for a changed zone.
    assert!(!projector.prepare(1, &make("alpha")).rebuilt);
    assert_eq!(blends(&projector), [MappingBlend::Alpha, MappingBlend::Max]);

    // A new revision builds again.
    assert!(projector.prepare(2, &make("alpha")).rebuilt);
    assert_eq!(
        blends(&projector),
        [MappingBlend::Alpha, MappingBlend::Alpha]
    );
    // Going back is a new revision too.
    assert!(projector.prepare(1, &make("max")).rebuilt);

    projector.forget("z");
    assert_eq!(projector.revision("z"), None);
    assert!(projector.plans("z", "t").is_none());
}

#[test]
fn prepare_reports_issues_and_skips_mappings_with_errors() {
    let z = zone(
        json!([bare_strip("a", 4)]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[
            // Cells 0..9 don't exist on a 4-cell strip.
            Map::new("bad", "a", "t", "manual").ranges(&[((0, 9), (0, 4), false)]),
            Map::new("good", "a", "t", "manual").ranges(&[((0, 4), (0, 4), false)]),
        ]),
    );
    let mut projector = CpuProjector::new();
    let prepared = projector.prepare(0, &z);
    let errors: Vec<_> = prepared
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "{:?}", prepared.issues);
    assert!(errors[0].path.contains("mappings[bad]"));
    let plans = projector.plans("z", "t").unwrap();
    assert_eq!(
        plans.iter().map(|m| m.mapping.as_str()).collect::<Vec<_>>(),
        ["good"]
    );
    // The issues are the same on a kept revision.
    assert_eq!(projector.prepare(0, &z).issues, prepared.issues);
}

#[test]
fn mappings_apply_in_ascending_priority_and_max_blends_channels() {
    let a = solid(4, RED);
    let b = solid(4, BLUE);
    let z = two_strips(
        Map::new("ma", "a", "t", "manual").priority(5),
        Map::new("mb", "b", "t", "manual").priority(0),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    let order: Vec<_> = projector
        .plans("z", "t")
        .unwrap()
        .iter()
        .map(|m| m.mapping.clone())
        .collect();
    assert_eq!(order, ["mb", "ma"]);
    let out = project_to(&projector, &[("a", &a), ("b", &b)], 4);
    // Max per channel where both reach (elements 1 and 2); `a` alone elsewhere.
    assert_eq!(out, [RED, [200.0, 100.0, 50.0], [200.0, 100.0, 50.0], RED]);
}

#[test]
fn alpha_blending_paints_over_what_is_below() {
    let (a, b) = (solid(4, RED), solid(4, BLUE));
    let surfaces: [(&str, &[[f32; 3]]); 2] = [("a", &a), ("b", &b)];

    // `b` above `a`: it replaces `a` on elements 1 and 2.
    let z = two_strips(
        Map::new("ma", "a", "t", "manual")
            .blend("alpha")
            .priority(0),
        Map::new("mb", "b", "t", "manual")
            .blend("alpha")
            .priority(5),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    assert_eq!(project_to(&projector, &surfaces, 4), [RED, BLUE, BLUE, RED]);

    // The other way round, `a` covers `b` completely.
    let z = two_strips(
        Map::new("ma", "a", "t", "manual")
            .blend("alpha")
            .priority(5),
        Map::new("mb", "b", "t", "manual")
            .blend("alpha")
            .priority(0),
    );
    projector.prepare(1, &z);
    assert_eq!(project_to(&projector, &surfaces, 4), [RED; 4]);

    // Equal priorities go in document order: the later mapping is on top.
    let z = two_strips(
        Map::new("ma", "a", "t", "manual").blend("alpha"),
        Map::new("mb", "b", "t", "manual").blend("alpha"),
    );
    projector.prepare(2, &z);
    assert_eq!(project_to(&projector, &surfaces, 4), [RED, BLUE, BLUE, RED]);
}

#[test]
fn alpha_follows_the_share_of_the_kernel_on_the_surface() {
    // 4 cells over 8 elements, bilinear with the `none` edge: the end elements' kernels hang a
    // quarter off the surface, so they're a quarter transparent over what is below.
    let below = solid(4, [200.0, 200.0, 200.0]);
    let above = solid(4, [100.0, 100.0, 100.0]);
    let z = zone(
        json!([bare_strip("below", 4), bare_strip("above", 4)]),
        json!([line("t", 8, &[], &[])]),
        mappings(&[
            Map::new("lo", "below", "t", "manual")
                .kernel("nearest")
                .blend("alpha")
                .ranges(&[((0, 4), (0, 8), false)]),
            Map::new("hi", "above", "t", "manual")
                .kernel("bilinear")
                .edge("none")
                .blend("alpha")
                .priority(1)
                .ranges(&[((0, 4), (0, 8), false)]),
        ]),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    let out = project_to(&projector, &[("below", &below), ("above", &above)], 8);
    // Element 0 samples at 0.25: bilinear reads cell -1 (a quarter) and cell 0 (three quarters).
    assert!(close(out[0], [125.0; 3]), "{:?}", out[0]);
    assert!(close(out[7], [125.0; 3]), "{:?}", out[7]);
    assert!(close(out[1], [100.0; 3]), "{:?}", out[1]);
    assert!(close(out[3], [100.0; 3]), "{:?}", out[3]);
}

#[test]
fn elements_no_mapping_covers_are_black_and_a_missing_surface_is_skipped() {
    let a = solid(4, RED);
    let z = zone(
        json!([bare_strip("a", 4), bare_strip("b", 4)]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[
            Map::new("ma", "a", "t", "manual").ranges(&[((0, 2), (1, 2), false)]),
            Map::new("mb", "b", "t", "manual").ranges(&[((0, 4), (0, 4), false)]),
        ]),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    // `b` has no frame yet: only `a` draws, on elements 1 and 2.
    let out = project_to(&projector, &[("a", &a)], 4);
    assert_eq!(out, [[0.0; 3], RED, RED, [0.0; 3]]);
    // Nothing at all: black.
    assert_eq!(project_to(&projector, &[], 4), [[0.0; 3]; 4]);
}

#[test]
fn errors_leave_the_output_untouched() {
    let a = solid(4, RED);
    let z = zone(
        json!([bare_strip("a", 4)]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[Map::new("ma", "a", "t", "manual").ranges(&[((0, 4), (0, 4), false)])]),
    );
    let mut projector = CpuProjector::new();
    let mut out = vec![[7.0; 3]; 4];
    assert_eq!(
        projector.project("z", "t", &[], &mut out),
        Err(ProjectError::UnknownZone("z".into()))
    );
    projector.prepare(0, &z);
    assert_eq!(
        projector.project("z", "nope", &[], &mut out),
        Err(ProjectError::UnknownTarget("nope".into()))
    );
    let mut short = vec![[7.0; 3]; 3];
    assert_eq!(
        projector.project("z", "t", &[("a", view((4, 1), &a))], &mut short),
        Err(ProjectError::OutputLength {
            expected: 4,
            got: 3
        })
    );
    // A frame of the wrong size: the surface was resized and the plans are for the old size.
    let wide = solid(5, RED);
    assert_eq!(
        projector.project("z", "t", &[("a", view((5, 1), &wide))], &mut out),
        Err(ProjectError::SurfaceSize {
            surface: "a".into(),
            expected: (4, 1),
            got: (5, 1)
        })
    );
    assert_eq!(out, vec![[7.0; 3]; 4]);
    assert!(folo_geometry::SurfaceView::new(2, 2, &a[..3]).is_err());
}

#[test]
fn a_display_target_has_no_elements_and_is_skipped() {
    let a = solid(4, RED);
    let z = zone(
        json!([bare_strip("a", 4)]),
        json!([{"id": "t", "kind": "display", "geometry": {"kind": "display"}}]),
        mappings(&[Map::new("ma", "a", "t", "manual").ranges(&[((0, 4), (0, 4), false)])]),
    );
    let mut projector = CpuProjector::new();
    let prepared = projector.prepare(0, &z);
    assert!(
        prepared
            .issues
            .iter()
            .all(|i| i.severity == Severity::Warning),
        "{prepared:?}"
    );
    assert!(projector.plans("z", "t").unwrap().is_empty());
    let mut out: Vec<[f32; 3]> = Vec::new();
    projector
        .project("z", "t", &[("a", view((4, 1), &a))], &mut out)
        .unwrap();
}

#[test]
fn a_plane_mapping_reads_the_cell_under_each_element() {
    // 4 × 2 cells of one metre each, on the floor. Cell (x, y) is worth 1 + x·10 + y·100.
    let surface = canvas(
        "c",
        (4, 2),
        &plane([0.0; 3], [4.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
    );
    let px = pixels((4, 2), |x, y| [(1 + x * 10 + y * 100) as f32, 0.0, 0.0]);
    let at = |x: f64, y: f64| [x, y, 1.0]; // The height above the plane doesn't matter.
    let positions = [
        at(0.5, 0.5),
        at(3.5, 0.5),
        at(1.2, 1.9),
        at(3.99, 2.0),
        at(-0.1, 0.5),
    ];
    let run = |kernel: &str, edge: &str| {
        let z = zone(
            json!([surface.clone()]),
            json!([line("t", 5, &[], &positions)]),
            mappings(&[Map::new("m", "c", "t", "plane").kernel(kernel).edge(edge)]),
        );
        let mut projector = CpuProjector::new();
        projector.prepare(0, &z);
        let mut out = vec![[0.0; 3]; 5];
        projector
            .project("z", "t", &[("c", view((4, 2), &px))], &mut out)
            .unwrap();
        out.iter().map(|c| c[0]).collect::<Vec<_>>()
    };
    // The point on the far edge (y = 2.0) is in the last row. The one left of the plane is clamped
    // to the first cell, or dropped (black) with the `none` policy.
    assert_eq!(run("nearest", "clamp"), [1.0, 31.0, 111.0, 131.0, 1.0]);
    assert_eq!(run("nearest", "none"), [1.0, 31.0, 111.0, 131.0, 0.0]);
}

#[test]
fn a_plane_mapping_with_a_bilinear_kernel_blends_neighbours() {
    let surface = canvas(
        "c",
        (4, 2),
        &plane([0.0; 3], [4.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
    );
    let px = pixels((4, 2), |x, y| [(x * 10 + y * 100) as f32, 0.0, 0.0]);
    let z = zone(
        json!([surface]),
        json!([line("t", 1, &[], &[[1.0, 0.5, 0.0]])]),
        mappings(&[Map::new("m", "c", "t", "plane").kernel("bilinear")]),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    let mut out = vec![[0.0; 3]];
    projector
        .project("z", "t", &[("c", view((4, 2), &px))], &mut out)
        .unwrap();
    // Between the centres of cells 0 and 1 of the first row.
    assert!(close(out[0], [5.0, 0.0, 0.0]), "{:?}", out[0]);
}

#[test]
fn the_area_kernel_averages_a_box_of_radius_times_the_elements_spacing() {
    // 8 × 2 cells of one metre; four elements 2 m apart on the row y = 1. Radius 0.5 gives a box
    // 2 m across, so each element averages the 2 × 2 cells around it. Cell (x, y) is x·8 + y.
    let surface = canvas(
        "c",
        (8, 2),
        &plane([0.0; 3], [8.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
    );
    let px = pixels((8, 2), |x, y| [(x * 8 + y) as f32, 0.0, 0.0]);
    let z = zone(
        json!([surface]),
        json!([line(
            "t",
            4,
            &[],
            &[
                [1.0, 1.0, 0.0],
                [3.0, 1.0, 0.0],
                [5.0, 1.0, 0.0],
                [7.0, 1.0, 0.0]
            ]
        )]),
        mappings(&[Map::new("m", "c", "t", "plane")]),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    let mut out = vec![[0.0; 3]; 4];
    projector
        .project("z", "t", &[("c", view((8, 2), &px))], &mut out)
        .unwrap();
    // The cells x and x + 1 of both rows, x = 0, 2, 4, 6: (32x + 18) / 4 = 8x + 4.5.
    let want = [4.5, 20.5, 36.5, 52.5].map(|v| [v, 0.0, 0.0]);
    for (got, want) in out.iter().zip(want) {
        assert!(close(*got, want), "{got:?} vs {want:?}");
    }
    // Radius 0 reads only the cell under the point.
    let z = zone(
        json!([canvas(
            "c",
            (8, 2),
            &plane([0.0; 3], [8.0, 0.0, 0.0], [0.0, 2.0, 0.0])
        )]),
        json!([line("t", 1, &[], &[[1.5, 0.5, 0.0]])]),
        mappings(&[Map::new("m", "c", "t", "plane").radius(0.0)]),
    );
    projector.prepare(1, &z);
    let mut out = vec![[0.0; 3]; 1];
    projector
        .project("z", "t", &[("c", view((8, 2), &px))], &mut out)
        .unwrap();
    assert_eq!(out[0], [8.0, 0.0, 0.0]);
}

#[test]
fn a_path_mapping_reads_the_cell_at_the_nearest_point_of_the_path() {
    // A 10 m path along x with 10 cells; cell x is worth x·10.
    let path = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
    let px = pixels((10, 1), |x, _| [(x * 10) as f32, 0.0, 0.0]);
    let positions = [
        [0.5, 3.0, 0.0],
        [9.5, -2.0, 0.0],
        [4.99, 0.0, 1.0],
        [12.0, 0.0, 0.0],
    ];
    for edge in ["clamp", "none"] {
        let z = zone(
            json!([strip("s", 10, &path)]),
            json!([line("t", 4, &[], &positions)]),
            mappings(&[Map::new("m", "s", "t", "path").kernel("nearest").edge(edge)]),
        );
        let mut projector = CpuProjector::new();
        let prepared = projector.prepare(0, &z);
        assert!(
            prepared
                .issues
                .iter()
                .all(|i| i.severity != Severity::Error),
            "{prepared:?}"
        );
        let mut out = vec![[0.0; 3]; 4];
        projector
            .project("z", "t", &[("s", view((10, 1), &px))], &mut out)
            .unwrap();
        // The point past the end is nearest the end of the path, which belongs to the last cell.
        assert_eq!(
            out.iter().map(|c| c[0]).collect::<Vec<_>>(),
            [0.0, 90.0, 40.0, 90.0],
            "{edge}"
        );
    }
}

#[test]
fn mappings_that_cant_work_are_skipped_and_reported() {
    // A path mapping onto a surface with no path.
    let z = zone(
        json!([bare_strip("s", 4)]),
        json!([line("t", 4, &[[0.0; 3], [1.0, 0.0, 0.0]], &[])]),
        mappings(&[Map::new("m", "s", "t", "path")]),
    );
    let mut projector = CpuProjector::new();
    let prepared = projector.prepare(0, &z);
    assert!(
        prepared
            .issues
            .iter()
            .any(|i| i.is_error() && i.message.contains("path placement"))
    );
    assert!(projector.plans("z", "t").unwrap().is_empty());
}

#[test]
fn plans_are_made_over_the_published_size_of_a_canvas() {
    // The engine publishes canvases at most 128 × 72 cells a side, so a 256 × 144 canvas arrives
    // as 128 × 72 and a 1024 × 1024 one as 128 × 72 too.
    for (configured, published) in [
        ((256, 144), (128, 72)),
        ((1024, 1024), (128, 72)),
        ((64, 36), (64, 36)),
    ] {
        let surface = canvas("c", configured, &floor_of(configured));
        let px = pixels(published, |x, y| [x as f32, y as f32, 0.0]);
        let z = zone(
            json!([surface]),
            json!([line("t", 2, &[], &[[0.5, 0.5, 0.0], [0.99, 0.99, 0.0]])]),
            mappings(&[Map::new("m", "c", "t", "plane").kernel("nearest")]),
        );
        let mut projector = CpuProjector::new();
        projector.prepare(0, &z);
        let mut out = vec![[0.0; 3]; 2];
        projector
            .project("z", "t", &[("c", view(published, &px))], &mut out)
            .unwrap();
        // The middle of the canvas and its far corner, in the published cells.
        assert_eq!(
            out[0],
            [(published.0 / 2) as f32, (published.1 / 2) as f32, 0.0],
            "{configured:?}"
        );
        let corner = |n: usize| ((n as f64 * 0.99) as usize) as f32;
        assert_eq!(
            out[1],
            [corner(published.0), corner(published.1), 0.0],
            "{configured:?}"
        );
    }
    // Manual ranges count published cells: 128 × 72 = 9216, not the configured 256 × 144.
    let z = zone(
        json!([canvas("c", (256, 144), &floor_of((256, 144)))]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[Map::new("m", "c", "t", "manual").ranges(&[((9212, 4), (0, 4), false)])]),
    );
    let mut projector = CpuProjector::new();
    assert!(
        projector
            .prepare(0, &z)
            .issues
            .iter()
            .all(|i| !i.is_error())
    );
    let too_far = zone(
        json!([canvas("c", (256, 144), &floor_of((256, 144)))]),
        json!([line("t", 4, &[], &[])]),
        mappings(&[Map::new("m", "c", "t", "manual").ranges(&[((9214, 4), (0, 4), false)])]),
    );
    assert!(
        projector
            .prepare(1, &too_far)
            .issues
            .iter()
            .any(folo_geometry::Issue::is_error)
    );
}

/// A one-metre square, so positions are fractions of the canvas.
fn floor_of(_size: (u32, u32)) -> serde_json::Value {
    plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
}

#[test]
fn a_flat_surface_stays_exactly_flat_through_every_kernel() {
    // Weights sum to 1 only up to f32 rounding; a flat surface must still read exactly.
    for value in [255.0_f32, 200.0, 100.0, 37.0, 1.0] {
        for (cells, elements) in [(97_u32, 31_u32), (170, 60), (31, 97), (64, 7), (1000, 13)] {
            for kernel in ["nearest", "bilinear", "area"] {
                for edge in ["clamp", "none"] {
                    for radius in [0.5, 1.0] {
                        let z = zone(
                            json!([bare_strip("s", cells)]),
                            json!([line("t", elements, &[], &[])]),
                            mappings(&[Map::new("m", "s", "t", "manual")
                                .kernel(kernel)
                                .edge(edge)
                                .radius(radius)
                                .ranges(&[((0, cells), (0, elements), false)])]),
                        );
                        let mut projector = CpuProjector::new();
                        projector.prepare(0, &z);
                        let flat = solid(cells as usize, [value; 3]);
                        let out = project_to(&projector, &[("s", &flat)], elements as usize);
                        for (i, c) in out.iter().enumerate() {
                            assert_eq!(
                                *c, [value; 3],
                                "{value} {cells}->{elements} {kernel} {edge} {radius} element {i}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_flat_canvas_stays_exactly_flat_through_every_kernel() {
    let z_for = |kernel: &str, radius: f64| {
        zone(
            json!([canvas(
                "c",
                (128, 72),
                &plane([0.0; 3], [4.0, 0.0, 0.0], [0.0, 2.25, 0.0])
            )]),
            json!([line("t", 37, &[[0.0, 0.0, 0.0], [4.0, 2.25, 0.0]], &[])]),
            mappings(&[Map::new("m", "c", "t", "plane")
                .kernel(kernel)
                .radius(radius)]),
        )
    };
    let flat = vec![[255.0_f32; 3]; 128 * 72];
    for kernel in ["nearest", "bilinear", "area"] {
        for radius in [0.5, 1.0, 4.0] {
            let mut projector = CpuProjector::new();
            projector.prepare(0, &z_for(kernel, radius));
            let mut out = vec![[0.0; 3]; 37];
            projector
                .project("z", "t", &[("c", view((128, 72), &flat))], &mut out)
                .unwrap();
            assert!(out.iter().all(|c| *c == [255.0; 3]), "{kernel} {radius}");
        }
    }
}

#[test]
fn a_tiny_plane_neither_panics_nor_hangs() {
    // A plane of a nanometre: every area box is enormous in cells.
    let z = zone(
        json!([canvas(
            "c",
            (64, 36),
            &plane([0.0; 3], [1e-9, 0.0, 0.0], [0.0, 1e-9, 0.0])
        )]),
        json!([line(
            "t",
            3,
            &[],
            &[[0.0; 3], [1e-9, 1e-9, 0.0], [5.0, 5.0, 0.0]]
        )]),
        mappings(&[Map::new("m", "c", "t", "plane")]),
    );
    let mut projector = CpuProjector::new();
    projector.prepare(0, &z);
    let px = vec![[9.0_f32; 3]; 64 * 36];
    let mut out = vec![[0.0; 3]; 3];
    projector
        .project("z", "t", &[("c", view((64, 36), &px))], &mut out)
        .unwrap();
}
