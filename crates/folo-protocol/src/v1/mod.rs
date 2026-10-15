//! Version 1 (today's `studio.json`): a port of `app/studio_config.py`'s
//! `sanitize_config()`, statement by statement, so that it gives the same
//! output on the RB-01 corpus (`tests/corpus/sanitize_config.json`). Version 1
//! is looser about types than the v2 clamper: it reads numbers from strings,
//! turns any value into a name with Python's `str()`, and so on (`py`).

mod py;
#[rustfmt::skip]
#[allow(clippy::unreadable_literal)]
mod pyunicode;

use serde_json::{Map, Number, Value, json};

use crate::number::{self, py_round};

/// The Unicode version of the Python the tables came from (`scripts/pyunicode.py`).
pub use pyunicode::UNICODE_VERSION as PYTHON_UNICODE_VERSION;

const MODES: [&str; 10] = [
    "fill", "pulse", "spectrum", "shift", "scroll", "drops", "scan", "wipe", "flow", "fire",
];
const DIRECTIONS: [&str; 4] = ["forward", "reverse", "center", "edges"];
const TRIGGERS: [&str; 3] = ["level", "hits", "beat"];
const BEAT_DIVISIONS: [f64; 4] = [0.5, 1.0, 2.0, 4.0];
const HITS_MAX_DB: f64 = 40.0;
const MAX_BEAT_OFFSET: f64 = 200.0;
const REACTIVE_MODES: [&str; 3] = ["on", "auto", "off"];
const AMBIENT_STYLES: [&str; 2] = ["solid", "breathe"];
const MAX_IDLE_AFTER: f64 = 600.0;
const MAX_GAIN: f64 = 24.0;
const MIN_HZ: f64 = 10.0;
const MAX_HZ: f64 = 24000.0;
const MIN_DB: f64 = -120.0;
const MAX_SPEED: f64 = 2000.0;
const MAX_SEGMENTS: usize = 64;
const MAX_LANES: f64 = 16.0;
const MAX_LEDS: f64 = 2000.0;
const MAX_PRESETS: usize = 60;
const MAX_TAGS: usize = 16;
const MAX_TAG_LENGTH: usize = 24;

/// A new id, as `studio_config.new_id()` makes them.
pub type NewId<'a> = &'a mut dyn FnMut() -> String;

/// `_float(value, default, lo, hi)`.
fn float(value: Option<&Value>, default: f64, lo: f64, hi: f64) -> f64 {
    match value.and_then(py::float) {
        Some(number) if number.is_finite() => lo.max(hi.min(number)),
        _ => default,
    }
}

/// `_int(value, default, lo, hi)`: `round()` ties to even.
#[allow(clippy::cast_possible_truncation)]
fn int(value: Option<&Value>, default: f64, lo: f64, hi: f64) -> i64 {
    float(value, default, lo, hi).round_ties_even() as i64
}

/// `_color(value, default)`: `re.match('^#[0-9a-fA-F]{6}$')`, whose `$` also
/// matches before a final newline, then upper-cased.
fn color(value: Option<&Value>) -> Option<String> {
    let Some(Value::String(s)) = value else {
        return None;
    };
    let body = s.strip_suffix('\n').unwrap_or(s);
    let ok = body.len() == 7
        && body.starts_with('#')
        && body[1..].bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| s.to_ascii_uppercase())
}

fn get<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key)
}

/// `str(value or default)`.
fn str_or(value: Option<&Value>, default: &str) -> String {
    match value {
        Some(v) if py::truthy(v) => py::str(v),
        _ => default.to_owned(),
    }
}

/// `bool(object.get(key, default))`.
fn flag(object: &Map<String, Value>, key: &str, default: bool) -> bool {
    object.get(key).map_or(default, py::truthy)
}

fn section<'a>(cfg: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    cfg.get(key).and_then(Value::as_object)
}

static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);

fn f(x: f64) -> Value {
    Value::Number(number::from_f64(x))
}

