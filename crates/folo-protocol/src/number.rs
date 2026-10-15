//! JSON numbers as Python sees them, so the Rust clampers match the Python
//! ones byte for byte: an integer literal is an exact integer (Python's
//! `int`), anything with a point or an exponent is a float. Needs
//! `serde_json`'s `arbitrary_precision`, which keeps the literal.

use serde_json::Number;

/// A JSON number, by its literal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Num {
    /// An integer literal that fits in an `i128`.
    Int(i128),
    /// An integer literal beyond `i128`: only its sign matters (beyond every bound).
    Huge(i8),
    /// A literal with a point or an exponent; may be infinite (`1e400`).
    Float(f64),
}

impl Num {
    /// The number behind a JSON value.
    #[must_use]
    pub fn of(number: &Number) -> Self {
        let text = number.to_string();
        if text.contains(['.', 'e', 'E']) {
            return Self::Float(text.parse().unwrap_or(f64::NAN));
        }
        match text.parse::<i128>() {
            Ok(value) => Self::Int(value),
            Err(_) => Self::Huge(if text.starts_with('-') { -1 } else { 1 }),
        }
    }

    /// Python rejects `inf` and `nan` floats, but any integer is a number.
    #[must_use]
    pub fn is_finite_or_integer(self) -> bool {
        match self {
            Self::Float(value) => value.is_finite(),
            Self::Int(_) | Self::Huge(_) => true,
        }
    }

    /// Python's `round(x)`: to the nearest integer, ties to even.
    #[must_use]
    pub fn round_ties_even(self) -> Self {
        match self {
            Self::Float(value) => Self::Float(value.round_ties_even()),
            other => other,
        }
    }

    /// `max(lo, min(hi, x))` with Python's exact comparisons.
    #[must_use]
    pub fn clamp(self, lo: Option<f64>, hi: Option<f64>) -> Self {
        let mut out = self;
        if let Some(hi) = hi
            && out.compare(hi) == std::cmp::Ordering::Greater
        {
            out = Self::Float(hi);
        }
        if let Some(lo) = lo
            && out.compare(lo) == std::cmp::Ordering::Less
        {
            out = Self::Float(lo);
        }
        out
    }

    #[allow(clippy::cast_precision_loss)]
    fn compare(self, bound: f64) -> std::cmp::Ordering {
        match self {
            Self::Huge(sign) => {
                if sign > 0 {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            }
            // The bounds are below 2^53, where the conversion keeps the order
            Self::Int(value) => (value as f64).total_cmp(&bound),
            Self::Float(value) => value
                .partial_cmp(&bound)
                .unwrap_or(std::cmp::Ordering::Equal),
        }
    }

    /// Python's `x > n`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn greater_than(self, n: i64) -> bool {
        match self {
            Self::Int(value) => value > i128::from(n),
            Self::Huge(sign) => sign > 0,
            Self::Float(value) => value > n as f64,
        }
    }

    /// Python's `x == y` between numbers: exact, as Python compares.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Huge(_), _) | (_, Self::Huge(_)) => false,
            _ => self.to_f64() == other.to_f64(),
        }
    }

    /// Python's `float(x)`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn to_f64(self) -> f64 {
        match self {
            Self::Int(value) => value as f64,
            Self::Huge(sign) => f64::from(sign) * f64::INFINITY,
            Self::Float(value) => value,
        }
    }

    /// Python's `int(x)` of a clamped, integral number.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn to_integer(self) -> Number {
        match self {
            Self::Int(value) => i64::try_from(value).map_or_else(
                |_| Number::from_f64(value as f64).unwrap_or_else(|| Number::from(0)),
                Number::from,
            ),
            Self::Float(value) => Number::from(value as i64),
            Self::Huge(sign) => Number::from(i64::from(sign) * i64::MAX),
        }
    }
}

/// A finite float as a JSON number.
///
/// # Panics
///
/// On a non-finite value, which the clampers never produce.
#[must_use]
pub fn from_f64(value: f64) -> Number {
    Number::from_f64(value).expect("clamped numbers are finite")
}

/// Python's `round(x, ndigits)`: the decimal nearest to the exact binary value,
/// ties to even. Rust's fixed-precision formatting rounds the same way.
#[must_use]
pub fn py_round(x: f64, ndigits: i64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let digits = usize::try_from(ndigits).unwrap_or(0);
    format!("{x:.digits$}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::{Num, py_round};
    use serde_json::Value;

    fn num(text: &str) -> Num {
        match serde_json::from_str::<Value>(text).unwrap() {
            Value::Number(n) => Num::of(&n),
            _ => unreachable!(),
        }
    }

    #[test]
    fn literals() {
        assert_eq!(num("5"), Num::Int(5));
        assert_eq!(num("5.0"), Num::Float(5.0));
        assert_eq!(num("1e2"), Num::Float(100.0));
        assert_eq!(num(&format!("1{}", "0".repeat(60))), Num::Huge(1));
        assert!(!num("1e400").is_finite_or_integer());
    }

    #[test]
    fn rounds_like_python() {
        // Exact ties go to the even digit; everything else to the nearest
        assert!((py_round(0.25, 1) - 0.2).abs() < f64::EPSILON);
        assert!((py_round(0.375, 2) - 0.38).abs() < f64::EPSILON);
        assert!((py_round(2.675, 2) - 2.67).abs() < f64::EPSILON);
        assert!((py_round(-0.05, 1) - (-0.1)).abs() < f64::EPSILON);
        assert!((num("2.5").round_ties_even().to_f64() - 2.0).abs() < f64::EPSILON);
    }
}
