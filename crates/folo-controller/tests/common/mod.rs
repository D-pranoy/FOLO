//! Helpers the output core's tests share: golden images, zones from JSON, frames and a clock.

#![allow(dead_code)]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use folo_controller::OutputCore;
use folo_protocol::frames::{Pixels, SurfaceFrame};
use folo_protocol::generated::{Zone, ZonesDocument};
use serde_json::{Value, json};

/// One step of the node's 60 Hz floor, in µs.
pub const STEP_US: u64 = 16_667;

pub fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// An 8-bit RGB image: one row of `width` cells per frame.
pub struct Image {
    pub width: usize,
    pub rows: Vec<Vec<u8>>,
}

pub fn read_png(path: &Path) -> Image {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    let stride = info.line_size;
    Image {
        width: info.width as usize,
        rows: buffer[..stride * info.height as usize]
            .chunks_exact(stride)
            .map(<[u8]>::to_vec)
            .collect(),
    }
}

/// A zone from JSON; what it leaves out takes the schema's defaults.
pub fn zone(value: Value) -> Zone {
    serde_json::from_value(value).expect("a valid zone")
}

/// The zones of a `zones.json`.
pub fn zones_of(path: &Path) -> Vec<Zone> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let document: ZonesDocument = serde_json::from_str(&text).unwrap();
    document.zones
}

/// A zone like the migration makes: one strip surface of `leds` cells fed by `local`, one RGB
/// line target of `leds` elements on the instance `instance`, and a manual 1:1 mapping.
pub fn strip_zone(id: &str, leds: u32, instance: &str, extra: &Value) -> Zone {
    let mut zone = json!({
        "id": id,
        "reactive": "on",
        "surfaces": [{
            "id": "main", "kind": "strip", "resolution": {"width": leds, "height": 1},
            "role": "strip.main", "sources": ["local"],
        }],
        "targets": [{
            "id": format!("{id}-strip"), "kind": "line",
            "provider": {"instance": instance, "address": {"start": 0, "count": leds}},
            "geometry": {"kind": "line", "count": leds},
        }],
        "mappings": [{
            "id": "m", "surface": "main", "target": format!("{id}-strip"), "method": "manual",
            "ranges": [{"cells": {"start": 0, "count": leds}, "elements": {"start": 0, "count": leds}}],
        }],
    });
    if let (Some(zone), Some(extra)) = (zone.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            zone.insert(key.clone(), value.clone());
        }
    }
    self::zone(zone)
}

/// An rgb8 frame of a strip.
pub fn strip_frame(sequence: u32, bytes: &[u8]) -> SurfaceFrame {
    let width = u16::try_from(bytes.len() / 3).unwrap();
    SurfaceFrame::new(0, sequence, 0, width, 1, Pixels::Rgb8(bytes.to_vec()))
}

/// A core with these zones at revision 1.
pub fn core_of(zones: &[Zone]) -> OutputCore {
    let mut core = OutputCore::new();
    core.configure(1, zones);
    core
}
