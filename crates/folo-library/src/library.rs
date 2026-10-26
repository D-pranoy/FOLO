//! The library service (RB-09 step 1, ADR-0038 D1 to D10): the built-in
//! packs, and the installed and local packs in `library/<author>/<name>/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use semver::Version;
use serde_json::{Value, json};

use crate::archive::{self, Entries};
use crate::compat;
use crate::disk;
use crate::error::LibraryError;
use crate::limits;
use crate::names::{self, ItemType, MANIFEST};
use crate::pack::{self, Assembled, Item, Pack};
use crate::{builtin, edit};

/// The pack the node keeps for the user's presets (ADR-0038 D1).
pub const MY_PRESETS: &str = "local/my-presets";
/// What `library` events keep: the newest.
const EVENTS_KEPT: usize = 256;

/// What happened to a pack (`event {kind: library, state}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Installed,
    Removed,
    Changed,
}

impl State {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Removed => "removed",
            Self::Changed => "changed",
        }
    }
}

/// An event for the live channel, numbered so each connection sends it once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEvent {
    pub seq: u64,
    /// The pack's id.
    pub id: String,
    pub state: &'static str,
}

struct Inner {
    packs: BTreeMap<String, Pack>,
    /// Folders that failed their checks at start: left as they are.
    unreadable: BTreeSet<String>,
}

/// The numbered events, apart from the packs so that reading them (the live
/// channel does, many times a second) never waits for a write.
#[derive(Default)]
struct EventLog {
    events: Vec<LibraryEvent>,
    seq: u64,
}

/// The content library.
pub struct Library {
    root: PathBuf,
    node_version: Version,
    inner: Mutex<Inner>,
    events: Mutex<EventLog>,
}

impl Library {
    /// Reads the library at `root` (the data folder's `library/`): the
    /// built-in packs, then every pack folder; a folder that fails is
    /// skipped and logged, because packs are content, not configuration
    /// (ADR-0038 D3). Creates `local/my-presets` when its folder is missing.
    #[must_use]
    pub fn open(root: PathBuf, node_version: Version) -> Self {
        disk::recover(&root);
        let mut packs = BTreeMap::new();
        let mut unreadable = BTreeSet::new();
        load_builtins(&mut packs);
        load_folders(&root, &mut packs, &mut unreadable);
        let library = Self {
            root,
            node_version,
            inner: Mutex::new(Inner { packs, unreadable }),
            events: Mutex::new(EventLog::default()),
        };
        library.create_my_presets();
        library
    }

    fn events_lock(&self) -> MutexGuard<'_, EventLog> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn announce(&self, id: &str, state: State) {
        let mut log = self.events_lock();
        log.seq += 1;
        let seq = log.seq;
        log.events.push(LibraryEvent {
            seq,
            id: id.to_owned(),
            state: state.name(),
        });
        if log.events.len() > EVENTS_KEPT {
            log.events.remove(0);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the empty `local/my-presets`, only when its folder doesn't exist.
    fn create_my_presets(&self) {
        if disk::folder(&self.root, MY_PRESETS).exists() {
            return;
        }
        let manifest = json!({
            "id": MY_PRESETS, "version": "1.0.0", "folo": ">=1.0 <2", "name": "My presets",
            "description": "Presets saved on this node.", "license": "MIT", "authors": [],
            "contents": {}, "files": {},
        });
        let mut entries = Entries::new();
        entries.insert(MANIFEST.to_owned(), manifest.to_string().into_bytes());
        let result = pack::assemble(entries, Some(MY_PRESETS), false)
            .and_then(|a| disk::place(&self.root, MY_PRESETS, &a.pack.files).map(|()| a.pack));
        match result {
            Ok(pack) => {
                self.lock().packs.insert(MY_PRESETS.to_owned(), pack);
            }
            Err(error) => log::error!("library: {MY_PRESETS} can't be created: {error}"),
        }
    }

    // ------------------------------------------------------------ reading

    /// `PackInfo` of every pack: the built-in packs first, then by id.
    #[must_use]
    pub fn packs(&self) -> Vec<Value> {
        let inner = self.lock();
        let mut packs: Vec<&Pack> = inner.packs.values().collect();
        packs.sort_by_key(|p| (!p.builtin, p.id.clone()));
        packs.into_iter().map(info_of).collect()
    }

    /// A pack's manifest in the canonical form.
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`] when there is no such pack.
    pub fn manifest(&self, id: &str) -> Result<Value, LibraryError> {
        self.lock()
            .packs
            .get(id)
            .map(|p| p.manifest.clone())
            .ok_or_else(|| not_found(id))
    }

    /// An item, as clamped.
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`] when there is no such pack or item.
    pub fn item(&self, pack: &str, item_type: ItemType, id: &str) -> Result<Value, LibraryError> {
        let inner = self.lock();
        let found = inner.packs.get(pack).ok_or_else(|| not_found(pack))?;
        found
            .item(item_type, id)
            .map(|i| i.value.clone())
            .ok_or_else(|| not_found(&format!("{pack} {} {id}", item_type.folder())))
    }

