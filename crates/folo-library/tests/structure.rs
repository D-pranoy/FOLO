//! Structural validation and the limits (ADR-0038 D4, D6): what makes a
//! `422 structural_error`, with the path and the reason.

mod common;

use std::io::Write as _;

use common::{build, entries, open, palette, rehash, scratch, zip};
use folo_library::archive::Entries;
use folo_library::{ItemType, LibraryError};
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn refused(bytes: &[u8]) -> (String, String) {
    let library = open(&scratch("structure"), "0.0.0");
    match library.install(bytes, false) {
        Err(LibraryError::Structural { path, reason }) => (path, reason),
        other => panic!("expected a structural error, got {other:?}"),
    }
}

fn refused_entries(files: &Entries) -> (String, String) {
    refused(&zip(files))
}

fn with(mut files: Entries, path: &str, bytes: &[u8]) -> Entries {
    files.insert(path.to_owned(), bytes.to_vec());
    files
}

fn archive_of(entries: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data, method) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(*method),
            )
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn not_an_archive() {
    assert!(
        refused(b"definitely not a zip")
            .1
            .contains("not a zip archive")
    );
    assert!(refused(b"").1.contains("not a zip archive"));
}

#[test]
fn entry_names() {
    let good = entries("acme/pack", "1.0.0");
    for name in [
        "palettes/Upper.json",
        "palettes/con.json",
        "palettes/a b.json",
        "palettes/sub/x.json",
        "other/x.json",
        "palettes/x.txt",
        "x.json",
        "previews/x.svg",
        "palettes/..json",
        "palettes/é.json",
    ] {
        let (path, _) = refused_entries(&with(good.clone(), name, b"{}"));
        assert_eq!(path, name);
    }
}

#[test]
fn directory_entries_are_ignored() {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    writer.add_directory("palettes/", options).unwrap();
    for (name, data) in &entries("acme/pack", "1.0.0") {
        writer.start_file(name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    assert!(
        open(&scratch("dirs"), "0.0.0")
            .install(&bytes, false)
            .is_ok()
    );
}

#[test]
fn a_duplicate_entry_is_an_error() {
    let manifest = entries("acme/pack", "1.0.0")["folo-pack.json"].clone();
    let mut bytes = archive_of(&[
        ("folo-pack.json", &manifest, CompressionMethod::Stored),
        ("folo-pack.jsoX", &manifest, CompressionMethod::Stored),
    ]);
    // The writer refuses a duplicate name, so one is made by renaming the second
    for at in 0..bytes.len() - 14 {
        if &bytes[at..at + 14] == b"folo-pack.jsoX" {
            bytes[at..at + 14].copy_from_slice(b"folo-pack.json");
        }
    }
    assert_ne!(refused(&bytes).1, "");
}

#[test]
fn a_link_is_an_error() {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .add_symlink(
            "palettes/x.json",
            "/etc/passwd",
            SimpleFileOptions::default(),
        )
        .unwrap();
    let (path, reason) = refused(&writer.finish().unwrap().into_inner());
    assert_eq!(
        (path.as_str(), reason.as_str()),
        ("palettes/x.json", "is a link")
    );
}

#[test]
fn the_entries_are_counted() {
    let mut files = entries("acme/pack", "1.0.0");
    for n in 0..1000 {
        files.insert(format!("previews/p{n}.png"), b"\x89PNG\r\n\x1a\n".to_vec());
    }
    assert!(refused_entries(&files).1.contains("more than 1000 entries"));
}

#[test]
fn files_are_limited_by_the_bytes_that_come_out() {
    let big = vec![b' '; 1024 * 1024 + 1];
    let (path, reason) = refused_entries(&with(
        entries("acme/pack", "1.0.0"),
        "palettes/big.json",
        &big,
    ));
    assert_eq!(path, "palettes/big.json");
    assert!(reason.contains("larger than"));
    let mut preview = b"\x89PNG\r\n\x1a\n".to_vec();
    preview.resize(2 * 1024 * 1024 + 1, 0);
    assert!(
        refused_entries(&with(
            entries("acme/pack", "1.0.0"),
            "previews/big.png",
            &preview
        ))
        .1
        .contains("larger than")
    );
}

#[test]
fn the_archive_holds_32_mb_uncompressed() {
    let mut files = entries("acme/pack", "1.0.0");
    // 33 preview files of 1 MB each; zeros compress to almost nothing
    for n in 0..33 {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(1024 * 1024, 0);
        files.insert(format!("previews/p{n}.png"), png);
    }
    let bytes = zip(&files);
    assert!(bytes.len() < 1024 * 1024, "the archive itself is small");
    assert!(refused(&bytes).1.contains("more than 33554432 bytes"));
}

#[test]
fn unsupported_methods_are_refused() {
    let manifest = entries("acme/pack", "1.0.0")["folo-pack.json"].clone();
    let mut bytes = archive_of(&[("folo-pack.json", &manifest, CompressionMethod::Stored)]);
    // The central directory's method field (offset 10 from its signature) says "bzip2"
    let central = bytes.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
    bytes[central + 10] = 12;
    let local = 0;
    bytes[local + 8] = 12;
    assert!(refused(&bytes).1.contains("method") || refused(&bytes).1.contains("read"));
}

#[test]
fn the_manifest() {
    let good = entries("acme/pack", "1.0.0");
    let mut missing = good.clone();
    missing.remove("folo-pack.json");
    assert_eq!(refused_entries(&missing).0, "folo-pack.json");
    assert!(
        refused_entries(&with(good.clone(), "folo-pack.json", b"[1]"))
            .1
            .contains("JSON object")
    );
    assert!(
        refused_entries(&with(good.clone(), "folo-pack.json", b"nope"))
            .1
            .contains("JSON")
    );
    let mut huge = vec![b' '; 1024 * 1024];
    huge.extend_from_slice(b"{}");
    assert!(
        refused_entries(&with(good.clone(), "folo-pack.json", &huge))
            .1
            .contains("larger")
    );
    let manifest = |edit: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&good["folo-pack.json"]).unwrap();
        edit(&mut value);
        with(
            good.clone(),
            "folo-pack.json",
            &serde_json::to_vec(&value).unwrap(),
        )
    };
    for key in [
        "id",
        "version",
        "folo",
        "name",
        "description",
        "license",
        "authors",
        "contents",
        "files",
    ] {
        let (path, reason) = refused_entries(&manifest(&|m| {
            m.as_object_mut().unwrap().remove(key);
        }));
        assert!(
            path == "folo-pack.json" && reason.contains(key),
            "{key}: {reason}"
        );
        let (_, wrong) = refused_entries(&manifest(&|m| m[key] = json!(7)));
        assert!(wrong.contains(key), "{key}: {wrong}");
    }
    for id in ["noslash", "A/b", "a/b/c", "a/b_c", ""] {
        assert!(
            refused_entries(&manifest(&|m| m["id"] = json!(id)))
                .1
                .contains("id"),
            "{id}"
        );
    }
    for version in ["1", "one", "1.0", ""] {
        assert!(
            refused_entries(&manifest(&|m| m["version"] = json!(version)))
                .1
                .contains("version"),
            "{version}"
        );
    }
}

