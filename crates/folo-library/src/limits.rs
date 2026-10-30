//! The library's limits (ADR-0038 D1, D6; docs/specs/pack-format.md#container).

/// An archive: at most 1,000 entries.
pub const ARCHIVE_ENTRIES: usize = 1000;
/// An archive: at most 32 MB, counted on the bytes decompressed.
pub const ARCHIVE_UNCOMPRESSED: u64 = 32 * 1024 * 1024;
/// The manifest and each item file.
pub const JSON_FILE: u64 = 1024 * 1024;
/// A preview image.
pub const PREVIEW_FILE: u64 = 2 * 1024 * 1024;
/// Items in a pack, all types counted.
pub const PACK_ITEMS: usize = 500;
/// Scenes in `local/my-presets`, as today's saved presets.
pub const MY_PRESETS_SCENES: usize = 60;
/// Installed and local packs.
pub const LIBRARY_PACKS: usize = 100;
/// The installed and local packs together, in canonical form.
pub const LIBRARY_BYTES: u64 = 128 * 1024 * 1024;
/// An item id.
pub const ITEM_ID: usize = 64;
/// A document in a backup, counted on the bytes decompressed.
pub const DOCUMENT_FILE: u64 = 32 * 1024 * 1024;
/// A backup: at most 10,000 entries (ADR-0038 D27).
pub const BACKUP_ENTRIES: usize = 10_000;
/// A backup: at most 512 MB, counted on the bytes decompressed.
pub const BACKUP_UNCOMPRESSED: u64 = 512 * 1024 * 1024;