/// `sanitize_segment(seg, led_count)`. Long, because it follows the Python
/// function statement by statement.
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
fn segment(seg: &Map<String, Value>, led_count: i64, new_id: NewId<'_>) -> Value {
    let last = (led_count - 1).max(0) as f64;
    let mut start = int(get(seg, "start"), 0.0, 0.0, last);
    let mut end = int(get(seg, "end"), last, 0.0, last);
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }

    let mut freq_lo = float(get(seg, "freq_lo"), 60.0, MIN_HZ, MAX_HZ);
    let mut freq_hi = float(get(seg, "freq_hi"), 250.0, MIN_HZ, MAX_HZ);
    if freq_hi < freq_lo {
        std::mem::swap(&mut freq_lo, &mut freq_hi);
    }
    if freq_hi < freq_lo * 1.01 {
        freq_hi = MAX_HZ.min(freq_lo * 1.01);
        freq_lo = freq_hi / 1.01;
    }

    let trigger = get(seg, "trigger")
        .and_then(|t| py::is_one_of(t, &TRIGGERS))
        .unwrap_or("level");
    let (threshold, mut ceiling) = if trigger == "hits" {
        (
            float(get(seg, "threshold"), 3.0, 0.0, HITS_MAX_DB - 1.0),
            float(get(seg, "ceiling"), 10.0, 1.0, HITS_MAX_DB),
        )
    } else {
        (
            float(get(seg, "threshold"), -60.0, MIN_DB, -1.0),
            float(get(seg, "ceiling"), -25.0, MIN_DB + 1.0, 0.0),
        )
    };
    if ceiling < threshold + 1.0 {
        ceiling = threshold + 1.0;
    }

    // `seg.get('palette') or []`, read only from a list, a dict's keys or a string's characters
    let colours: Vec<Value> = match get(seg, "palette") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(map)) => map.keys().map(|k| Value::String(k.clone())).collect(),
        Some(Value::String(s)) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        _ => Vec::new(),
    };
    let mut palette: Vec<Value> = colours
        .iter()
        .filter_map(|c| color(Some(c)))
        .filter(|c| !c.is_empty())
        .map(Value::String)
        .collect();
    palette.truncate(16);
    if palette.is_empty() {
        palette.push(json!("#FFFFFF"));
    }

    let mode = get(seg, "mode")
        .and_then(|m| py::is_one_of(m, &MODES))
        .unwrap_or("fill");
    let direction = get(seg, "direction")
        .and_then(|d| py::is_one_of(d, &DIRECTIONS))
        .unwrap_or("forward");
    let beats = float(get(seg, "beats"), 1.0, 0.0, 16.0);
    let beats = if BEAT_DIVISIONS.contains(&beats) {
        beats
    } else {
        1.0
    };

    let id = match get(seg, "id") {
        Some(v) if py::truthy(v) => py::str(v),
        _ => new_id(),
    };
    let mut out = Map::new();
    out.insert("id".into(), json!(py::cut(&id, 32)));
    out.insert(
        "name".into(),
        json!(py::cut(&str_or(get(seg, "name"), "Segment"), 40)),
    );
    out.insert("enabled".into(), json!(flag(seg, "enabled", true)));
    out.insert("solo".into(), json!(flag(seg, "solo", false)));
    out.insert("start".into(), json!(start));
    out.insert("end".into(), json!(end));
    out.insert("freq_lo".into(), f(py_round(freq_lo, 2)));
    out.insert("freq_hi".into(), f(py_round(freq_hi, 2)));
    out.insert("trigger".into(), json!(trigger));
    out.insert("beats".into(), f(beats));
    out.insert("threshold".into(), f(py_round(threshold, 1)));
    out.insert("ceiling".into(), f(py_round(ceiling, 1)));
    out.insert("gain".into(), f(float(get(seg, "gain"), 1.0, 0.1, 8.0)));
    out.insert("curve".into(), f(float(get(seg, "curve"), 1.0, 0.2, 5.0)));
    out.insert(
        "attack".into(),
        f(float(get(seg, "attack"), 15.0, 0.0, 2000.0)),
    );
    out.insert(
        "release".into(),
        f(float(get(seg, "release"), 200.0, 0.0, 5000.0)),
    );
    out.insert("floor".into(), f(float(get(seg, "floor"), 0.0, 0.0, 1.0)));
    out.insert("mode".into(), json!(mode));
    out.insert("direction".into(), json!(direction));
    out.insert("peak".into(), json!(flag(seg, "peak", false)));
    out.insert(
        "speed".into(),
        f(float(get(seg, "speed"), 120.0, 0.0, MAX_SPEED)),
    );
    out.insert(
        "width".into(),
        f(float(get(seg, "width"), 20.0, 1.0, 100.0)),
    );
    out.insert("palette".into(), Value::Array(palette));
    out.insert(
        "lane".into(),
        json!(int(get(seg, "lane"), -1.0, -1.0, MAX_LANES - 1.0)),
    );
    Value::Object(out)
}

