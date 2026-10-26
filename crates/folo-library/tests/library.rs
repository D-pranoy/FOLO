//! The library service end to end, in a folder: loading, install, export,
//! removal, and the items of local packs (ADR-0038 D1 to D10).

mod common;

use common::{build, code, entries, open, palette, rehash, scene, scratch, zip};
use folo_library::{ItemType, Library, LibraryError};
use serde_json::{Value, json};

fn ids(library: &Library) -> Vec<String> {
    library
        .packs()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

fn install(library: &Library, id: &str, version: &str) -> Result<Value, LibraryError> {
    library.install(&zip(&entries(id, version)), false)
}

// ------------------------------------------------------------------ loading

#[test]
fn the_built_in_packs_come_first() {
    let dir = scratch("builtin");
    let library = open(&dir, "0.0.0");
    install(&library, "acme/aaa", "1.0.0").unwrap();
    assert_eq!(
        ids(&library),
        [
            "folo/core",
            "folo/effects",
            "folo/genres",
            "acme/aaa",
            "local/my-presets"
        ]
    );
    let packs = library.packs();
    let count = |id: &str| {
        packs.iter().find(|p| p["id"] == id).unwrap()["items"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(
        (
            count("folo/core"),
            count("folo/genres"),
            count("folo/effects")
        ),
        (35, 14, 32)
    );
    assert!(packs[0]["builtin"].as_bool().unwrap() && !packs[0]["editable"].as_bool().unwrap());
    assert_eq!(packs[0]["hash"].as_str().unwrap().len(), 64);
}

#[test]
fn every_alias_names_a_built_in_scene() {
    let library = open(&scratch("aliases"), "0.0.0");
    let scenes = library.items(ItemType::Scenes);
    let refs: Vec<&str> = scenes.iter().map(|i| i["ref"].as_str().unwrap()).collect();
    for id in ["fx:ledfx-bar", "genre:edm", "fx:beat-sync"] {
        let reference = library.alias(id).unwrap();
        assert!(refs.contains(&reference.as_str()), "{id} -> {reference}");
    }
    assert!(library.alias("fx:nothing").is_none());
}

#[test]
fn my_presets_is_created_empty_once() {
    let dir = scratch("my-presets");
    let library = open(&dir, "0.0.0");
    let info = library
        .packs()
        .into_iter()
        .find(|p| p["id"] == "local/my-presets")
        .unwrap();
    assert_eq!(
        (info["items"].as_u64(), info["editable"].as_bool()),
        (Some(0), Some(true))
    );
    library
        .put_item("local/my-presets", ItemType::Scenes, "mine", scene("mine"))
        .unwrap();
    drop(library);
    let again = open(&dir, "0.0.0");
    assert_eq!(
        again
            .items(ItemType::Scenes)
            .iter()
            .filter(|i| i["pack"] == "local/my-presets")
            .count(),
        1
    );
}

#[test]
fn a_bad_folder_is_skipped_and_the_library_still_opens() {
    let dir = scratch("bad-folder");
    install(&open(&dir, "0.0.0"), "acme/good", "1.0.0").unwrap();
    let bad = dir.join("library/acme/broken");
    std::fs::create_dir_all(bad.join("scenes")).unwrap();
    std::fs::write(bad.join("folo-pack.json"), b"{ not json").unwrap();
    std::fs::create_dir_all(dir.join("library/acme/odd")).unwrap();
    std::fs::write(dir.join("library/acme/odd/stray.txt"), b"x").unwrap();
    let library = open(&dir, "0.0.0");
    let found = ids(&library);
    assert!(found.contains(&"acme/good".to_owned()));
    assert!(!found.contains(&"acme/broken".to_owned()) && !found.contains(&"acme/odd".to_owned()));
    assert_eq!(
        std::fs::read(bad.join("folo-pack.json")).unwrap(),
        b"{ not json"
    );
    assert_eq!(
        code(&library.manifest("acme/broken").unwrap_err()),
        "not_found"
    );
}

#[test]
fn a_folder_whose_path_is_not_its_id_is_skipped() {
    let dir = scratch("wrong-folder");
    let moved = dir.join("library/acme/other");
    let library = open(&dir, "0.0.0");
    install(&library, "acme/real", "1.0.0").unwrap();
    drop(library);
    std::fs::rename(dir.join("library/acme/real"), &moved).unwrap();
    assert!(!ids(&open(&dir, "0.0.0")).contains(&"acme/other".to_owned()));
}

#[test]
fn an_unreadable_my_presets_is_never_written_over() {
    let dir = scratch("unreadable");
    drop(open(&dir, "0.0.0"));
    let manifest = dir.join("library/local/my-presets/folo-pack.json");
    std::fs::write(&manifest, b"garbage").unwrap();
    let library = open(&dir, "0.0.0");
    let error = library
        .put_item("local/my-presets", ItemType::Scenes, "x", scene("x"))
        .unwrap_err();
    assert_eq!((error.status(), code(&error)), (409, "pack_unreadable"));
    assert_eq!(std::fs::read(&manifest).unwrap(), b"garbage");
}

// ------------------------------------------------------------------ install

#[test]
fn install_lists_reads_and_exports() {
    let library = open(&scratch("install"), "0.0.0");
    let info = install(&library, "acme/pack", "1.2.3").unwrap();
    assert_eq!(
        (
            info["id"].as_str(),
            info["version"].as_str(),
            info["items"].as_u64()
        ),
        (Some("acme/pack"), Some("1.2.3"), Some(2))
    );
    assert_eq!(
        (info["builtin"].as_bool(), info["editable"].as_bool()),
        (Some(false), Some(false))
    );
    assert_eq!(library.manifest("acme/pack").unwrap()["version"], "1.2.3");
    assert_eq!(
        library
            .item("acme/pack", ItemType::Palettes, "item-a")
            .unwrap()["name"],
        "item-a"
    );
    let listed = library.items(ItemType::Scenes);
    let mine = listed.iter().find(|i| i["pack"] == "acme/pack").unwrap();
    assert_eq!(mine["ref"], "acme/pack:item-b");
    assert_eq!(mine["hash"].as_str().unwrap().len(), 64);
}

#[test]
fn export_then_install_round_trips() {
    let (a, b) = (
        open(&scratch("trip-a"), "0.0.0"),
        open(&scratch("trip-b"), "0.0.0"),
    );
    let first = install(&a, "acme/pack", "1.0.0").unwrap();
    let exported = a.export("acme/pack").unwrap();
    let second = b.install(&exported, false).unwrap();
    assert_eq!(first["hash"], second["hash"]);
    assert_eq!(b.export("acme/pack").unwrap(), exported);
    assert_eq!(
        a.manifest("acme/pack").unwrap(),
        b.manifest("acme/pack").unwrap()
    );
}

#[test]
fn the_hashes_do_not_depend_on_formatting_or_key_order() {
    let reordered = |compact: bool| {
        let mut files = entries("acme/pack", "1.0.0");
        let value: Value = serde_json::from_slice(&files["palettes/p1.json"]).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().iter().collect();
        keys.reverse();
        let object: serde_json::Map<String, Value> = keys
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let text = if compact {
            serde_json::to_vec(&object)
        } else {
            serde_json::to_vec_pretty(&object)
        };
        files.insert("palettes/p1.json".into(), text.unwrap());
        rehash(files)
    };
    let (a, b) = (
        open(&scratch("hash-a"), "0.0.0"),
        open(&scratch("hash-b"), "0.0.0"),
    );
    let one = a.install(&zip(&reordered(true)), false).unwrap();
    let two = b.install(&zip(&reordered(false)), false).unwrap();
    assert_eq!(one["hash"], two["hash"]);
    let item = |l: &Library| {
        l.items(ItemType::Palettes)
            .into_iter()
            .find(|i| i["pack"] == "acme/pack")
            .unwrap()["hash"]
            .clone()
    };
    assert_eq!(item(&a), item(&b));
}

#[test]
fn an_export_is_deterministic_and_canonical() {
    let library = open(&scratch("export"), "0.0.0");
    install(&library, "acme/pack", "1.0.0").unwrap();
    let one = library.export("acme/pack").unwrap();
    assert_eq!(one, library.export("acme/pack").unwrap());
    let read = folo_library::archive::read(&one).unwrap();
    assert_eq!(
        read.keys().cloned().collect::<Vec<_>>(),
        ["folo-pack.json", "palettes/p1.json", "scenes/s1.json"]
    );
    let text = String::from_utf8(read["palettes/p1.json"].clone()).unwrap();
    assert!(text.starts_with('{') && !text.contains(": ") && !text.contains('\n'));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&one)).unwrap();
    assert_eq!(
        archive.by_index(0).unwrap().name().unwrap(),
        "folo-pack.json"
    );
    let file = archive.by_index(1).unwrap();
    assert_eq!(file.last_modified().unwrap().year(), 1980);
}

#[test]
fn the_pack_hash_changes_with_the_content() {
    let library = open(&scratch("hash-changes"), "0.0.0");
    let first = install(&library, "acme/pack", "1.0.0").unwrap();
    let mut other = entries("acme/pack", "1.0.1");
    other.insert(
        "palettes/p1.json".into(),
        serde_json::to_vec(&palette("item-a")).unwrap(),
    );
    let second = library.install(&zip(&rehash(other)), false).unwrap();
    assert_ne!(first["hash"], second["hash"]);
}

#[test]
fn ids_are_clamped_the_same_way_every_time() {
    let layer_less = json!({"id": "s", "schema_version": 2, "scene": {"id": "s", "name": "s", "layers_by_role": {"strip.main": [
        {"name": "a", "effect": {"id": "audio-visualizer", "version": "1.0.0"}}, {"name": "b", "effect": {"id": "audio-visualizer", "version": "1.0.0"}}]}}});
    let files = build(
        "acme/ids",
        "1.0.0",
        json!({}),
        &[(ItemType::Scenes, "s", layer_less)],
    );
    let (a, b) = (
        open(&scratch("ids-a"), "0.0.0"),
        open(&scratch("ids-b"), "0.0.0"),
    );
    a.install(&zip(&files), false).unwrap();
    b.install(&zip(&files), false).unwrap();
    let get = |l: &Library| l.item("acme/ids", ItemType::Scenes, "s").unwrap();
    let (one, two) = (get(&a), get(&b));
    assert_eq!(one, two);
    let layers = one["scene"]["layers_by_role"]["strip.main"]
        .as_array()
        .unwrap();
    assert_eq!(layers.len(), 2);
    assert_ne!(layers[0]["id"], layers[1]["id"]);
    assert_eq!(layers[0]["id"].as_str().unwrap().len(), 8);
}

#[test]
fn installs_are_clamped() {
    let mut loud = palette("loud");
    loud["colors"] = json!(["#ff00ff", "nonsense"]);
    loud["unknown"] = json!(1);
    let files = build(
        "acme/clamp",
        "1.0.0",
        json!({"surprise": true}),
        &[(ItemType::Palettes, "loud", loud)],
    );
    let library = open(&scratch("clamp"), "0.0.0");
    library.install(&zip(&files), false).unwrap();
    let item = library
        .item("acme/clamp", ItemType::Palettes, "loud")
        .unwrap();
    assert!(item.get("unknown").is_none());
    assert_eq!(item["colors"][0], "#FF00FF");
    assert!(
        library
            .manifest("acme/clamp")
            .unwrap()
            .get("surprise")
            .is_none()
    );
}

#[test]
fn conflicting_ids() {
    let library = open(&scratch("conflict"), "0.0.0");
    install(&library, "acme/pack", "1.1.0").unwrap();
    let same = install(&library, "acme/pack", "1.1.0").unwrap_err();
    assert_eq!((same.status(), code(&same)), (409, "already_installed"));
    assert_eq!(
        same.details(),
        json!({"installed": "1.1.0", "offered": "1.1.0"})
    );
    assert_eq!(
        code(&install(&library, "acme/pack", "1.0.0").unwrap_err()),
        "already_installed"
    );
    assert_eq!(
        install(&library, "acme/pack", "1.2.0").unwrap()["version"],
        "1.2.0"
    );
    let older = library
        .install(&zip(&entries("acme/pack", "0.5.0")), true)
        .unwrap();
    assert_eq!(older["version"], "0.5.0");
    assert_eq!(library.manifest("acme/pack").unwrap()["version"], "0.5.0");
}

#[test]
fn the_reserved_authors_cannot_install() {
    let library = open(&scratch("reserved"), "0.0.0");
    for id in ["folo/extra", "local/mine"] {
        let error = install(&library, id, "1.0.0").unwrap_err();
        assert_eq!((error.status(), code(&error)), (409, "reserved_id"), "{id}");
    }
}

#[test]
fn compatibility_and_dependencies() {
    let library = open(&scratch("compat"), "2.1.0-rc.1");
    let wants_one = build("acme/old", "1.0.0", json!({}), &[]);
    let error = library.install(&zip(&wants_one), false).unwrap_err();
    assert_eq!((error.status(), code(&error)), (422, "incompatible"));
    assert_eq!(error.details(), json!({"folo": ">=1.0 <2"}));
    let two = build("acme/new", "1.0.0", json!({"folo": ">=2.0"}), &[]);
    library.install(&zip(&two), false).unwrap();
    let needs = build(
        "acme/needs",
        "1.0.0",
        json!({"folo": "*", "requires": {"acme/new": "^1.0", "acme/absent": "*", "acme/too-new": "^1"}}),
        &[],
    );
    library
        .install(
            &zip(&build("acme/too-new", "0.9.0", json!({"folo": "*"}), &[])),
            false,
        )
        .unwrap();
    let error = library.install(&zip(&needs), false).unwrap_err();
    assert_eq!((error.status(), code(&error)), (422, "missing_dependency"));
    assert_eq!(
        error.details(),
        json!({"requires": {"acme/absent": "*", "acme/too-new": "^1"}})
    );
    let fine = build(
        "acme/needs",
        "1.0.0",
        json!({"folo": "*", "requires": {"acme/new": "^1.0 <2"}}),
        &[],
    );
    assert!(library.install(&zip(&fine), false).is_ok());
}

#[test]
fn a_development_build_skips_the_folo_check() {
    let library = open(&scratch("dev-build"), "0.0.0");
    let future = build("acme/future", "1.0.0", json!({"folo": ">=9"}), &[]);
    assert!(library.install(&zip(&future), false).is_ok());
}

#[test]
fn removal() {
    let library = open(&scratch("remove"), "0.0.0");
    install(&library, "acme/base", "1.0.0").unwrap();
    let dependent = build(
        "acme/dependent",
        "1.0.0",
        json!({"requires": {"acme/base": "^1"}}),
        &[],
    );
    library.install(&zip(&dependent), false).unwrap();
    let error = library.remove("acme/base").unwrap_err();
    assert_eq!((error.status(), code(&error)), (409, "required_by"));
    assert_eq!(error.details(), json!({"packs": ["acme/dependent"]}));
    for protected in ["folo/core", "local/my-presets"] {
        assert_eq!(
            code(&library.remove(protected).unwrap_err()),
            "protected_pack"
        );
    }
    library.remove("acme/dependent").unwrap();
    library.remove("acme/base").unwrap();
    assert_eq!(code(&library.remove("acme/base").unwrap_err()), "not_found");
    assert!(!ids(&library).contains(&"acme/base".to_owned()));
}

#[test]
fn an_install_survives_a_restart_and_a_removal_too() {
    let dir = scratch("restart");
    let library = open(&dir, "0.0.0");
    install(&library, "acme/keep", "1.0.0").unwrap();
    install(&library, "acme/drop", "1.0.0").unwrap();
    library.remove("acme/drop").unwrap();
    let hash = library
        .packs()
        .into_iter()
        .find(|p| p["id"] == "acme/keep")
        .unwrap()["hash"]
        .clone();
    drop(library);
    let again = open(&dir, "0.0.0");
    assert!(
        ids(&again).contains(&"acme/keep".to_owned())
            && !ids(&again).contains(&"acme/drop".to_owned())
    );
    assert_eq!(
        again
            .packs()
            .into_iter()
            .find(|p| p["id"] == "acme/keep")
            .unwrap()["hash"],
        hash
    );
    assert!(
        !dir.join("library/.staging").exists()
            || std::fs::read_dir(dir.join("library/.staging"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn the_library_holds_at_most_100_packs() {
    let library = open(&scratch("full"), "0.0.0");
    // local/my-presets counts as one of the hundred
    for n in 0..99 {
        install(&library, &format!("acme/p{n}"), "1.0.0").unwrap();
    }
    let error = install(&library, "acme/one-more", "1.0.0").unwrap_err();
    assert_eq!((error.status(), code(&error)), (409, "library_full"));
    assert!(
        install(&library, "acme/p5", "2.0.0").is_ok(),
        "replacing doesn't need room"
    );
}

// ------------------------------------------------------------------ the items of local packs

#[test]
fn items_are_written_and_removed_in_local_packs() {
    let dir = scratch("items");
    let library = open(&dir, "0.0.0");
    let mine = "local/my-presets";
    let stored = library
        .put_item(
            mine,
            ItemType::Scenes,
            "first",
            json!({"scene": {"name": "First"}, "bogus": 1}),
        )
        .unwrap();
    assert_eq!(
        (stored["id"].as_str(), stored["scene"]["name"].as_str()),
        (Some("first"), Some("First"))
    );
    assert!(stored.get("bogus").is_none());
    library
        .put_item(mine, ItemType::Scenes, "second", scene("ignored"))
        .unwrap();
    let folder = dir.join("library/local/my-presets/scenes");
    assert!(folder.join("item-001.json").exists() && folder.join("item-002.json").exists());
    library
        .put_item(
            mine,
            ItemType::Scenes,
            "first",
            json!({"scene": {"name": "Renamed"}}),
        )
        .unwrap();
    assert!(!folder.join("item-003.json").exists());
    assert_eq!(
        library.item(mine, ItemType::Scenes, "first").unwrap()["scene"]["name"],
        "Renamed"
    );
    library
        .delete_item(mine, ItemType::Scenes, "first")
        .unwrap();
    library
        .put_item(mine, ItemType::Scenes, "third", scene("third"))
        .unwrap();
    assert!(
        folder.join("item-001.json").exists(),
        "the lowest free number is used"
    );
    assert_eq!(
        code(
            &library
                .delete_item(mine, ItemType::Scenes, "first")
                .unwrap_err()
        ),
        "not_found"
    );
    let manifest = library.manifest(mine).unwrap();
    assert_eq!(manifest["contents"]["scenes"], json!(["second", "third"]));
    drop(library);
    let again = open(&dir, "0.0.0");
    assert_eq!(
        again.manifest(mine).unwrap()["contents"]["scenes"],
        json!(["second", "third"])
    );
}

#[test]
fn item_edits_are_refused_where_the_documents_say() {
    let library = open(&scratch("refused"), "0.0.0");
    install(&library, "acme/pack", "1.0.0").unwrap();
    let read_only = library
        .put_item("acme/pack", ItemType::Scenes, "x", scene("x"))
        .unwrap_err();
    assert_eq!(
        (read_only.status(), code(&read_only)),
        (409, "read_only_pack")
    );
    assert_eq!(
        code(
            &library
                .delete_item("folo/core", ItemType::Scenes, "default")
                .unwrap_err()
        ),
        "read_only_pack"
    );
    assert_eq!(
        code(
            &library
                .put_item("acme/none", ItemType::Scenes, "x", scene("x"))
                .unwrap_err()
        ),
        "not_found"
    );
    let mine = "local/my-presets";
    library
        .put_item(mine, ItemType::Palettes, "same", palette("same"))
        .unwrap();
    let taken = library
        .put_item(mine, ItemType::Scenes, "same", scene("same"))
        .unwrap_err();
    assert_eq!((taken.status(), code(&taken)), (409, "id_taken"));
    for body in [json!([1]), json!("text"), json!(null)] {
        let bad = library
            .put_item(mine, ItemType::Scenes, "y", body)
            .unwrap_err();
        assert_eq!((bad.status(), code(&bad)), (400, "bad_body"));
    }
    let long = "x".repeat(65);
    assert_eq!(
        code(
            &library
                .put_item(mine, ItemType::Scenes, &long, scene("x"))
                .unwrap_err()
        ),
        "bad_body"
    );
    let newer = json!({"schema_version": 3});
    let error = library
        .put_item(mine, ItemType::Scenes, "z", newer)
        .unwrap_err();
    assert_eq!((error.status(), code(&error)), (422, "structural_error"));
    assert!(library.item(mine, ItemType::Scenes, "z").is_err());
}

#[test]
fn my_presets_holds_60_scenes() {
    let library = open(&scratch("sixty"), "0.0.0");
    let mine = "local/my-presets";
    for n in 0..60 {
        library
            .put_item(mine, ItemType::Scenes, &format!("s{n}"), scene("x"))
            .unwrap();
    }
    let error = library
        .put_item(mine, ItemType::Scenes, "one-more", scene("x"))
        .unwrap_err();
    assert_eq!((error.status(), code(&error)), (409, "list_full"));
    library
        .put_item(mine, ItemType::Scenes, "s3", scene("x"))
        .unwrap();
    assert!(
        library
            .put_item(mine, ItemType::Palettes, "a-palette", palette("x"))
            .is_ok()
    );
}

#[test]
fn other_local_packs_hold_500_items() {
    let library = open(&scratch("five-hundred"), "0.0.0");
    let id = "local/big";
    let items: Vec<_> = (0..500)
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
    let files = build(id, "1.0.0", json!({}), &refs);
    // Local packs reach the library through a restore (RB-09 change 5), or a folder
    let folder = library_dir(&library, &files, id);
    drop(library);
    let library = Library::open(folder, "0.0.0".parse().unwrap());
    let error = library
        .put_item(id, ItemType::Scenes, "x", scene("x"))
        .unwrap_err();
    assert_eq!((error.status(), code(&error)), (409, "list_full"));
    assert!(
        library
            .put_item(id, ItemType::Palettes, "p7", palette("p7"))
            .is_ok()
    );
}

/// A library folder holding `files` as pack `id`; its path.
fn library_dir(
    _: &Library,
    files: &folo_library::archive::Entries,
    id: &str,
) -> std::path::PathBuf {
    let dir = scratch("big-library").join("library");
    for (path, bytes) in files {
        let target = dir.join(id).join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    dir
}

// ------------------------------------------------------------------ events

#[test]
fn changes_are_announced_once_each() {
    let library = open(&scratch("events"), "0.0.0");
    assert_eq!(library.last_event(), 0);
    install(&library, "acme/pack", "1.0.0").unwrap();
    library
        .put_item("local/my-presets", ItemType::Scenes, "s", scene("s"))
        .unwrap();
    library.remove("acme/pack").unwrap();
    let events = library.events_since(0);
    let summary: Vec<_> = events.iter().map(|e| (e.id.as_str(), e.state)).collect();
    assert_eq!(
        summary,
        [
            ("acme/pack", "installed"),
            ("local/my-presets", "changed"),
            ("acme/pack", "removed")
        ]
    );
    assert_eq!(library.events_since(events[1].seq).len(), 1);
    assert_eq!(
        library.events_since(library.last_event()),
        Vec::<folo_library::LibraryEvent>::new()
    );
}

// ------------------------------------------------------------------ the review's findings

#[test]
fn a_write_that_grows_past_1_mb_once_indented_is_refused_and_leaves_the_pack_readable() {
    let dir = scratch("grows");
    let library = open(&dir, "0.0.0");
    let mine = "local/my-presets";
    library
        .put_item(mine, ItemType::Scenes, "keep", scene("keep"))
        .unwrap();
    // About 600 KB compact, several MB indented
    let numbers: Vec<u8> = vec![1; 300_000];
    let body = json!({"scene": {"name": "heavy"}, "hints": {"numbers": numbers}});
    assert!(serde_json::to_vec(&body).unwrap().len() < 1024 * 1024);
    let error = library
        .put_item(mine, ItemType::Scenes, "heavy", body)
        .unwrap_err();
    assert_eq!((error.status(), code(&error)), (422, "structural_error"));
    drop(library);
    let again = open(&dir, "0.0.0");
    assert!(
        again.item(mine, ItemType::Scenes, "keep").is_ok(),
        "the pack still reads"
    );
    assert!(
        again
            .put_item(mine, ItemType::Scenes, "next", scene("next"))
            .is_ok()
    );
}

#[test]
fn pack_ids_that_are_device_names_are_refused() {
    let library = open(&scratch("device"), "0.0.0");
    for id in ["con/x", "acme/aux", "lpt1/nul"] {
        let error = library
            .install(&zip(&entries(id, "1.0.0")), false)
            .unwrap_err();
        assert_eq!(code(&error), "structural_error", "{id}");
    }
}

#[test]
fn a_removal_is_not_undone_by_recovery_but_an_interrupted_replacement_is() {
    let dir = scratch("recover");
    let library = open(&dir, "0.0.0");
    install(&library, "acme/stay", "1.0.0").unwrap();
    install(&library, "acme/lost", "1.0.0").unwrap();
    drop(library);
    let root = dir.join("library");
    let trash = root.join(".trash");
    std::fs::create_dir_all(&trash).unwrap();
    // A removal that couldn't finish deleting, and a replacement interrupted between its two moves
    std::fs::create_dir_all(trash.join("gone~1/scenes")).unwrap();
    std::fs::rename(root.join("acme/lost"), trash.join("acme~lost~7")).unwrap();
    let again = open(&dir, "0.0.0");
    assert!(
        ids(&again).contains(&"acme/lost".to_owned()),
        "the interrupted replacement came back"
    );
    assert!(ids(&again).contains(&"acme/stay".to_owned()));
    assert!(
        !root.join(".trash").exists()
            || std::fs::read_dir(root.join(".trash"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn a_newer_version_ignores_build_metadata() {
    let library = open(&scratch("build-meta"), "0.0.0");
    install(&library, "acme/pack", "1.0.0+a").unwrap();
    assert_eq!(
        code(&install(&library, "acme/pack", "1.0.0+b").unwrap_err()),
        "already_installed"
    );
}
