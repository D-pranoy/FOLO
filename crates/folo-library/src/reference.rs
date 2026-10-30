//! Reading a scene reference (ADR-0038 D2) and naming the base file of an
//! applied scene (D11).

use sha2::{Digest, Sha256};

use crate::canonical::hex;

/// The pack and item of text matching `^[a-z0-9-]{1,32}/[a-z0-9-]{1,32}:.+$`,
/// split at the first `/` and then the first `:`.
#[must_use]
pub fn parse(text: &str) -> Option<(&str, &str)> {
    let (author, rest) = text.split_once('/')?;
    let (name, item) = rest.split_once(':')?;
    let id_part = |part: &str| {
        (1..=32).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if !id_part(author) || !id_part(name) || item.is_empty() {
        return None;
    }
    // The pack id is the text up to the colon: `author/name`
    Some((&text[..author.len() + 1 + name.len()], item))
}

/// The file name of a scene's base in `applied/`: the hexadecimal SHA-256 of
/// the scene's id, so no id reaches a file name (ADR-0038 D11).
#[must_use]
pub fn base_file_name(scene_id: &str) -> String {
    format!("{}.json", hex(&Sha256::digest(scene_id.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pack_items_and_nothing_else() {
        assert_eq!(
            parse("folo/effects:ledfx-bar"),
            Some(("folo/effects", "ledfx-bar"))
        );
        assert_eq!(parse("a/b:c:d"), Some(("a/b", "c:d")));
        for text in [
            "fx:ledfx-bar",
            "a/b:",
            "A/b:c",
            "a/b",
            "/b:c",
            "a/:c",
            "a/b/c:d",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        let long = "x".repeat(33);
        assert_eq!(parse(&format!("{long}/b:c")), None);
    }

    #[test]
    fn base_files_are_named_by_the_hash_of_the_id() {
        let name = base_file_name("scene-1");
        assert_eq!(name.len(), 64 + 5);
        assert!(name.strip_suffix(".json").is_some());
        assert_ne!(name, base_file_name("scene-2"));
    }
}