/// `_sanitize_segments(segments, led_count)`.
fn segments(items: &[Value], led_count: i64, new_id: NewId<'_>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut clean = Vec::new();
    for item in items.iter().take(MAX_SEGMENTS) {
        let Value::Object(seg) = item else { continue };
        let mut seg = segment(seg, led_count, new_id);
        let id = seg["id"].as_str().unwrap_or_default().to_owned();
        let id = if seen.contains(&id) {
            let fresh = new_id();
            seg["id"] = json!(fresh);
            fresh
        } else {
            id
        };
        seen.insert(id);
        clean.push(seg);
    }
    clean
}

/// `sanitize_tags(tags)`.
fn tags(value: Option<&Value>) -> Vec<Value> {
    let mut clean = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(Value::Array(items)) = value else {
        return clean;
    };
    for item in items {
        let Value::String(tag) = item else { continue };
        let tag = py::rstrip(&py::cut(&py::collapse_spaces(tag), MAX_TAG_LENGTH)).to_owned();
        let key = py::lower(&tag);
        if !tag.is_empty() && !seen.contains(&key) {
            seen.insert(key);
            clean.push(json!(tag));
        }
    }
    clean.truncate(MAX_TAGS);
    clean
}

/// `sanitize_preset(preset)`.
fn preset(preset: &Map<String, Value>, new_id: NewId<'_>) -> Value {
    let led_count = int(get(preset, "led_count"), 150.0, 1.0, MAX_LEDS);
    let items = match get(preset, "segments") {
        Some(Value::Array(items)) => items.as_slice(),
        _ => &[],
    };
    let id = match get(preset, "id") {
        Some(v) if py::truthy(v) => py::str(v),
        _ => new_id(),
    };
    let name = py::rstrip(&py::cut(py::strip(&str_or(get(preset, "name"), "")), 40)).to_owned();
    let mut out = Map::new();
    out.insert("id".into(), json!(py::cut(&id, 64)));
    out.insert(
        "name".into(),
        json!(if name.is_empty() {
            "My preset".to_owned()
        } else {
            name
        }),
    );
    out.insert(
        "desc".into(),
        json!(py::cut(&str_or(get(preset, "desc"), ""), 200)),
    );
    out.insert("tags".into(), Value::Array(tags(get(preset, "tags"))));
    out.insert("led_count".into(), json!(led_count));
    out.insert(
        "segments".into(),
        Value::Array(segments(items, led_count, new_id)),
    );
    Value::Object(out)
}

