//! Projection golden images: surfaces and what the kernels, edge policies and blends make of them,
//! stacked into PNGs in `tests/golden/projection/`.
//!
//! The images are compared as they are. To write them (a new case, or behaviour meant to change),
//! run with `FOLO_BLESS=1`; the changed images then show in the diff for a person to review.

// The values are small, so casts are exact; the pixels are rounded on purpose.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

use common::{Map, bare_strip, canvas, grid, line, mappings, pixels, plane, strip, view, zone};
use folo_geometry::{CpuProjector, Projector};
use serde_json::{Value, json};

/// Pixels between rows of a sheet, and what pads a row shorter than the sheet.
const GAP: usize = 2;
const BACKDROP: [u8; 3] = [40, 40, 40];

/// Images stacked top to bottom, left-aligned, on a dark backdrop.
struct Sheet {
    width: usize,
    rows: Vec<(usize, usize, Vec<u8>)>,
}

impl Sheet {
    fn new(width: usize) -> Self {
        Self {
            width,
            rows: Vec::new(),
        }
    }

    /// Adds `colours` (`size` wide and high) with every colour `scale` pixels across and down.
    fn add(&mut self, colours: &[[f32; 3]], size: (usize, usize), scale: (usize, usize)) {
        let (w, h) = (size.0 * scale.0, size.1 * scale.1);
        assert!(
            w <= self.width,
            "a row of {w} pixels on a sheet of {}",
            self.width
        );
        assert_eq!(colours.len(), size.0 * size.1);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let c = colours[(y / scale.1) * size.0 + x / scale.0];
                rgb.extend(c.map(|v| (v + 0.5).clamp(0.0, 255.0) as u8));
            }
        }
        self.rows.push((w, h, rgb));
    }

    fn image(&self) -> (usize, usize, Vec<u8>) {
        let height: usize =
            self.rows.iter().map(|r| r.1).sum::<usize>() + GAP * self.rows.len().saturating_sub(1);
        let mut rgb = BACKDROP.repeat(self.width * height);
        let mut top = 0;
        for (w, h, row) in &self.rows {
            for y in 0..*h {
                let at = ((top + y) * self.width) * 3;
                rgb[at..at + w * 3].copy_from_slice(&row[y * w * 3..(y + 1) * w * 3]);
            }
            top += h + GAP;
        }
        (self.width, height, rgb)
    }
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/projection")
        .join(format!("{name}.png"))
}

/// Compares the sheet with `tests/golden/projection/<name>.png`, or writes it with `FOLO_BLESS=1`.
fn check(name: &str, sheet: &Sheet) {
    let (width, height, rgb) = sheet.image();
    let path = golden_path(name);
    if std::env::var_os("FOLO_BLESS").is_some_and(|v| v == "1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(&path).unwrap()),
            width as u32,
            height as u32,
        );
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&rgb).unwrap();
        writer.finish().unwrap();
        return;
    }
    let file = File::open(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}. A new case? Run the tests with FOLO_BLESS=1 and review the image.",
            path.display()
        )
    });
    let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
    let mut want = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut want).unwrap();
    assert_eq!(
        (
            info.width as usize,
            info.height as usize,
            info.color_type,
            info.bit_depth
        ),
        (width, height, png::ColorType::Rgb, png::BitDepth::Eight),
        "{name}: the golden is another size or format"
    );
    want.truncate(info.buffer_size());
    if want != rgb {
        let at = want.iter().zip(&rgb).position(|(a, b)| a != b).unwrap() / 3;
        panic!(
            "{name}: the projection changed, first at pixel ({}, {}). If that is meant, run with \
             FOLO_BLESS=1 and review the image.",
            at % width,
            at / width
        );
    }
}

/// One surface frame to project: its id, size and pixels.
type Frame<'a> = (&'a str, (usize, usize), &'a [[f32; 3]]);

