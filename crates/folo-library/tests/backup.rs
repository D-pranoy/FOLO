//! Backup archives at the library's level (RB-09 change 5, ADR-0038 D25 to
//! D27): the round trip, the limits that are cheap to reach, the pack checks
//! of a restore, and the replacement of the library.

mod common;

use std::collections::BTreeMap;

use common::{build, entries, open, palette, scratch};
use folo_library::archive::{self, ArchiveLimits, Entries};
use folo_library::backup::{self, Contents, MANIFEST};
use folo_library::{ItemType, LibraryError};
use serde_json::{Value, json};

fn documents() -> Entries {
    ["zones.json", "show.json", "providers.json", "sources.json"]
        .into_iter()
        .map(|name| (name.to_owned(), b"{}\n".to_vec()))
        .collect()
}

fn contents(packs: &[(&str, Entries)]) -> Contents {
    Contents {
        documents: documents(),
        packs: packs
            .iter()
            .map(|(id, files)| ((*id).to_owned(), files.clone()))
            .collect(),
        bases: BTreeMap::new(),
    }
}

fn expect_error(bytes: &[u8]) -> LibraryError {
    match backup::read(bytes) {
        Err(error) => error,
        Ok(_) => panic!("the backup was accepted"),
    }
}

#[test]
fn a_backup_reads_back_as_it_was_written() {
    let mut bases = BTreeMap::new();
    let name = format!("{}.json", "ab".repeat(32));
    bases.insert(name.clone(), b"{\"id\":1}".to_vec());
    let mut parts = contents(&[("acme/one", entries("acme/one", "1.0.0"))]);
    parts.bases = bases;
    let bytes = backup::write(parts, "1.2.3", 1_700_000_000).unwrap();
    let restored = backup::read(&bytes).unwrap();
    assert_eq!(restored.documents.len(), 4);
    assert_eq!(restored.packs.len(), 1);
    assert_eq!(restored.packs[0].pack.id, "acme/one");
    assert_eq!(restored.bases[&name], b"{\"id\":1}");
    // The manifest comes first and says what it describes
    let limit = ArchiveLimits {
        entries: 100,
        uncompressed: 1 << 20,
    };
    let all = archive::read_with(&bytes, limit, &|_| Ok(1 << 20)).unwrap();
    let manifest: Value = serde_json::from_slice(&all[MANIFEST]).unwrap();
    assert_eq!(manifest["node_version"], "1.2.3");
    assert_eq!(manifest["created"], 1_700_000_000_u64);
}

#[test]
fn the_same_content_is_the_same_archive() {
    let one = backup::write(
        contents(&[("acme/one", entries("acme/one", "1.0.0"))]),
        "1",
        5,
    );
    let two = backup::write(
        contents(&[("acme/one", entries("acme/one", "1.0.0"))]),
        "1",
        5,
    );
    assert_eq!(one.unwrap(), two.unwrap());
}

#[test]
fn more_than_ten_thousand_entries_are_refused() {
    let mut files = documents();
    for number in 0..10_000 {
        files.insert(format!("applied/{number:064x}.json"), b"{}".to_vec());
    }
    let bytes = backup::write(
        Contents {
            documents: files,
            packs: BTreeMap::new(),
            bases: BTreeMap::new(),
        },
        "1",
        0,
    )
    .unwrap();
    // 4 documents, 10,000 bases and the manifest
    let error = expect_error(&bytes);
    assert!(
        matches!(&error, LibraryError::Structural { reason, .. } if reason.contains("10000 entries")),
        "{error:?}"
    );
}

#[test]
fn sizes_are_counted_on_the_bytes_decompressed() {
    let mut files = Entries::new();
    files.insert("a.json".into(), vec![b' '; 600 * 1024]);
    files.insert("b.json".into(), vec![b' '; 600 * 1024]);
    let bytes = archive::write_first(&files, "a.json").unwrap();
    assert!(
        bytes.len() < 10 * 1024,
        "the zeros compress to {}",
        bytes.len()
    );
    let limit = ArchiveLimits {
        entries: 10,
        uncompressed: 1024 * 1024,
    };
    let error = archive::read_with(&bytes, limit, &|_| Ok(1 << 30)).unwrap_err();
    assert!(
        matches!(&error, LibraryError::Structural { path, reason }
            if path == "b.json" && reason.contains("more than 1048576 bytes")),
        "{error:?}"
    );
    // A document over its own limit, however well it compresses
    let mut files = documents();
    files.insert("show.json".into(), vec![b' '; 33 * 1024 * 1024]);
    let bytes = backup::write(
        Contents {
            documents: files,
            packs: BTreeMap::new(),
            bases: BTreeMap::new(),
        },
        "1",
        0,
    )
    .unwrap();
    let error = expect_error(&bytes);
    assert!(
        matches!(&error, LibraryError::Structural { path, .. } if path == "show.json"),
        "{error:?}"
    );
}

