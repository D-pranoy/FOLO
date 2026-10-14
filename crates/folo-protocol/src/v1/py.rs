//! Python's semantics for the JSON values the version-1 rules touch:
//! `float()`, `str()`/`repr()`, truthiness, `strip()`, `split()`, `lower()`
//! and `round()`. The Unicode facts come from Python itself (`pyunicode.rs`).

use std::fmt::Write as _;

use serde_json::{Number, Value};

use super::pyunicode;
use crate::number::Num;

fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let cp = u32::from(c);
    ranges
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `str.isspace()` of one character.
pub fn is_space(c: char) -> bool {
    in_ranges(pyunicode::SPACE, c)
}

fn is_printable(c: char) -> bool {
    in_ranges(pyunicode::PRINTABLE, c)
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `' '.join(s.split())`.
pub fn collapse_spaces(s: &str) -> String {
    s.split(is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `s[:n]`, counting code points.
pub fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `str.lower()`, with the final sigma rule.
pub fn lower(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '\u{3A3}' {
            out.push(if final_sigma(&chars, i) {
                '\u{3C2}'
            } else {
                '\u{3C3}'
            });
            continue;
        }
        let cp = u32::from(c);
        match pyunicode::LOWER.binary_search_by_key(&cp, |&(k, _)| k) {
            Ok(index) => out.push_str(pyunicode::LOWER[index].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `CPython`'s `handle_capital_sigma`.
fn final_sigma(chars: &[char], i: usize) -> bool {
    let cased = |c: char| in_ranges(pyunicode::CASED, c);
    let ignorable = |c: char| in_ranges(pyunicode::CASE_IGNORABLE, c);
    let before = chars[..i].iter().rev().find(|&&c| !ignorable(c));
    if !before.is_some_and(|&c| cased(c)) {
        return false;
    }
    let after = chars[i + 1..].iter().find(|&&c| !ignorable(c));
    after.is_none_or(|&c| !cased(c))
}

/// Python's truthiness of a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => match Num::of(n) {
            Num::Int(i) => i != 0,
            Num::Huge(_) => true,
            Num::Float(f) => f != 0.0, // NaN is truthy too
        },
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `float(value)`: `None` where Python raises (`TypeError`, `ValueError`, `OverflowError`).
pub fn float(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => {
            let text = n.to_string();
            let parsed: f64 = text.parse().ok()?;
            // float() of an int too big for a double overflows; a float literal is already one
            if parsed.is_infinite() && !text.contains(['.', 'e', 'E']) {
                None
            } else {
                Some(parsed)
            }
        }
        Value::String(s) => parse_float(s),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn decimal_value(c: char) -> Option<u8> {
    let cp = u32::from(c);
    pyunicode::DECIMAL
        .binary_search_by_key(&cp, |&(k, _)| k)
        .ok()
        .map(|i| pyunicode::DECIMAL[i].1)
}

/// `float(str)`: Unicode digits and spaces count, underscores between digits (PEP 515).
fn parse_float(s: &str) -> Option<f64> {
    // CPython first turns every decimal digit into ASCII and every space into ' '
    let mut ascii = String::with_capacity(s.len());
    for c in s.chars() {
        if is_space(c) {
            ascii.push(' ');
        } else if let Some(d) = decimal_value(c) {
            ascii.push(char::from(b'0' + d));
        } else if c.is_ascii() {
            ascii.push(c);
        } else {
            return None;
        }
    }
    let text = ascii.trim_matches(' ');
    let unsigned = text.trim_start_matches(['+', '-']);
    if text.len() - unsigned.len() > 1 {
        return None;
    }
    let negative = text.starts_with('-');
    let word = unsigned.to_ascii_lowercase();
    let special = match word.as_str() {
        "inf" | "infinity" => Some(f64::INFINITY),
        "nan" => Some(f64::NAN),
        _ => None,
    };
    if let Some(value) = special {
        return Some(if negative { -value } else { value });
    }
    let plain = remove_digit_underscores(unsigned)?;
    if !valid_decimal(&plain) {
        return None;
    }
    let value: f64 = plain.parse().ok()?;
    Some(if negative { -value } else { value })
}

/// Underscores are only allowed between two digits.
fn remove_digit_underscores(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            let between = i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !between {
                return None;
            }
        } else {
            out.push(char::from(b));
        }
    }
    Some(out)
}

/// digits [. digits] | . digits, then an optional exponent.
fn valid_decimal(s: &str) -> bool {
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (int, frac) = match mantissa.find('.') {
        Some(i) => (&mantissa[..i], Some(&mantissa[i + 1..])),
        None => (mantissa, None),
    };
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = digits(int)
        && frac.is_none_or(digits)
        && (!int.is_empty() || frac.is_some_and(|f| !f.is_empty()));
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}

/// `repr(float)`: the shortest digits that round-trip, positional for
/// exponents from -4 to 15, otherwise with an exponent of at least two digits.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let exp_form = format!("{x:e}");
    let (mantissa, exponent) = exp_form.split_once('e').expect("{:e} has an exponent");
    let exponent: i32 = exponent.parse().expect("an integer exponent");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    let decpt = exponent + 1;
    if (-3..=16).contains(&decpt) {
        let n = i32::try_from(digits.len()).unwrap_or(i32::MAX);
        let body = if decpt <= 0 {
            format!(
                "0.{}{}",
                "0".repeat(usize::try_from(-decpt).unwrap_or(0)),
                digits
            )
        } else if decpt >= n {
            format!(
                "{}{}.0",
                digits,
                "0".repeat(usize::try_from(decpt - n).unwrap_or(0))
            )
        } else {
            let (a, b) = digits.split_at(usize::try_from(decpt).unwrap_or(0));
            format!("{a}.{b}")
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        let esign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{esign}{:02}", exponent.abs())
    }
}

fn int_repr(n: &Number) -> String {
    let text = n.to_string();
    // JSON allows -0; Python's int has no negative zero
    if text.trim_start_matches('-').bytes().all(|b| b == b'0') {
        "0".to_owned()
    } else {
        text
    }
}

fn number_repr(n: &Number) -> String {
    let text = n.to_string();
    if text.contains(['.', 'e', 'E']) {
        float_repr(text.parse().unwrap_or(f64::NAN))
    } else {
        int_repr(n)
    }
}

/// `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if u32::from(c) < 0x20 || u32::from(c) == 0x7F => {
                {
                    let _ = write!(out, "\\x{:02x}", u32::from(c));
                };
            }
            c if u32::from(c) < 0x7F || is_printable(c) => out.push(c),
            c if u32::from(c) < 0x100 => {
                let _ = write!(out, "\\x{:02x}", u32::from(c));
            }
            c if u32::from(c) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => {
                let _ = write!(out, "\\U{:08x}", u32::from(c));
            }
        }
    }
    out.push(quote);
    out
}