#[test]
fn files_must_list_exactly_the_entries() {
    let good = entries("acme/pack", "1.0.0");
    let (path, reason) = refused_entries(&with(
        good.clone(),
        "palettes/p1.json",
        b"{\"id\": \"item-a\", \"changed\": 1}",
    ));
    assert_eq!(
        (path.as_str(), reason.as_str()),
        ("palettes/p1.json", "doesn't match its hash in files")
    );
    let extra = with(
        good.clone(),
        "palettes/extra.json",
        &serde_json::to_vec(&palette("extra")).unwrap(),
    );
    assert_eq!(refused_entries(&extra).1, "isn't listed in files");
    let mut gone = good.clone();
    gone.remove("scenes/s1.json");
    assert_eq!(
        refused_entries(&gone).1,
        "is listed in files but isn't in the pack"
    );
}

#[test]
fn items() {
    let good = entries("acme/pack", "1.0.0");
    let case = |path: &str, bytes: &[u8]| refused_entries(&rehash(with(good.clone(), path, bytes)));
    assert!(
        case("palettes/p1.json", b"[1]").1.contains("no id")
            || case("palettes/p1.json", b"[1]").1.contains("object")
    );
    assert!(
        case("palettes/p1.json", b"{\"name\": \"x\"}")
            .1
            .contains("no id")
    );
    assert!(
        case("palettes/p1.json", b"{\"id\": \"\"}")
            .1
            .contains("no id")
    );
    assert!(case("palettes/p1.json", b"{\"id\": 7}").1.contains("no id"));
    assert!(case("palettes/p1.json", b"nope").1.contains("JSON"));
    let (path, reason) = case(
        "palettes/p1.json",
        br#"{"id": "item-a", "schema_version": 3}"#,
    );
    assert_eq!(path, "palettes/p1.json");
    assert!(reason.contains("schema version"));
    let long = "x".repeat(65);
    let files = build(
        "acme/long",
        "1.0.0",
        json!({}),
        &[(ItemType::Palettes, "p", palette(&long))],
    );
    assert!(refused_entries(&files).1.contains("64 characters"));
}

#[test]
fn ids_are_unique_across_types() {
    let files = build(
        "acme/dup",
        "1.0.0",
        json!({}),
        &[
            (ItemType::Palettes, "a", palette("same")),
            (ItemType::Scenes, "b", common::scene("same")),
        ],
    );
    assert!(refused_entries(&files).1.contains("used twice"));
}