#[test]
fn a_hundred_and_one_packs_are_too_many() {
    let packs: Vec<(String, Entries)> = (0..101)
        .map(|n| {
            let id = format!("acme/p{n}");
            let files = entries(&id, "1.0.0");
            (id, files)
        })
        .collect();
    let mut parts = contents(&[]);
    parts.packs = packs.into_iter().collect();
    let bytes = backup::write(parts, "1", 0).unwrap();
    let error = expect_error(&bytes);
    assert!(
        matches!(&error, LibraryError::Structural { path, reason }
            if path == "library" && reason.contains("100 packs")),
        "{error:?}"
    );
}

#[test]
fn a_restore_checks_the_packs_against_this_node() {
    let dir = scratch("backup-compat");
    let library = open(&dir, "1.0.0");
    let too_new = build(
        "acme/future",
        "1.0.0",
        json!({"folo": ">=2 <3"}),
        &[(ItemType::Palettes, "p1", palette("a"))],
    );
    let bytes = backup::write(contents(&[("acme/future", too_new)]), "1", 0).unwrap();
    let restored = backup::read(&bytes).unwrap();
    let error = library.check_backup(&restored).unwrap_err();
    assert!(
        matches!(error, LibraryError::Incompatible { .. }),
        "{error:?}"
    );

    let needy = build(
        "acme/needy",
        "1.0.0",
        json!({"requires": {"acme/base": "^1"}}),
        &[(ItemType::Palettes, "p1", palette("a"))],
    );
    let bytes = backup::write(contents(&[("acme/needy", needy.clone())]), "1", 0).unwrap();
    let error = library
        .check_backup(&backup::read(&bytes).unwrap())
        .unwrap_err();
    assert!(
        matches!(error, LibraryError::MissingDependency { .. }),
        "{error:?}"
    );

    // Satisfied by a pack of the same backup
    let base = entries("acme/base", "1.4.0");
    let bytes = backup::write(
        contents(&[("acme/needy", needy), ("acme/base", base)]),
        "1",
        0,
    )
    .unwrap();
    library
        .check_backup(&backup::read(&bytes).unwrap())
        .unwrap();
}

#[test]
fn the_library_becomes_exactly_the_backups() {
    let dir = scratch("backup-replace");
    let library = open(&dir, "0.0.0");
    library
        .install(&common::zip(&entries("acme/old", "1.0.0")), false)
        .unwrap();
    let before = library.last_event();
    let bytes = backup::write(
        contents(&[("acme/new", entries("acme/new", "1.0.0"))]),
        "1",
        0,
    )
    .unwrap();
    let restored = backup::read(&bytes).unwrap();

    // A move that fails leaves the packs as they were
    let failed = library.replace_all(&restored.packs, || {
        Err(LibraryError::Io {
            reason: "disk".into(),
        })
    });
    assert!(failed.is_err());
    let ids = |library: &folo_library::Library| -> Vec<String> {
        library
            .packs()
            .iter()
            .filter(|p| p["builtin"] == false)
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&library), ["acme/old", "local/my-presets"]);
    assert_eq!(library.last_event(), before);

    // The move is the caller's: here it moves the staged folder in
    let staging = dir.join("staged");
    backup::stage(
        &staging.join("library"),
        &staging.join("applied"),
        &restored,
    )
    .unwrap();
    let root = dir.join("library");
    library
        .replace_all(&restored.packs, || {
            std::fs::remove_dir_all(&root)?;
            std::fs::rename(staging.join("library"), &root)?;
            Ok(())
        })
        .unwrap();
    // local/my-presets is made again when the backup lacks it
    assert_eq!(ids(&library), ["acme/new", "local/my-presets"]);
    assert!(root.join("acme/new/folo-pack.json").exists());
    assert!(root.join("local/my-presets/folo-pack.json").exists());
    let events = library.events_since(before);
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].id.as_str(), events[0].state), ("", "restored"));
    // The built-in packs stay
    assert!(library.manifest("folo/core").is_ok());
}
