//! Canonical JSON (RFC 8785, the JSON Canonicalization Scheme) and the item
//! hash built on it (ADR-0038 D5). `source/folo_source/contracts/canonical.py`
//! gives the same bytes for the same value; `tests/corpus/canonical.json`
//! checks that they agree.

use std::fmt::Write as _;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Why a value has no canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotCanonical(pub String);

impl std::fmt::Display for NotCanonical {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no canonical JSON: {}", self.0)
    }
}

impl std::error::Error for NotCanonical {}

/// `value` serialised by the JSON Canonicalization Scheme: sorted keys, no
/// whitespace, ECMAScript number formatting, UTF-8.
///
/// # Errors
///
/// When a number is not finite (JSON can't write one, but a literal such as
/// `1e999` reads as infinite).
pub fn to_canonical(value: &Value) -> Result<String, NotCanonical> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// The SHA-256 of the value's canonical JSON, in lowercase hexadecimal: an
/// item's hash.
///
/// # Errors
///
/// As [`to_canonical`].
pub fn item_hash(value: &Value) -> Result<String, NotCanonical> {
    Ok(hex(&Sha256::digest(to_canonical(value)?.as_bytes())))
}

/// Lowercase hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn write_value(value: &Value, out: &mut String) -> Result<(), NotCanonical> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            let text = number.to_string();
            let float = text
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| NotCanonical(format!("the number {text} is not finite")))?;
            out.push_str(&number_text(float));
        }
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            // UTF-16 code units, which is not the same order as UTF-8 bytes
            // for characters beyond the Basic Multilingual Plane
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (at, key) in keys.into_iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// ECMAScript's `Number::toString` (ECMA-262, 6.1.6.1.20) of a finite number.
fn number_text(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest digits that read back as the same
    // number, which is what the ECMAScript algorithm asks for
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` always has an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().expect("`{:e}` writes a decimal exponent");
    format!("{sign}{}", es_digits(&digits, exponent))
}

/// The digits `d1d2…dk` of a number `d1.d2…dk × 10^exponent`, written the
/// way ECMAScript does.
fn es_digits(digits: &str, exponent: i32) -> String {
    let k = i32::try_from(digits.len()).expect("a float has at most 17 digits");
    let n = exponent + 1;
    if k <= n && n <= 21 {
        let zeros = usize::try_from(n - k).expect("n >= k here");
        format!("{digits}{}", "0".repeat(zeros))
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(usize::try_from(n).expect("n > 0 here"));
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        let zeros = usize::try_from(-n).expect("n <= 0 here");
        format!("0.{}{digits}", "0".repeat(zeros))
    } else {
        let sign = if exponent < 0 { '-' } else { '+' };
        let power = exponent.abs();
        match digits.split_at(1) {
            (first, "") => format!("{first}e{sign}{power}"),
            (first, rest) => format!("{first}.{rest}e{sign}{power}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canon(text: &str) -> String {
        to_canonical(&serde_json::from_str(text).unwrap()).unwrap()
    }

    #[test]
    fn sorts_keys_and_drops_whitespace() {
        assert_eq!(
            canon(r#"{ "b": 1, "a": [ true, null ] }"#),
            r#"{"a":[true,null],"b":1}"#
        );
    }

    #[test]
    fn keys_sort_by_utf16_units_not_bytes() {
        // U+FF5E (BMP) sorts after U+1F600 (a surrogate pair) in UTF-8, before it in UTF-16
        let value = json!({"\u{ff5e}": 1, "\u{1f600}": 2});
        assert_eq!(
            to_canonical(&value).unwrap(),
            "{\"\u{1f600}\":2,\"\u{ff5e}\":1}"
        );
    }

    #[test]
    fn numbers_are_written_as_ecmascript_does() {
        for (literal, expected) in [
            ("45.0", "45"),
            ("-0", "0"),
            ("-0.0", "0"),
            ("0.5", "0.5"),
            ("0.1111111111111111", "0.1111111111111111"),
            ("1e21", "1e+21"),
            ("1e20", "100000000000000000000"),
            ("123456789012345680000", "123456789012345680000"),
            ("1.5e-7", "1.5e-7"),
            ("0.000001", "0.000001"),
            ("0.0000001", "1e-7"),
            ("1E+2", "100"),
            ("4.35", "4.35"),
            ("5e-324", "5e-324"),
            ("1.7976931348623157e308", "1.7976931348623157e+308"),
            ("9007199254740993", "9007199254740992"),
            ("-12.5", "-12.5"),
        ] {
            assert_eq!(canon(literal), expected, "{literal}");
        }
    }

    #[test]
    fn strings_escape_only_what_json_requires() {
        let value = json!("a\"b\\c\n\t\u{1}\u{7f}é\u{1f600}/");
        assert_eq!(
            to_canonical(&value).unwrap(),
            "\"a\\\"b\\\\c\\n\\t\\u0001\u{7f}é\u{1f600}/\""
        );
    }

    #[test]
    fn an_infinite_number_has_no_canonical_form() {
        let value: Value = serde_json::from_str("1e999").unwrap();
        assert!(to_canonical(&value).is_err());
    }

    #[test]
    fn the_hash_ignores_key_order_and_formatting() {
        let a: Value = serde_json::from_str(r#"{"x": 1.0, "y": "z"}"#).unwrap();
        let b: Value = serde_json::from_str("{\n \"y\": \"z\", \"x\": 1\n}").unwrap();
        assert_eq!(item_hash(&a).unwrap(), item_hash(&b).unwrap());
        assert_eq!(item_hash(&a).unwrap().len(), 64);
    }
}