    /// `LibraryItem` of every item of a type: built-in packs first, then by
    /// pack id, each pack's items in its `contents` order.
    #[must_use]
    pub fn items(&self, item_type: ItemType) -> Vec<Value> {
        let inner = self.lock();
        let mut packs: Vec<&Pack> = inner.packs.values().collect();
        packs.sort_by_key(|p| (!p.builtin, p.id.clone()));
        packs
            .into_iter()
            .flat_map(|p| p.of_type(item_type).map(move |i| library_item(p, i)))
            .collect()
    }

    /// A preview's bytes and media type.
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`] when the pack has no such preview.
    pub fn preview(&self, pack: &str, file: &str) -> Result<(Vec<u8>, &'static str), LibraryError> {
        let path = format!("previews/{file}");
        // Read while holding the lock, so a replace or removal can't move the folder meanwhile
        let inner = self.lock();
        let found = inner.packs.get(pack).ok_or_else(|| not_found(pack))?;
        if !found.previews.contains_key(&path) {
            return Err(not_found(&format!("{pack} preview {file}")));
        }
        let bytes = disk::read_preview(&disk::folder(&self.root, pack), &path)?;
        Ok((bytes, media_type(file)))
    }

    /// The pack's canonical `.folopack` (ADR-0038 D9).
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`] when there is no such pack.
    pub fn export(&self, id: &str) -> Result<Vec<u8>, LibraryError> {
        let inner = self.lock();
        let pack = inner.packs.get(id).ok_or_else(|| not_found(id))?;
        let previews = self.previews_of(pack)?;
        archive::write(&pack::canonical_entries(pack, &previews)?)
    }

    fn previews_of(&self, pack: &Pack) -> Result<Entries, LibraryError> {
        // The built-in packs are embedded, not on disk
        if pack.builtin {
            return Ok(Entries::new());
        }
        let dir = disk::folder(&self.root, &pack.id);
        pack.previews
            .keys()
            .map(|path| Ok((path.clone(), disk::read_preview(&dir, path)?)))
            .collect()
    }

    /// The built-in id's reference (`fx:ledfx-bar` is `folo/effects:ledfx-bar`).
    #[must_use]
    pub fn alias(&self, id: &str) -> Option<String> {
        builtin::aliases().remove(id)
    }

    // ------------------------------------------------------------ events

    /// The number of the newest event.
    #[must_use]
    pub fn last_event(&self) -> u64 {
        self.events_lock().seq
    }

    /// The events after number `seq`.
    #[must_use]
    pub fn events_since(&self, seq: u64) -> Vec<LibraryEvent> {
        self.events_lock()
            .events
            .iter()
            .filter(|e| e.seq > seq)
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------ install and remove

    /// Installs a `.folopack` (ADR-0038 D4 to D8); `PackInfo` of the result.
    ///
    /// # Errors
    ///
    /// The errors of `POST /library/packs`.
    pub fn install(&self, bytes: &[u8], replace: bool) -> Result<Value, LibraryError> {
        let entries = archive::read(bytes)?;
        let Assembled { pack, previews } = pack::assemble(entries, None, false)?;
        if matches!(names::author(&pack.id), "folo" | "local") {
            return Err(LibraryError::ReservedId { id: pack.id });
        }
        self.check_compatible(&pack)?;
        let mut inner = self.lock();
        check_requires(&inner, &pack)?;
        check_conflict(&inner, &pack, replace)?;
        check_room(&inner, &pack)?;
        let mut files = pack.files.clone();
        files.extend(previews);
        disk::place(&self.root, &pack.id, &files)?;
        let info = info_of(&pack);
        let id = pack.id.clone();
        inner.packs.insert(id.clone(), pack);
        inner.unreadable.remove(&id);
        self.announce(&id, State::Installed);
        Ok(info)
    }

    fn check_compatible(&self, pack: &Pack) -> Result<(), LibraryError> {
        if self.node_version == Version::new(0, 0, 0) {
            log::warn!(
                "library: a development build doesn't check {}'s folo range",
                pack.id
            );
            return Ok(());
        }
        let range = pack.manifest["folo"].as_str().unwrap_or_default();
        let parsed = compat::parse_range(range).map_err(|e| {
            LibraryError::structural(MANIFEST, format!("folo isn't a version range ({e})"))
        })?;
        if compat::meets(&parsed, &self.node_version) {
            Ok(())
        } else {
            Err(LibraryError::Incompatible {
                folo: range.to_owned(),
            })
        }
    }

    /// Removes an installed or local pack.
    ///
    /// # Errors
    ///
    /// The errors of `DELETE /library/packs/{pack}`.
    pub fn remove(&self, id: &str) -> Result<(), LibraryError> {
        let mut inner = self.lock();
        let pack = inner.packs.get(id).ok_or_else(|| not_found(id))?;
        if pack.builtin || id == MY_PRESETS {
            return Err(LibraryError::ProtectedPack { id: id.to_owned() });
        }
        let needed_by: Vec<String> = inner
            .packs
            .values()
            .filter(|p| p.id != id && requires_of(p).iter().any(|(r, _)| r == id))
            .map(|p| p.id.clone())
            .collect();
        if !needed_by.is_empty() {
            return Err(LibraryError::RequiredBy { packs: needed_by });
        }
        disk::remove(&self.root, id)?;
        inner.packs.remove(id);
        self.announce(id, State::Removed);
        Ok(())
    }

    // ------------------------------------------------------------ items of local packs

    /// Writes an item of a local pack (ADR-0038 D10): clamped, its id the
    /// one given. Returns the item as stored.
    ///
    /// # Errors
    ///
    /// The errors of `PUT /library/packs/{pack}/items/{type}/{item}`.
    ///
    /// # Panics
    ///
    /// Never: the item it returns was just written.
    pub fn put_item(
        &self,
        pack: &str,
        item_type: ItemType,
        id: &str,
        body: Value,
    ) -> Result<Value, LibraryError> {
        let mut inner = self.lock();
        let current = editable(&inner, pack)?;
        let previews = self.previews_of(current)?;
        let entries = edit::with_item(current, &previews, item_type, id, body)?;
        let stored = self.commit(&mut inner, pack, entries)?;
        let item = stored
            .item(item_type, id)
            .expect("the item was just written");
        Ok(item.value.clone())
    }

    /// Removes an item of a local pack.
    ///
    /// # Errors
    ///
    /// The errors of `DELETE /library/packs/{pack}/items/{type}/{item}`.
    pub fn delete_item(
        &self,
        pack: &str,
        item_type: ItemType,
        id: &str,
    ) -> Result<(), LibraryError> {
        let mut inner = self.lock();
        let current = editable(&inner, pack)?;
        let previews = self.previews_of(current)?;
        let entries = edit::without_item(current, &previews, item_type, id)?;
        self.commit(&mut inner, pack, entries)?;
        Ok(())
    }

    /// Checks the edited pack as an install does, writes it, and swaps it in.
    fn commit<'a>(
        &self,
        inner: &'a mut MutexGuard<'_, Inner>,
        pack: &str,
        entries: Entries,
    ) -> Result<&'a Pack, LibraryError> {
        let Assembled {
            pack: edited,
            previews,
        } = pack::assemble(entries, Some(pack), false)?;
        // An edit can't take the library past its size either
        let others: u64 = inner
            .packs
            .values()
            .filter(|p| !p.builtin && p.id != pack)
            .map(|p| p.bytes)
            .sum();
        if others + edited.bytes > limits::LIBRARY_BYTES {
            return Err(LibraryError::ListFull {
                reason: "the library holds 128 MB".to_owned(),
            });
        }
        let mut files = edited.files.clone();
        files.extend(previews);
        disk::place(&self.root, pack, &files)?;
        inner.packs.insert(pack.to_owned(), edited);
        self.announce(pack, State::Changed);
        Ok(&inner.packs[pack])
    }
}

