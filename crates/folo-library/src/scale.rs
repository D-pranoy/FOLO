//! Scaling `cells` regions to another strip length, as the studio scales
//! saved presets (`scaleSegments()` in `web/studio.js`, ADR-0038 D14).

use serde_json::Value;

/// JavaScript's `Math.round` for the values regions take: halves go up.
#[allow(clippy::cast_possible_truncation)] // positions are far below i64's range
fn round_half_up(x: f64) -> i64 {
    let floor = x.floor();
    let whole = floor as i64;
    if x - floor >= 0.5 { whole + 1 } else { whole }
}

/// The cells `start` to `end` of a strip of `from` cells, on a strip of `to`.
#[must_use]
pub fn scale_cells(start: i64, end: i64, from: i64, to: i64) -> (i64, i64) {
    #[allow(clippy::cast_precision_loss)] // cell counts are at most 2000
    let ratio = to as f64 / from as f64;
    #[allow(clippy::cast_precision_loss)]
    let scaled = |x: i64| round_half_up(x as f64 * ratio);
    let first = (to - 1).min(scaled(start));
    let last = (to - 1).min(first.max(scaled(end + 1) - 1));
    (first, last)
}

/// Scales the `cells` regions of `layers` from a strip of `from` cells to
/// one of `to`; other regions, and a scene made for no length (`from` 0) or
/// for this one, stay as they are.
pub fn scale_layers(layers: &mut [Value], from: i64, to: i64) {
    if from == 0 || from == to {
        return;
    }
    for layer in layers {
        let region = &mut layer["region"];
        if region["kind"] != "cells" {
            continue;
        }
        let start = region["start"].as_i64().unwrap_or(0);
        let end = region["end"].as_i64().unwrap_or(0);
        let (first, last) = scale_cells(start, end, from, to);
        region["start"] = Value::from(first);
        region["end"] = Value::from(last);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn halves_round_up_like_math_round() {
        assert_eq!(round_half_up(2.5), 3);
        assert_eq!(round_half_up(2.4999), 2);
        assert_eq!(round_half_up(0.0), 0);
    }

    #[test]
    fn a_third_of_a_strip_scales_with_it() {
        assert_eq!(scale_cells(0, 49, 150, 30), (0, 9));
        assert_eq!(scale_cells(50, 99, 150, 30), (10, 19));
        assert_eq!(scale_cells(100, 149, 150, 30), (20, 29));
    }

    #[test]
    fn ends_stay_inside_the_strip_and_after_the_start() {
        assert_eq!(scale_cells(10, 10, 100, 1), (0, 0));
        let (first, last) = scale_cells(99, 99, 100, 10);
        assert!(first <= last && last <= 9);
    }

    #[test]
    fn only_cells_regions_of_a_scene_with_a_length_change() {
        let mut layers = vec![
            json!({"region": {"kind": "cells", "start": 0, "end": 49}}),
            json!({"region": {"kind": "relative", "start": 0.0, "end": 0.5}}),
        ];
        scale_layers(&mut layers, 100, 100);
        assert_eq!(layers[0]["region"]["end"], 49);
        scale_layers(&mut layers, 0, 10);
        assert_eq!(layers[0]["region"]["end"], 49);
        scale_layers(&mut layers, 100, 10);
        assert_eq!(
            layers[0]["region"],
            json!({"kind": "cells", "start": 0, "end": 4})
        );
        assert_eq!(layers[1]["region"]["end"], 0.5);
    }
}
