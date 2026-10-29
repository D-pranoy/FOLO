//! The parity case: the migrated default setup's manual 1:1 mapping must copy a frame exactly.

// The test values are small whole numbers, so casts are exact and float comparisons are meant.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

mod common;

use common::{migrated_zone, pixels, view};
use folo_geometry::{CpuProjector, Projector, Severity};
use folo_protocol::generated::{KernelType, MappingBlend, MappingEdge};

const SIZE: (usize, usize) = (150, 1);

/// Bytes of every value, in an order with no pattern a mapping could get right by accident.
fn frame() -> Vec<[f32; 3]> {
    let mut state = 0x2545_F491_u32;
    let mut byte = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f32::from((state >> 24) as u8)
    };
    let mut out = pixels(SIZE, |_, _| [byte(), byte(), byte()]);
    out[0] = [0.0, 0.0, 0.0];
    out[1] = [255.0, 255.0, 255.0];
    out[149] = [1.0, 254.0, 127.0];
    out
}

fn project(projector: &CpuProjector, frame: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut out = vec![[f32::NAN; 3]; 150];
    projector
        .project("default", "strip", &[("main", view(SIZE, frame))], &mut out)
        .unwrap();
    out
}

fn bits(colours: &[[f32; 3]]) -> Vec<[u32; 3]> {
    colours.iter().map(|c| c.map(f32::to_bits)).collect()
}

#[test]
fn the_migrated_zone_has_one_tap_of_weight_one_per_element() {
    let zone = migrated_zone();
    let mut projector = CpuProjector::new();
    let prepared = projector.prepare(0, &zone);
    assert!(prepared.rebuilt);
    assert!(
        prepared
            .issues
            .iter()
            .all(|i| i.severity != Severity::Error),
        "{:?}",
        prepared.issues
    );

    let plans = projector
        .plans("default", "strip")
        .expect("the strip target");
    assert_eq!(plans.len(), 1);
    let plan = &plans[0].plan;
    assert_eq!(
        (plan.elements(), plan.surface_size(), plan.max_taps()),
        (150, SIZE, 1)
    );
    for e in 0..150 {
        let (cells, weights) = plan.taps(e);
        assert_eq!(
            (cells, weights),
            (&[e as u32][..], &[1.0_f32][..]),
            "element {e}"
        );
        assert_eq!(plan.alpha(e), 1.0);
    }
}

#[test]
fn the_migrated_zone_copies_every_byte_exactly() {
    let zone = migrated_zone();
    let mut projector = CpuProjector::new();
    projector.prepare(0, &zone);
    let input = frame();
    let out = project(&projector, &input);
    assert_eq!(bits(&out), bits(&input));
    // As the 8-bit values a provider sends.
    let bytes = |c: &[[f32; 3]]| c.iter().flatten().map(|v| *v as u8).collect::<Vec<_>>();
    assert_eq!(bytes(&out), bytes(&input));
}

#[test]
fn equal_counts_copy_exactly_whatever_the_kernel_edge_or_blend() {
    let input = frame();
    for kernel in [KernelType::Nearest, KernelType::Bilinear, KernelType::Area] {
        for edge in [MappingEdge::Clamp, MappingEdge::None] {
            for blend in [MappingBlend::Max, MappingBlend::Alpha] {
                for radius in [0.0, 0.5, 1.0] {
                    let mut zone = migrated_zone();
                    zone.mappings[0].kernel.type_ = kernel;
                    zone.mappings[0].kernel.radius = radius;
                    zone.mappings[0].edge = edge;
                    zone.mappings[0].blend = blend;
                    let mut projector = CpuProjector::new();
                    projector.prepare(0, &zone);
                    let out = project(&projector, &input);
                    assert_eq!(bits(&out), bits(&input), "{kernel} {edge} {blend} {radius}");
                }
            }
        }
    }
}

#[test]
fn a_reversed_range_reverses_the_frame() {
    let mut zone = migrated_zone();
    zone.mappings[0].ranges[0].reversed = true;
    let mut projector = CpuProjector::new();
    projector.prepare(0, &zone);
    let input = frame();
    let out = project(&projector, &input);
    let reversed: Vec<_> = input.iter().rev().copied().collect();
    assert_eq!(bits(&out), bits(&reversed));
}

#[test]
fn the_frame_is_copied_through_a_range_that_doesnt_start_at_zero() {
    // The strip's cells 10..60 to elements 100..150.
    let mut zone = migrated_zone();
    let range = &mut zone.mappings[0].ranges[0];
    range.cells.start = 10;
    range.cells.count = 50.try_into().unwrap();
    range.elements.start = 100;
    range.elements.count = 50.try_into().unwrap();
    let mut projector = CpuProjector::new();
    projector.prepare(0, &zone);
    let input = frame();
    let out = project(&projector, &input);
    assert_eq!(bits(&out[100..]), bits(&input[10..60]));
    // The rest is not covered by the mapping: black.
    assert!(out[..100].iter().all(|c| *c == [0.0; 3]));
}
