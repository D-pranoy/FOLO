//! Version requirements (ADR-0038 D7): `folo` and the ranges in `requires`.

use semver::{BuildMetadata, Prerelease, Version, VersionReq};

/// The range as Rust's `semver` crate reads it, with spaces between
/// comparators read as commas (`>=1.0 <2` is `>=1.0, <2`).
///
/// # Errors
///
/// When the text isn't a version requirement.
pub fn parse_range(text: &str) -> Result<VersionReq, semver::Error> {
    let mut comparators: Vec<String> = Vec::new();
    let mut operator_only = false;
    for token in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
    {
        // `>= 1.0` is one comparator
        if operator_only && let Some(last) = comparators.last_mut() {
            last.push_str(token);
        } else {
            comparators.push(token.to_owned());
        }
        operator_only = token
            .chars()
            .all(|c| matches!(c, '<' | '>' | '=' | '^' | '~'));
    }
    VersionReq::parse(&comparators.join(", "))
}

/// The version without its pre-release part, so `1.0.0-rc.1` meets `>=1.0`.
#[must_use]
pub fn release_of(version: &Version) -> Version {
    Version {
        pre: Prerelease::EMPTY,
        build: BuildMetadata::EMPTY,
        ..version.clone()
    }
}

/// Whether `version` meets `range`.
#[must_use]
pub fn meets(range: &VersionReq, version: &Version) -> bool {
    range.matches(&release_of(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(range: &str, version: &str) -> bool {
        meets(
            &parse_range(range).unwrap(),
            &Version::parse(version).unwrap(),
        )
    }

    #[test]
    fn spaces_between_comparators_are_commas() {
        assert!(ok(">=1.0 <2", "1.4.0"));
        assert!(!ok(">=1.0 <2", "2.0.0"));
        assert!(!ok(">=1.0 <2", "0.9.9"));
        assert!(ok(">=1.0, <2", "1.0.0"));
        assert!(ok(">= 1.0  <  2", "1.9.0"));
        assert!(ok("^1.2", "1.9.0"));
        assert!(ok("*", "0.0.1"));
    }

    #[test]
    fn the_prerelease_part_is_ignored() {
        assert!(ok(">=1.0", "1.0.0-rc.1"));
        assert!(!ok(">=1.0", "0.9.0-rc.1"));
    }

    #[test]
    fn junk_is_not_a_range() {
        assert!(parse_range("one point two").is_err());
        assert!(parse_range("").is_err());
    }
}