/// `repr(value)` of a value inside a list or dict.
fn repr(value: &Value) -> String {
    match value {
        Value::String(s) => str_repr(s),
        other => str(other),
    }
}

/// `str(value)`.
pub fn str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(n) => number_repr(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", str_repr(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `value in options`, for the string tuples of the version-1 rules.
pub fn is_one_of<'a>(value: &Value, options: &[&'a str]) -> Option<&'a str> {
    let Value::String(s) = value else {
        return None;
    };
    options.iter().find(|&&o| o == s).copied()
}

#[cfg(test)]
mod tests {
    use super::{collapse_spaces, float, float_repr, lower, str, str_repr};
    use serde_json::json;

    #[test]
    fn reprs() {
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1e-5), "1e-05");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(1_234_567_890_123_456.0), "1234567890123456.0");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(str_repr("it's"), "\"it's\"");
        assert_eq!(str_repr("a\u{1f}\u{a0}é"), "'a\\x1f\\xa0é'");
        assert_eq!(
            str(&json!({"a": [1, 2.0, true, null, "x"]})),
            "{'a': [1, 2.0, True, None, 'x']}"
        );
    }

    #[test]
    fn floats() {
        assert_eq!(float(&json!(" 1_000.5 ")), Some(1000.5));
        assert_eq!(float(&json!("1__0")), None);
        assert_eq!(float(&json!("\u{663}")), Some(3.0));
        assert!(float(&json!("-Infinity")).is_some_and(f64::is_infinite));
        assert_eq!(float(&json!(true)), Some(1.0));
        assert_eq!(float(&json!([1])), None);
    }

    #[test]
    fn strings() {
        assert_eq!(collapse_spaces("  a \u{3000} b  "), "a b");
        assert_eq!(lower("ΟΔΟΣ"), "οδος");
        assert_eq!(lower("ΣΑ"), "σα");
    }
}
