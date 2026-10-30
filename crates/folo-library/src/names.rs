//! Item types and the entry names a pack may hold (ADR-0038 D4,
//! docs/specs/pack-format.md#container).

/// The manifest's file name.
pub const MANIFEST: &str = "folo-pack.json";
/// The folder of previews.
pub const PREVIEWS: &str = "previews";
/// Preview file extensions, with the image type each names.
pub const PREVIEW_EXTENSIONS: [&str; 4] = ["png", "jpg", "webp", "gif"];
/// Windows' reserved device names, which no entry may take.
const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The five kinds of item a pack holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemType {
    Palettes,
    Reactions,
    Looks,
    Scenes,
    ZoneTemplates,
}

impl ItemType {
    /// In the order of the manifest's `contents`.
    pub const ALL: [Self; 5] = [
        Self::Palettes,
        Self::Reactions,
        Self::Looks,
        Self::Scenes,
        Self::ZoneTemplates,
    ];

    /// The type as its folder and the API's `ItemType` call it.
    #[must_use]
    pub const fn folder(self) -> &'static str {
        match self {
            Self::Palettes => "palettes",
            Self::Reactions => "reactions",
            Self::Looks => "looks",
            Self::Scenes => "scenes",
            Self::ZoneTemplates => "zone-templates",
        }
    }

    /// The type's key in the manifest's `contents`.
    #[must_use]
    pub const fn contents_key(self) -> &'static str {
        match self {
            Self::ZoneTemplates => "zone_templates",
            other => other.folder(),
        }
    }

    /// The schema definition its items are clamped with.
    #[must_use]
    pub const fn definition(self) -> &'static str {
        match self {
            Self::Palettes => "PaletteItem",
            Self::Reactions => "ReactionItem",
            Self::Looks => "LookItem",
            Self::Scenes => "SceneItem",
            Self::ZoneTemplates => "ZoneTemplateItem",
        }
    }

    /// The type a folder or API name stands for.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.folder() == name)
    }
}

/// What an entry of a pack is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Manifest,
    Item(ItemType),
    Preview,
}

/// Whether `name` is a valid file name stem: `[a-z0-9][a-z0-9_-]{0,63}`, and
/// not one of Windows' reserved names. Names are lowercase ASCII, so they
/// can't collide by case or Unicode normalisation, and hold no `..`, `:`,
/// backslash, trailing dot or space.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !RESERVED_NAMES.contains(&name)
}

/// What the entry at `path` (`/`-separated, relative to the pack) is, or why
/// it isn't allowed.
///
/// # Errors
///
/// The reason, when no entry may have that path.
pub fn classify(path: &str) -> Result<Entry, String> {
    if path == MANIFEST {
        return Ok(Entry::Manifest);
    }
    let parts: Vec<&str> = path.split('/').collect();
    let [folder, file] = parts.as_slice() else {
        return Err("entries are folo-pack.json or one folder deep".to_owned());
    };
    let Some((stem, extension)) = file.rsplit_once('.') else {
        return Err("entries need an extension".to_owned());
    };
    if !valid_name(stem) {
        return Err(format!("the name {stem:?} isn't allowed"));
    }
    if *folder == PREVIEWS {
        return PREVIEW_EXTENSIONS
            .contains(&extension)
            .then_some(Entry::Preview)
            .ok_or_else(|| "previews are png, jpg, webp or gif".to_owned());
    }
    match ItemType::parse(folder) {
        Some(kind) if extension == "json" => Ok(Entry::Item(kind)),
        Some(_) => Err("items are .json files".to_owned()),
        None => Err(format!("{folder:?} isn't one of the pack's folders")),
    }
}

/// Whether `id` is a pack id, `author/name`: `[a-z0-9-]`, 1 to 32 characters each.
#[must_use]
pub fn valid_pack_id(id: &str) -> bool {
    // The id is a folder path, so Windows' device names are refused here too
    let part = |p: &str| {
        (1..=32).contains(&p.len())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !RESERVED_NAMES.contains(&p)
    };
    id.split_once('/').is_some_and(|(a, n)| part(a) && part(n))
}

/// The author of a pack id.
#[must_use]
pub fn author(id: &str) -> &str {
    id.split_once('/').map_or(id, |(author, _)| author)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_rule() {
        for ok in ["a", "0", "preset-001", "kick_punch", &"a".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            "_a",
            "A",
            "a.b",
            "a b",
            "a:b",
            "a\\b",
            "é",
            "con",
            "lpt9",
            "aux",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn entries_are_classified() {
        assert_eq!(classify("folo-pack.json"), Ok(Entry::Manifest));
        assert_eq!(classify("scenes/x.json"), Ok(Entry::Item(ItemType::Scenes)));
        assert_eq!(
            classify("zone-templates/x.json"),
            Ok(Entry::Item(ItemType::ZoneTemplates))
        );
        assert_eq!(classify("previews/x.webp"), Ok(Entry::Preview));
        for bad in [
            "x.json",
            "scenes/sub/x.json",
            "scenes/x.png",
            "previews/x.svg",
            "other/x.json",
            "scenes/X.json",
            "scenes/con.json",
            "scenes/..json",
            "scenes/x",
            "../x.json",
        ] {
            assert!(classify(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pack_ids_have_an_author_and_a_name() {
        assert!(valid_pack_id("folo/core"));
        assert!(valid_pack_id(&format!(
            "{}/{}",
            "a".repeat(32),
            "b".repeat(32)
        )));
        for bad in [
            "con/x",
            "a/aux",
            "lpt1/lpt2",
            "core",
            "a/b/c",
            "A/b",
            "a/",
            "/b",
            "a/b_c",
            &format!("{}/b", "a".repeat(33)),
        ] {
            assert!(!valid_pack_id(bad), "{bad}");
        }
        assert_eq!(author("local/my-presets"), "local");
    }
}