#[test]
fn contents_lists_exactly_the_ids_of_the_files() {
    let good = entries("acme/pack", "1.0.0");
    let change = |edit: &dyn Fn(&mut Value)| {
        let mut manifest: Value = serde_json::from_slice(&good["folo-pack.json"]).unwrap();
        edit(&mut manifest);
        with(
            good.clone(),
            "folo-pack.json",
            &serde_json::to_vec(&manifest).unwrap(),
        )
    };
    for edit in [
        (&|m: &mut Value| m["contents"]["palettes"] = json!([])) as &dyn Fn(&mut Value),
        &|m| m["contents"]["palettes"] = json!(["item-a", "item-a"]),
        &|m| m["contents"]["palettes"] = json!(["item-a", "ghost"]),
        &|m| m["contents"]["palettes"] = json!("item-a"),
        &|m| m["contents"]["reactions"] = json!(["x"]),
        &|m| m["contents"]["scenes"] = json!([7]),
    ] {
        assert!(refused_entries(&change(edit)).1.contains("contents"));
    }
}

#[test]
fn contents_order_is_kept() {
    let items = [
        (ItemType::Palettes, "c", palette("zeta")),
        (ItemType::Palettes, "a", palette("alpha")),
        (ItemType::Palettes, "b", palette("mid")),
    ];
    let library = open(&scratch("order"), "0.0.0");
    library
        .install(
            &zip(&build("acme/order", "1.0.0", json!({}), &items)),
            false,
        )
        .unwrap();
    let ids: Vec<_> = library
        .items(ItemType::Palettes)
        .into_iter()
        .filter(|i| i["pack"] == "acme/order")
        .map(|i| i["item"]["id"].clone())
        .collect();
    assert_eq!(ids, [json!("zeta"), json!("alpha"), json!("mid")]);
}

#[test]
fn a_pack_holds_500_items() {
    let items: Vec<_> = (0..501)
        .map(|n| {
            (
                ItemType::Palettes,
                format!("p{n}"),
                palette(&format!("p{n}")),
            )
        })
        .collect();
    let refs: Vec<_> = items
        .iter()
        .map(|(t, s, v)| (*t, s.as_str(), v.clone()))
        .collect();
    assert!(
        refused_entries(&build("acme/many", "1.0.0", json!({}), &refs))
            .1
            .contains("more than 500 items")
    );
}

#[test]
fn previews() {
    let png = b"\x89PNG\r\n\x1a\n rest".to_vec();
    let mut good = build(
        "acme/pic",
        "1.0.0",
        json!({"previews": {"item-a": "a.png"}}),
        &[(ItemType::Palettes, "p1", palette("item-a"))],
    );
    good.insert("previews/a.png".into(), png.clone());
    let good = rehash(good);
    let library = open(&scratch("previews"), "0.0.0");
    let info = library.install(&zip(&good), false).unwrap();
    assert_eq!(info["id"], "acme/pic");
    let (bytes, kind) = library.preview("acme/pic", "a.png").unwrap();
    assert_eq!((bytes, kind), (png.clone(), "image/png"));
    assert_eq!(
        library
            .items(ItemType::Palettes)
            .into_iter()
            .find(|i| i["pack"] == "acme/pic")
            .unwrap()["preview"],
        "a.png"
    );
    assert_eq!(
        library.preview("acme/pic", "b.png").unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        library.preview("acme/none", "a.png").unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        library.export("acme/pic").unwrap(),
        library.export("acme/pic").unwrap()
    );
    // not the image its extension says
    let fake = rehash(with(good.clone(), "previews/a.png", b"GIF89a....."));
    assert!(refused_entries(&fake).1.contains("image type"));
    // names an item or file the pack doesn't have
    let mut ghost: Value = serde_json::from_slice(&good["folo-pack.json"]).unwrap();
    ghost["previews"] = json!({"nobody": "a.png"});
    assert!(
        refused_entries(&with(
            good.clone(),
            "folo-pack.json",
            &serde_json::to_vec(&ghost).unwrap()
        ))
        .1
        .contains("nobody")
    );
    ghost["previews"] = json!({"item-a": "missing.png"});
    assert!(
        refused_entries(&with(
            good,
            "folo-pack.json",
            &serde_json::to_vec(&ghost).unwrap()
        ))
        .1
        .contains("isn't in the pack")
    );
}

#[test]
fn each_preview_type_is_checked_by_content() {
    let kinds: [(&str, &[u8]); 4] = [
        ("png", b"\x89PNG\r\n\x1a\n.."),
        ("jpg", &[0xff, 0xd8, 0xff, 0xe0, 0]),
        ("webp", b"RIFF\0\0\0\0WEBPVP8 "),
        ("gif", b"GIF87a.."),
    ];
    for (extension, bytes) in kinds {
        let mut files = entries("acme/pic", "1.0.0");
        files.insert(format!("previews/x.{extension}"), bytes.to_vec());
        assert!(
            open(&scratch("kinds"), "0.0.0")
                .install(&zip(&rehash(files)), false)
                .is_ok(),
            "{extension}"
        );
    }
}
