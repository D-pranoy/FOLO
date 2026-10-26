//! The bytes of the commands that go out on every frame, written as the legacy sender's Python
//! `json.dumps` writes them (`source/folo_source/legacy/hyperion.py`): `", "` and `": "`
//! separators, keys in the order the legacy code builds them, and non-ASCII text escaped. Hyperion
//! doesn't care, but the parity tests compare bytes ([parity](../../docs/architecture/pipeline.md)).

use std::fmt::Write as _;

/// Hyperion releases the colours this many milliseconds after the last frame
/// (`docs/integrations/hyperion.md`).
pub const DURATION_MS: u32 = 2000;

/// What Hyperion accepts as an origin: 4 to 20 characters (`schema-color.json`).
pub const MIN_ORIGIN_CHARS: usize = 4;
pub const MAX_ORIGIN_CHARS: usize = 20;

/// The shortest API token Hyperion looks at (`schema-authorize.json`); it doesn't answer a
/// shorter one at all.
pub const MIN_TOKEN_CHARS: usize = 36;

/// `json.dumps` of a string with `ensure_ascii`: everything but printable ASCII is escaped, and
/// characters beyond the Basic Multilingual Plane as surrogate pairs.
fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// The `color` command for one frame: the levels as bytes, the priority and the origin, with
/// `duration`, and the newline that ends it.
#[must_use]
pub fn color_line(rgb: &[u8], priority: u16, origin: &str) -> Vec<u8> {
    let mut line = String::with_capacity(96 + rgb.len() * 5);
    line.push_str("{\"command\": \"color\", \"color\": [");
    for (i, level) in rgb.iter().enumerate() {
        if i > 0 {
            line.push_str(", ");
        }
        let _ = write!(line, "{level}");
    }
    let _ = write!(line, "], \"priority\": {priority}, \"origin\": ");
    push_string(&mut line, origin);
    let _ = writeln!(line, ", \"duration\": {DURATION_MS}}}");
    line.into_bytes()
}

/// The `clear` command for a priority, as the legacy sender writes it when it lets go.
#[must_use]
pub fn clear_line(priority: u16) -> Vec<u8> {
    format!("{{\"command\": \"clear\", \"priority\": {priority}}}\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_color_line_is_what_python_writes() {
        let line = color_line(&[0, 255, 7], 100, "FOLO");
        assert_eq!(
            String::from_utf8(line).unwrap(),
            "{\"command\": \"color\", \"color\": [0, 255, 7], \"priority\": 100, \
             \"origin\": \"FOLO\", \"duration\": 2000}\n"
        );
    }

    #[test]
    fn a_clear_line_is_what_python_writes() {
        assert_eq!(
            clear_line(100),
            b"{\"command\": \"clear\", \"priority\": 100}\n"
        );
    }

    #[test]
    fn text_is_escaped_as_python_does() {
        let mut out = String::new();
        push_string(&mut out, "a\"b\\c\n\u{7f}\u{e9}\u{2603}\u{1f600}");
        let expected = ["\"a\\\"b\\\\c\\n", "\\u007f\\u00e9\\u2603\\ud83d\\ude00\""].concat();
        assert_eq!(out, expected);
    }
}
