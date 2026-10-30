//! The scaling of `cells` regions against what the studio's `scaleSegments()`
//! answers (`tests/corpus/scale_segments.json`, made by
//! `scripts/export_builtins.mjs --scale`; ADR-0038 D20's scaling check).

use std::path::PathBuf;

use folo_library::scale::{scale_cells, scale_layers};
use serde_json::{Value, json};

fn corpus() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/scale_segments.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn cells_scale_as_scale_segments_does() {
    let corpus = corpus();
    let rows = corpus["rows"].as_array().unwrap();
    assert!(rows.len() > 5000);
    for row in rows {
        let [from, to, start, end, first, last] =
            [0, 1, 2, 3, 4, 5].map(|i| row[i].as_i64().unwrap());
        assert_eq!(
            scale_cells(start, end, from, to),
            (first, last),
            "{start}-{end} of {from} on {to}"
        );
    }
}

#[test]
fn layers_scale_as_scale_segments_does() {
    let corpus = corpus();
    for row in corpus["rows"].as_array().unwrap().iter().step_by(7) {
        let [from, to, start, end, first, last] =
            [0, 1, 2, 3, 4, 5].map(|i| row[i].as_i64().unwrap());
        let mut layers = vec![json!({"region": {"kind": "cells", "start": start, "end": end}})];
        scale_layers(&mut layers, from, to);
        let expected = if from == to {
            json!({"kind": "cells", "start": start, "end": end})
        } else {
            json!({"kind": "cells", "start": first, "end": last})
        };
        assert_eq!(
            layers[0]["region"], expected,
            "{start}-{end} of {from} on {to}"
        );
    }
}