/// `sanitize_config(cfg, led_count)`: a complete, valid version-1 config.
/// `led_count` overrides the stored count (`None` reads `cfg['led_count']`).
#[must_use]
pub fn sanitize_config(cfg: &Value, led_count: Option<&Value>, new_id: NewId<'_>) -> Value {
    let cfg = cfg.as_object().unwrap_or(&EMPTY);
    let led_count = match led_count {
        Some(Value::Null) | None => get(cfg, "led_count"),
        given => given,
    };
    let led_count = int(led_count, 150.0, 1.0, MAX_LEDS);

    let audio = section(cfg, "audio").unwrap_or(&EMPTY);
    let hyperion = section(cfg, "hyperion").unwrap_or(&EMPTY);
    let ambient = section(cfg, "ambient").unwrap_or(&EMPTY);
    let beat = section(cfg, "beat").unwrap_or(&EMPTY);
    let playing = section(cfg, "now_playing").unwrap_or(&EMPTY);
    let segment_items = match get(cfg, "segments") {
        Some(Value::Array(items)) => items.as_slice(),
        _ => &[],
    };
    let clean = segments(segment_items, led_count, new_id);

    let preset_items = match get(cfg, "presets") {
        Some(Value::Array(items)) => items.as_slice(),
        _ => &[],
    };
    let mut presets: Vec<Value> = preset_items
        .iter()
        .take(MAX_PRESETS)
        .filter_map(Value::as_object)
        .map(|p| preset(p, new_id))
        .collect();
    let mut ids = std::collections::HashSet::new();
    for preset in &mut presets {
        let id = preset["id"].as_str().unwrap_or_default().to_owned();
        if ids.contains(&id) {
            let fresh = new_id();
            preset["id"] = json!(fresh);
            ids.insert(fresh);
        } else {
            ids.insert(id);
        }
    }

    let app = py::rstrip(&py::cut(
        &py::lower(py::strip(&str_or(get(audio, "app"), ""))),
        260,
    ))
    .to_owned();
    let offset = float(get(beat, "offset"), 0.0, -MAX_BEAT_OFFSET, MAX_BEAT_OFFSET);
    let reactive = get(cfg, "reactive")
        .and_then(|r| py::is_one_of(r, &REACTIVE_MODES))
        .unwrap_or("on");
    let style = get(ambient, "style")
        .and_then(|s| py::is_one_of(s, &AMBIENT_STYLES))
        .unwrap_or("solid");
    let app_mode = if get(audio, "app_mode") == Some(&json!("except")) {
        "except"
    } else {
        "only"
    };
    json!({
        "version": 1,
        "led_count": led_count,
        "audio": {
            "device": py::cut(&str_or(get(audio, "device"), "default"), 300),
            "app": app,
            "app_mode": app_mode,
            "gain": f(py_round(float(get(audio, "gain"), 0.0, -MAX_GAIN, MAX_GAIN), 1)),
            "auto_level": flag(audio, "auto_level", false),
        },
        "hyperion": {
            "enabled": flag(hyperion, "enabled", true),
            "host": py::cut(&str_or(get(hyperion, "host"), "localhost"), 255),
            "port": int(get(hyperion, "port"), 19444.0, 1.0, 65535.0),
            "priority": int(get(hyperion, "priority"), 100.0, 1.0, 253.0),
            "fps": int(get(hyperion, "fps"), 40.0, 5.0, 60.0),
        },
        "master": f(float(get(cfg, "master"), 1.0, 0.0, 1.0)),
        "background": color(get(cfg, "background")).unwrap_or_else(|| "#000000".to_owned()),
        "reactive": reactive,
        "ambient": {
            "color": color(get(ambient, "color")).unwrap_or_else(|| "#4B00FF".to_owned()),
            "style": style,
            "level": f(float(get(ambient, "level"), 0.35, 0.0, 1.0)),
            "idle_after": f(float(get(ambient, "idle_after"), 5.0, 1.0, MAX_IDLE_AFTER)),
        },
        "beat": {"offset": Value::Number(Number::from(round_int(offset)))},
        "now_playing": {
            "enabled": flag(playing, "enabled", true),
            "cover_colors": flag(playing, "cover_colors", false),
            "pause_ambient": flag(playing, "pause_ambient", true),
        },
        "segments": clean,
        "presets": presets,
        "default_preset": py::cut(&str_or(get(cfg, "default_preset"), ""), 64),
    })
}

#[allow(clippy::cast_possible_truncation)]
fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}