/// The local pack `id` that can be edited.
fn editable<'a>(inner: &'a Inner, id: &str) -> Result<&'a Pack, LibraryError> {
    match inner.packs.get(id) {
        None if inner.unreadable.contains(id) && names::author(id) == "local" => {
            Err(LibraryError::PackUnreadable { id: id.to_owned() })
        }
        None => Err(not_found(id)),
        Some(pack) if !pack.editable() => Err(LibraryError::ReadOnlyPack { id: id.to_owned() }),
        Some(pack) => Ok(pack),
    }
}

fn not_found(what: &str) -> LibraryError {
    LibraryError::NotFound {
        what: what.to_owned(),
    }
}

fn info_of(pack: &Pack) -> Value {
    json!({
        "id": pack.id,
        "version": pack.version(),
        "name": pack.manifest["name"],
        "description": pack.manifest["description"],
        "builtin": pack.builtin,
        "editable": pack.editable(),
        "items": pack.items.len(),
        "hash": pack.hash,
    })
}

fn library_item(pack: &Pack, item: &Item) -> Value {
    json!({
        "ref": format!("{}:{}", pack.id, item.id),
        "pack": pack.id,
        "version": pack.version(),
        "type": item.item_type.folder(),
        "hash": item.hash,
        "preview": pack.preview_of(&item.id),
        "item": item.value,
    })
}

