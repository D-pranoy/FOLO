//! Helpers the integration tests share: zones from JSON, and surfaces to project.

#![allow(dead_code)]

use folo_geometry::SurfaceView;
use folo_protocol::generated::{Zone, ZonesDocument};
use serde_json::{Value, json};

/// A zone from the parts of its JSON that matter; the rest takes the schema's defaults.
pub fn zone(surfaces: Value, targets: Value, mappings: Value) -> Zone {
    let mut zone = serde_json::Map::new();
    zone.insert("id".into(), json!("z"));
    zone.insert("surfaces".into(), surfaces);
    zone.insert("targets".into(), targets);
    zone.insert("mappings".into(), mappings);
    serde_json::from_value(Value::Object(zone)).expect("a valid zone")
}

/// A line target of `count` elements along `path` (`[[x, y, z], …]`), or at `positions`.
pub fn line(id: &str, count: u32, path: &[[f64; 3]], positions: &[[f64; 3]]) -> Value {
    let point = |p: &[f64; 3]| json!({"x": p[0], "y": p[1], "z": p[2]});
    json!({
        "id": id, "kind": "line",
        "geometry": {
            "kind": "line", "count": count,
            "path": path.iter().map(point).collect::<Vec<_>>(),
            "positions": positions.iter().map(point).collect::<Vec<_>>(),
        },
    })
}

/// A grid target on a plane, in wiring order.
pub fn grid(id: &str, (width, height): (u32, u32), plane: &Value, wiring: &Value) -> Value {
    json!({
        "id": id, "kind": "grid",
        "geometry": {"kind": "grid", "width": width, "height": height, "plane": plane, "wiring": wiring},
    })
}

pub fn plane(origin: [f64; 3], u: [f64; 3], v: [f64; 3]) -> Value {
    let point = |p: [f64; 3]| json!({"x": p[0], "y": p[1], "z": p[2]});
    json!({"origin": point(origin), "u": point(u), "v": point(v)})
}

/// A canvas surface on a plane.
pub fn canvas(id: &str, (width, height): (u32, u32), plane: &Value) -> Value {
    json!({
        "id": id, "kind": "canvas",
        "resolution": {"width": width, "height": height},
        "placement": {"kind": "plane", "plane": plane},
    })
}

/// A strip surface along a path.
pub fn strip(id: &str, cells: u32, path: &[[f64; 3]]) -> Value {
    let point = |p: &[f64; 3]| json!({"x": p[0], "y": p[1], "z": p[2]});
    json!({
        "id": id, "kind": "strip",
        "resolution": {"width": cells, "height": 1},
        "placement": {"kind": "path", "points": path.iter().map(point).collect::<Vec<_>>()},
    })
}

/// A strip surface with no placement, for manual mappings.
pub fn bare_strip(id: &str, cells: u32) -> Value {
    json!({"id": id, "kind": "strip", "resolution": {"width": cells, "height": 1}})
}

/// A manual range: `((cell start, count), (element start, count), reversed)`.
pub type Range = ((u32, u32), (u32, u32), bool);

pub struct Map<'a> {
    pub id: &'a str,
    pub surface: &'a str,
    pub target: &'a str,
    pub method: &'a str,
    pub kernel: &'a str,
    pub radius: f64,
    pub edge: &'a str,
    pub priority: i64,
    pub blend: &'a str,
    pub ranges: Value,
}

impl<'a> Map<'a> {
    pub fn new(id: &'a str, surface: &'a str, target: &'a str, method: &'a str) -> Self {
        Self {
            id,
            surface,
            target,
            method,
            kernel: "area",
            radius: 0.5,
            edge: "clamp",
            priority: 0,
            blend: "max",
            ranges: json!([]),
        }
    }

    pub fn kernel(mut self, kernel: &'a str) -> Self {
        self.kernel = kernel;
        self
    }

    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    pub fn edge(mut self, edge: &'a str) -> Self {
        self.edge = edge;
        self
    }

    pub fn priority(mut self, priority: i64) -> Self {
        self.priority = priority;
        self
    }

    pub fn blend(mut self, blend: &'a str) -> Self {
        self.blend = blend;
        self
    }

    /// Manual ranges as `((cell start, count), (element start, count), reversed)`.
    pub fn ranges(mut self, ranges: &[Range]) -> Self {
        self.ranges = ranges
            .iter()
            .map(|(c, e, reversed)| {
                json!({
                    "cells": {"start": c.0, "count": c.1},
                    "elements": {"start": e.0, "count": e.1},
                    "reversed": reversed,
                })
            })
            .collect();
        self
    }

    pub fn json(&self) -> Value {
        json!({
            "id": self.id, "surface": self.surface, "target": self.target, "method": self.method,
            "kernel": {"type": self.kernel, "radius": self.radius},
            "edge": self.edge, "priority": self.priority, "blend": self.blend,
            "ranges": self.ranges,
        })
    }
}

pub fn mappings(maps: &[Map<'_>]) -> Value {
    Value::Array(maps.iter().map(Map::json).collect())
}

/// The first zone of the migrated default setup, the parity case.
pub fn migrated_zone() -> Zone {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/migration/default/out/zones.json"
    );
    let text = std::fs::read_to_string(path).expect("the migration golden");
    let document: ZonesDocument = serde_json::from_str(&text).expect("zones.json");
    document.zones.into_iter().next().expect("a zone")
}

/// A surface's pixels with a function of `(x, y)`.
pub fn pixels(
    (width, height): (usize, usize),
    mut f: impl FnMut(usize, usize) -> [f32; 3],
) -> Vec<[f32; 3]> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| f(x, y))
        .collect()
}

pub fn view(size: (usize, usize), pixels: &[[f32; 3]]) -> SurfaceView<'_> {
    SurfaceView::new(size.0, size.1, pixels).expect("pixels of the right count")
}

pub fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
}