fn run(
    zone: &folo_protocol::generated::Zone,
    target: &str,
    frames: &[Frame<'_>],
    n: usize,
) -> Vec<[f32; 3]> {
    let mut projector = CpuProjector::new();
    let prepared = projector.prepare(0, zone);
    assert!(
        prepared.issues.iter().all(|i| !i.is_error()),
        "{:?}",
        prepared.issues
    );
    let views: Vec<_> = frames
        .iter()
        .map(|(id, size, px)| (*id, view(*size, px)))
        .collect();
    let mut out = vec![[0.0; 3]; n];
    projector
        .project(&zone.id, target, &views, &mut out)
        .unwrap();
    out
}

/// A card whose every cell is different, with a smooth part and a sharp part.
fn card(size: (usize, usize)) -> Vec<[f32; 3]> {
    pixels(size, |x, y| {
        let (fx, fy) = (
            x as f32 / (size.0 - 1) as f32,
            y as f32 / (size.1 - 1) as f32,
        );
        let checker = if (x / 2 + y / 2) % 2 == 0 { 1.0 } else { 0.25 };
        [
            255.0 * fx * checker,
            255.0 * fy,
            255.0 * (1.0 - fx) * (0.5 + 0.5 * checker),
        ]
    })
}

fn floor(metres: (f64, f64)) -> Value {
    plane([0.0; 3], [metres.0, 0.0, 0.0], [0.0, metres.1, 0.0])
}

#[test]
fn kernels_on_a_grid_target() {
    // An 8 × 8 card on an 8 m square floor and a 4 × 4 grid of lights over all of it.
    let card = card((8, 8));
    let mut sheet = Sheet::new(64);
    sheet.add(&card, (8, 8), (8, 8));
    for (kernel, radius) in [
        ("nearest", 0.5),
        ("bilinear", 0.5),
        ("area", 0.5),
        ("area", 1.0),
    ] {
        let z = zone(
            json!([canvas("c", (8, 8), &floor((8.0, 8.0)))]),
            json!([grid("t", (4, 4), &floor((8.0, 8.0)), &json!({}))]),
            mappings(&[Map::new("m", "c", "t", "plane")
                .kernel(kernel)
                .radius(radius)]),
        );
        sheet.add(&run(&z, "t", &[("c", (8, 8), &card)], 16), (4, 4), (16, 16));
    }
    check("grid-kernels", &sheet);
}

#[test]
fn the_area_kernel_over_64_cells_is_sampled() {
    // A 32 × 32 card read by 4 × 4 lights: radius 0.5 is a box of 8 × 8 cells, exactly the 64 taps
    // allowed, and radius 1 a box of 16 × 16 cells, read at 8 × 8 samples.
    let card = card((32, 32));
    let mut sheet = Sheet::new(64);
    sheet.add(&card, (32, 32), (2, 2));
    for radius in [0.5, 1.0] {
        let z = zone(
            json!([canvas("c", (32, 32), &floor((32.0, 32.0)))]),
            json!([grid("t", (4, 4), &floor((32.0, 32.0)), &json!({}))]),
            mappings(&[Map::new("m", "c", "t", "plane").radius(radius)]),
        );
        let mut projector = CpuProjector::new();
        projector.prepare(0, &z);
        let taps = projector.plans("z", "t").unwrap()[0].plan.max_taps();
        assert_eq!(taps, 64, "radius {radius}");
        sheet.add(
            &run(&z, "t", &[("c", (32, 32), &card)], 16),
            (4, 4),
            (16, 16),
        );
    }
    check("area-cap", &sheet);
}

#[test]
fn grid_wiring_orders() {
    // A 4 × 3 card, one colour per cell, read by a grid of the same size: the output is the
    // lights in the order the wire visits them.
    let card = pixels((4, 3), |x, y| {
        let i = (y * 4 + x) as f32;
        [
            255.0 * (i / 11.0),
            255.0 * (1.0 - i / 11.0),
            if (x + y) % 2 == 0 { 255.0 } else { 0.0 },
        ]
    });
    let mut sheet = Sheet::new(64);
    sheet.add(&card, (4, 3), (16, 16));
    let wirings = [
        json!({"order": "row", "serpentine": false, "start": "top_left"}),
        json!({"order": "row", "serpentine": true, "start": "top_left"}),
        json!({"order": "column", "serpentine": true, "start": "bottom_right"}),
        json!({"order": "column", "serpentine": false, "start": "top_right"}),
    ];
    for wiring in wirings {
        let z = zone(
            json!([canvas("c", (4, 3), &floor((4.0, 3.0)))]),
            json!([grid("t", (4, 3), &floor((4.0, 3.0)), &wiring)]),
            mappings(&[Map::new("m", "c", "t", "plane").kernel("nearest")]),
        );
        sheet.add(&run(&z, "t", &[("c", (4, 3), &card)], 12), (12, 1), (5, 12));
    }
    check("grid-wiring", &sheet);
}

#[test]
fn lines_on_a_canvas_and_past_its_edge() {
    let card = card((8, 8));
    let mut sheet = Sheet::new(64);
    sheet.add(&card, (8, 8), (8, 8));
    // A diagonal of 16 lights across the card.
    let diagonal = line("t", 16, &[[0.5, 0.5, 0.0], [7.5, 7.5, 0.0]], &[]);
    for kernel in ["nearest", "bilinear", "area"] {
        let z = zone(
            json!([canvas("c", (8, 8), &floor((8.0, 8.0)))]),
            json!([diagonal.clone()]),
            mappings(&[Map::new("m", "c", "t", "plane").kernel(kernel)]),
        );
        sheet.add(&run(&z, "t", &[("c", (8, 8), &card)], 16), (16, 1), (4, 10));
    }
    // 16 lights along a line that starts 3 m left of the card and ends 3 m right of it.
    let across = line("t", 16, &[[-3.0, 4.0, 0.0], [11.0, 4.0, 0.0]], &[]);
    for edge in ["clamp", "none"] {
        for kernel in ["nearest", "bilinear", "area"] {
            let z = zone(
                json!([canvas("c", (8, 8), &floor((8.0, 8.0)))]),
                json!([across.clone()]),
                mappings(&[Map::new("m", "c", "t", "plane").kernel(kernel).edge(edge)]),
            );
            sheet.add(&run(&z, "t", &[("c", (8, 8), &card)], 16), (16, 1), (4, 10));
        }
    }
    check("line-on-canvas", &sheet);
}

#[test]
fn a_line_following_a_strips_path() {
    // A strip of 32 cells along an L of 12 m; 30 lights along a path beside it, longer at both ends.
    let ramp = pixels((32, 1), |x, _| {
        let t = x as f32 / 31.0;
        [
            255.0 * t,
            255.0 * (1.0 - (2.0 * t - 1.0).abs()),
            255.0 * (1.0 - t),
        ]
    });
    let path = [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [8.0, 4.0, 0.0]];
    let lights = line(
        "t",
        30,
        &[[-1.0, 1.0, 0.0], [9.0, 1.0, 0.0], [9.0, 5.0, 0.0]],
        &[],
    );
    let mut sheet = Sheet::new(96);
    sheet.add(&ramp, (32, 1), (3, 12));
    for (kernel, edge) in [
        ("nearest", "clamp"),
        ("bilinear", "clamp"),
        ("area", "clamp"),
        ("area", "none"),
    ] {
        let z = zone(
            json!([strip("s", 32, &path)]),
            json!([lights.clone()]),
            mappings(&[Map::new("m", "s", "t", "path").kernel(kernel).edge(edge)]),
        );
        sheet.add(
            &run(&z, "t", &[("s", (32, 1), &ramp)], 30),
            (30, 1),
            (3, 12),
        );
    }
    check("strip-path", &sheet);
}

#[test]
fn manual_ranges_resample_and_reverse() {
    // Alternating red and blue cells with a white one, so an average and a pick look different.
    let stripes = pixels((16, 1), |x, _| match x {
        7 => [255.0; 3],
        x if x % 2 == 0 => [255.0, 0.0, 0.0],
        _ => [0.0, 0.0, 255.0],
    });
    let mut sheet = Sheet::new(96);
    sheet.add(&stripes, (16, 1), (6, 12));
    let cases: [(u32, &str, &[common::Range]); 8] = [
        (8, "area", &[((0, 16), (0, 8), false)]),
        (8, "nearest", &[((0, 16), (0, 8), false)]),
        (8, "bilinear", &[((0, 16), (0, 8), false)]),
        (32, "area", &[((0, 16), (0, 32), false)]),
        (32, "nearest", &[((0, 16), (0, 32), false)]),
        (32, "bilinear", &[((0, 16), (0, 32), false)]),
        (8, "area", &[((0, 16), (0, 8), true)]),
        (
            8,
            "area",
            &[((0, 8), (0, 4), false), ((8, 8), (4, 4), true)],
        ),
    ];
    for (elements, kernel, ranges) in cases {
        let z = zone(
            json!([bare_strip("s", 16)]),
            json!([line("t", elements, &[], &[])]),
            mappings(&[Map::new("m", "s", "t", "manual")
                .kernel(kernel)
                .ranges(ranges)]),
        );
        let n = elements as usize;
        sheet.add(
            &run(&z, "t", &[("s", (16, 1), &stripes)], n),
            (n, 1),
            (96 / n, 12),
        );
    }
    check("manual-resample", &sheet);
}

#[test]
fn priority_and_blend_of_several_mappings() {
    let a = pixels((16, 1), |x, _| [255.0 * x as f32 / 15.0, 60.0, 0.0]);
    let b = pixels((16, 1), |x, _| {
        if x % 4 < 2 {
            [0.0, 160.0, 255.0]
        } else {
            [0.0, 0.0, 90.0]
        }
    });
    let mut sheet = Sheet::new(96);
    sheet.add(&a, (16, 1), (6, 12));
    sheet.add(&b, (16, 1), (6, 12));
    let frames: [Frame<'_>; 2] = [("a", (16, 1), &a), ("b", (16, 1), &b)];
    let under = || Map::new("ma", "a", "t", "manual").ranges(&[((0, 16), (0, 16), false)]);
    let over = || Map::new("mb", "b", "t", "manual").ranges(&[((4, 8), (4, 8), false)]);
    let cases = [
        // Max of both where they overlap, whatever the priorities.
        mappings(&[under(), over()]),
        // `b` above `a`, painted over it.
        mappings(&[under().blend("alpha"), over().blend("alpha").priority(1)]),
        // `a` above `b`: `a` covers all of it.
        mappings(&[under().blend("alpha").priority(1), over().blend("alpha")]),
        // Cells 0 to 8 of `b` spread over 16 lights with the `none` edge: its ends are partly
        // transparent, so `a` shows through a little.
        mappings(&[
            under().blend("alpha"),
            Map::new("mb", "b", "t", "manual")
                .kernel("bilinear")
                .edge("none")
                .blend("alpha")
                .priority(1)
                .ranges(&[((0, 8), (0, 16), false)]),
        ]),
    ];
    for maps in cases {
        let z = zone(
            json!([bare_strip("a", 16), bare_strip("b", 16)]),
            json!([line("t", 16, &[], &[])]),
            maps,
        );
        sheet.add(&run(&z, "t", &frames, 16), (16, 1), (6, 12));
    }
    check("blend-priority", &sheet);
}