fn media_type(file: &str) -> &'static str {
    match file.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/gif",
    }
}

/// A pack's `requires`: pack id and range.
fn requires_of(pack: &Pack) -> Vec<(String, String)> {
    pack.manifest["requires"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(id, range)| Some((id.clone(), range.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `requires` entry is a pack installed in a matching version.
fn check_requires(inner: &Inner, pack: &Pack) -> Result<(), LibraryError> {
    let missing: Vec<(String, String)> = requires_of(pack)
        .into_iter()
        .filter(|(id, range)| {
            let installed = inner
                .packs
                .get(id)
                .and_then(|p| Version::parse(p.version()).ok());
            let wanted = compat::parse_range(range).ok();
            !matches!((installed, wanted), (Some(v), Some(r)) if compat::meets(&r, &v))
        })
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(LibraryError::MissingDependency { requires: missing })
    }
}

/// A pack with an installed id replaces it when newer, else needs `replace`.
fn check_conflict(inner: &Inner, pack: &Pack, replace: bool) -> Result<(), LibraryError> {
    let Some(installed) = inner.packs.get(&pack.id) else {
        return Ok(());
    };
    let newer = match (
        Version::parse(pack.version()),
        Version::parse(installed.version()),
    ) {
        (Ok(offered), Ok(have)) => offered.cmp_precedence(&have).is_gt(),
        _ => false,
    };
    if newer || replace {
        return Ok(());
    }
    Err(LibraryError::AlreadyInstalled {
        installed: installed.version().to_owned(),
        offered: pack.version().to_owned(),
    })
}

/// At most 100 installed and local packs, and 128 MB in all.
fn check_room(inner: &Inner, pack: &Pack) -> Result<(), LibraryError> {
    let others = inner
        .packs
        .values()
        .filter(|p| !p.builtin && p.id != pack.id);
    let (count, bytes) = others.fold((0usize, 0u64), |(c, b), p| (c + 1, b + p.bytes));
    if count + 1 > limits::LIBRARY_PACKS {
        return Err(LibraryError::LibraryFull {
            reason: format!("{} packs", limits::LIBRARY_PACKS),
        });
    }
    if bytes + pack.bytes > limits::LIBRARY_BYTES {
        return Err(LibraryError::LibraryFull {
            reason: "128 MB".to_owned(),
        });
    }
    Ok(())
}

fn load_builtins(packs: &mut BTreeMap<String, Pack>) {
    for (id, entries) in builtin::packs() {
        match pack::assemble(entries, Some(&id), true) {
            Ok(assembled) => {
                packs.insert(id, assembled.pack);
            }
            Err(error) => log::error!("library: the built-in pack {id} is skipped: {error}"),
        }
    }
}

fn load_folders(
    root: &std::path::Path,
    packs: &mut BTreeMap<String, Pack>,
    unreadable: &mut BTreeSet<String>,
) {
    for (id, dir) in disk::pack_folders(root) {
        let result = if matches!(names::author(&id), "folo") || packs.contains_key(&id) {
            Err(LibraryError::ReservedId { id: id.clone() })
        } else {
            disk::read_folder(&dir).and_then(|entries| pack::assemble(entries, Some(&id), false))
        };
        match result {
            Ok(assembled) => {
                packs.insert(id, assembled.pack);
            }
            Err(error) => {
                log::error!("library: {} is skipped: {error}", dir.display());
                unreadable.insert(id);
            }
        }
    }
}
