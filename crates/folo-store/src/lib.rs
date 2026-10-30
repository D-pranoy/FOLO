//! The node's documents (RB-06 step 4, docs/specs/config-model.md#documents):
//! versioned JSON documents in the data folder, clamped on the way in, with
//! a revision and server id each, written atomically after a short delay.
//! `secrets.json` is written at once, readable by its owner only.

pub mod files;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use folo_protocol::clamp::{ClampError, clamp};
use serde_json::{Map, Value};

/// How long a change waits before its document is written. Changes in the
/// meantime join that write rather than postponing it, so a document being
/// edited continuously is written at most this often.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long a failed write waits before it is tried again.
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// A document the store keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Doc {
    Zones,
    Show,
    Providers,
    Sources,
    Clients,
}

impl Doc {
    pub const ALL: [Self; 5] = [
        Self::Zones,
        Self::Show,
        Self::Providers,
        Self::Sources,
        Self::Clients,
    ];

    /// Its file name in the data folder.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Zones => "zones.json",
            Self::Show => "show.json",
            Self::Providers => "providers.json",
            Self::Sources => "sources.json",
            Self::Clients => "clients.json",
        }
    }

    /// The schema definition it's clamped with.
    #[must_use]
    pub const fn definition(self) -> &'static str {
        match self {
            Self::Zones => "ZonesDocument",
            Self::Show => "ShowDocument",
            Self::Providers => "ProvidersDocument",
            Self::Sources => "SourcesDocument",
            Self::Clients => "ClientsDocument",
        }
    }
}

const SECRETS_FILE: &str = "secrets.json";
const SECRETS_DEFINITION: &str = "SecretsDocument";

/// Why a document can't be read or written.
#[derive(Debug)]
pub enum StoreError {
    /// A structural error: the file isn't JSON, its root isn't an object, or
    /// it's from a newer FOLO. The node doesn't start (ADR-0034 D11).
    Structural { file: PathBuf, why: String },
    /// Reading or writing failed.
    Io { file: PathBuf, error: io::Error },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Structural { file, why } => write!(f, "{} can't be read: {why}", file.display()),
            Self::Io { file, error } => write!(f, "{}: {error}", file.display()),
        }
    }
}

impl std::error::Error for StoreError {}

/// A new id, as the clampers make them: 8 lowercase hexadecimal characters.
///
/// # Panics
///
/// When the operating system's random source fails.
#[must_use]
pub fn new_id() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("the operating system has a random source");
    files::hex(&bytes)
}

/// `value` clamped as `definition`, with new ids where they're missing.
fn clamped(definition: &str, value: &Value, file: &Path) -> Result<Value, StoreError> {
    clamp(definition, value, &mut new_id).map_err(|error| StoreError::Structural {
        file: file.to_path_buf(),
        why: match error {
            ClampError::Structural(why) | ClampError::UnknownDefinition(why) => why,
        },
    })
}

/// Parses and clamps a document's bytes.
fn parse_document(bytes: &[u8], path: &Path, definition: &str) -> Result<Value, StoreError> {
    let parsed = serde_json::from_slice::<Value>(bytes).map_err(|e| StoreError::Structural {
        file: path.to_path_buf(),
        why: format!("it isn't JSON ({e})"),
    })?;
    clamped(definition, &parsed, path)
}

/// A document's bytes (a backup's) parsed and clamped, not stored.
///
/// # Errors
///
/// [`StoreError::Structural`] when they aren't JSON, aren't an object, or
/// are from a newer FOLO.
pub fn parse(doc: Doc, bytes: &[u8]) -> Result<Value, StoreError> {
    parse_document(bytes, Path::new(doc.file()), doc.definition())
}

/// Reads and clamps a document file; a missing file gives the defaults.
fn read_document(path: &Path, definition: &str) -> Result<Value, StoreError> {
    let parsed = match std::fs::read(path) {
        Ok(bytes) => return parse_document(&bytes, path, definition),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => {
            return Err(StoreError::Io {
                file: path.to_path_buf(),
                error,
            });
        }
    };
    clamped(definition, &parsed, path)
}

/// One document from `dir`, clamped, without opening a store: what
/// `folo status` reads when no node runs.
///
/// # Errors
///
/// [`StoreError::Structural`] or [`StoreError::Io`].
pub fn read(dir: &Path, doc: Doc) -> Result<Value, StoreError> {
    read_document(&dir.join(doc.file()), doc.definition())
}

/// How documents are written: indented UTF-8 JSON with a final newline, as
/// the migration writes them.
///
/// # Panics
///
/// Never: a JSON value always encodes.
#[must_use]
pub fn encode(document: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(document).expect("a JSON value always encodes");
    out.push(b'\n');
    out
}

fn revision_of(document: &Value) -> u64 {
    document
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn stamp(document: &mut Value, revision: u64, server_id: &str) {
    if let Value::Object(map) = document {
        map.insert("revision".into(), Value::from(revision));
        map.insert("server_id".into(), Value::from(server_id));
    }
}

struct State {
    docs: [Arc<Value>; 5],
    dirty: BTreeSet<Doc>,
    due: Option<Instant>,
    closing: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Held while files are written, so the writer thread and `flush` don't overlap.
    writing: Mutex<()>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The node's documents in a data folder.
pub struct Store {
    dir: PathBuf,
    server_id: String,
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
}

impl Store {
    /// Opens the documents in `dir`. Missing ones start from their defaults
    /// and are written once they change.
    ///
    /// # Errors
    ///
    /// [`StoreError::Structural`] for a document the node can't read, which
    /// is left as it is, or [`StoreError::Io`].
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let server_id = new_id();
        let mut docs: [Arc<Value>; 5] = std::array::from_fn(|_| Arc::new(Value::Null));
        for doc in Doc::ALL {
            let mut value = read_document(&dir.join(doc.file()), doc.definition())?;
            let revision = revision_of(&value);
            stamp(&mut value, revision, &server_id);
            docs[index(doc)] = Arc::new(value);
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                docs,
                dirty: BTreeSet::new(),
                due: None,
                closing: false,
            }),
            wake: Condvar::new(),
            writing: Mutex::new(()),
        });
        let writer = {
            let shared = Arc::clone(&shared);
            let folder = dir.to_path_buf();
            std::thread::Builder::new()
                .name("store-writer".into())
                .spawn(move || write_when_due(&shared, &folder))
                .map_err(|error| StoreError::Io {
                    file: dir.to_path_buf(),
                    error,
                })?
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            server_id,
            shared,
            writer: Some(writer),
        })
    }

    /// The folder the documents are in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This process's server id, stamped on every document.
    #[must_use]
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// A document as it is now.
    #[must_use]
    pub fn get(&self, doc: Doc) -> Arc<Value> {
        Arc::clone(&locked(&self.shared.state).docs[index(doc)])
    }

    /// Replaces a document. It is clamped, gets the next revision and this
    /// server's id, and is written after [`SAVE_DELAY`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Structural`] when `value` can't be a document; the
    /// document stays as it was.
    pub fn replace(&self, doc: Doc, value: &Value) -> Result<Arc<Value>, StoreError> {
        self.update(doc, |_| Ok::<_, StoreError>(value.clone()))
    }

    /// Changes a document from its current value, atomically: `change`
    /// gets the document as it is and returns the new one, which is then
    /// stored as [`Store::replace`] stores it. No other change can come in
    /// between, so `change` can check the revision a writer last saw.
    ///
    /// # Errors
    ///
    /// The error `change` returns, or [`StoreError::Structural`] when the
    /// new value can't be a document; either way the document stays as it was.
    pub fn update<E: From<StoreError>>(
        &self,
        doc: Doc,
        change: impl FnOnce(&Value) -> Result<Value, E>,
    ) -> Result<Arc<Value>, E> {
        let mut state = locked(&self.shared.state);
        let current = Arc::clone(&state.docs[index(doc)]);
        let changed = change(&current)?;
        let mut value = clamped(doc.definition(), &changed, &self.dir.join(doc.file()))?;
        stamp(&mut value, revision_of(&current) + 1, &self.server_id);
        let value = Arc::new(value);
        state.docs[index(doc)] = Arc::clone(&value);
        state.dirty.insert(doc);
        if state.due.is_none() {
            state.due = Some(Instant::now() + SAVE_DELAY);
            self.shared.wake.notify_all();
        }
        Ok(value)
    }

    /// Replaces documents as a restore does: each gets the next revision and
    /// this server's id, and `place` is given their files' bytes to put in
    /// the folder itself. No document is written and none changes meanwhile;
    /// when `place` fails they stay as they were.
    ///
    /// # Errors
    ///
    /// What `place` returns.
    pub fn restore<E>(
        &self,
        documents: Vec<(Doc, Value)>,
        place: impl FnOnce(&[(Doc, Vec<u8>)]) -> Result<(), E>,
    ) -> Result<(), E> {
        // The order of the writer, so a write in flight finishes first
        let _writing = locked(&self.shared.writing);
        let mut state = locked(&self.shared.state);
        let stamped: Vec<(Doc, Arc<Value>)> = documents
            .into_iter()
            .map(|(doc, mut value)| {
                stamp(
                    &mut value,
                    revision_of(&state.docs[index(doc)]) + 1,
                    &self.server_id,
                );
                (doc, Arc::new(value))
            })
            .collect();
        let files: Vec<(Doc, Vec<u8>)> = stamped
            .iter()
            .map(|(doc, value)| (*doc, encode(value)))
            .collect();
        place(&files)?;
        for (doc, value) in stamped {
            state.docs[index(doc)] = value;
            state.dirty.remove(&doc);
        }
        Ok(())
    }

    /// Writes every changed document now.
    ///
    /// # Errors
    ///
    /// The first write that failed; the documents that couldn't be written
    /// are tried again later.
    pub fn flush(&self) -> Result<(), StoreError> {
        write_dirty(&self.shared, &self.dir)
    }

    /// Reads the documents again from their files (`folo reload`), after
    /// writing pending changes. Documents whose content changed take a new
    /// revision. Returns those that changed.
    ///
    /// # Errors
    ///
    /// [`StoreError::Structural`] or [`StoreError::Io`]; then no document changes.
    pub fn reload(&self) -> Result<Vec<Doc>, StoreError> {
        // No write between flushing and reading, so every file holds what
        // was in memory when it was read
        let _writing = locked(&self.shared.writing);
        write_pending(&self.shared, &self.dir)?;
        let mut fresh = Vec::with_capacity(Doc::ALL.len());
        for doc in Doc::ALL {
            fresh.push(read_document(&self.dir.join(doc.file()), doc.definition())?);
        }
        let mut state = locked(&self.shared.state);
        let mut changed = Vec::new();
        for (doc, mut value) in Doc::ALL.into_iter().zip(fresh) {
            let current = &state.docs[index(doc)];
            // Replaced while the files were read: newer than its file
            if state.dirty.contains(&doc) || content(&value) == content(current) {
                continue;
            }
            let revision = revision_of(&value).max(revision_of(current) + 1);
            stamp(&mut value, revision, &self.server_id);
            state.docs[index(doc)] = Arc::new(value);
            state.dirty.insert(doc);
            changed.push(doc);
        }
        if !changed.is_empty() && state.due.is_none() {
            state.due = Some(Instant::now() + SAVE_DELAY);
            self.shared.wake.notify_all();
        }
        Ok(changed)
    }

    /// Writes pending changes and stops the writer thread.
    ///
    /// # Errors
    ///
    /// The first write that failed.
    pub fn close(mut self) -> Result<(), StoreError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), StoreError> {
        {
            let mut state = locked(&self.shared.state);
            state.closing = true;
            self.shared.wake.notify_all();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        self.flush()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if self.writer.is_some()
            && let Err(error) = self.stop()
        {
            log::error!("{error}");
        }
    }
}

const fn index(doc: Doc) -> usize {
    match doc {
        Doc::Zones => 0,
        Doc::Show => 1,
        Doc::Providers => 2,
        Doc::Sources => 3,
        Doc::Clients => 4,
    }
}

/// A document without its revision and server id.
fn content(document: &Value) -> Value {
    let mut out = document.clone();
    if let Value::Object(map) = &mut out {
        map.remove("revision");
        map.remove("server_id");
    }
    out
}

/// The writer thread: writes changed documents once they're due.
fn write_when_due(shared: &Shared, dir: &Path) {
    let mut state = locked(&shared.state);
    loop {
        if state.closing {
            return;
        }
        match state.due {
            Some(due) if Instant::now() >= due => {
                drop(state);
                if let Err(error) = write_dirty(shared, dir) {
                    log::error!("{error}");
                }
                state = locked(&shared.state);
            }
            Some(due) => {
                let wait = due.saturating_duration_since(Instant::now());
                state = shared
                    .wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            None => {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
}

fn write_dirty(shared: &Shared, dir: &Path) -> Result<(), StoreError> {
    let _writing = locked(&shared.writing);
    write_pending(shared, dir)
}

/// [`write_dirty`] for a caller holding `shared.writing`.
fn write_pending(shared: &Shared, dir: &Path) -> Result<(), StoreError> {
    let pending: Vec<(Doc, Arc<Value>)> = {
        let mut state = locked(&shared.state);
        state.due = None;
        let dirty = std::mem::take(&mut state.dirty);
        dirty
            .into_iter()
            .map(|doc| (doc, Arc::clone(&state.docs[index(doc)])))
            .collect()
    };
    let mut first_error = None;
    for (doc, value) in pending {
        let path = dir.join(doc.file());
        if let Err(error) = files::write_atomic(&path, &encode(&value), false) {
            let mut state = locked(&shared.state);
            state.dirty.insert(doc);
            state
                .due
                .get_or_insert_with(|| Instant::now() + RETRY_DELAY);
            shared.wake.notify_all();
            first_error.get_or_insert(StoreError::Io { file: path, error });
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// `secrets.json` in `dir`, clamped; missing, its defaults. An existing file
/// is made owner-only again, in case something widened its permissions.
///
/// # Errors
///
/// [`StoreError::Structural`] or [`StoreError::Io`].
pub fn read_secrets(dir: &Path) -> Result<Value, StoreError> {
    let path = dir.join(SECRETS_FILE);
    if path.exists() {
        files::restrict(&path).map_err(|error| StoreError::Io {
            file: path.clone(),
            error,
        })?;
    }
    read_document(&path, SECRETS_DEFINITION)
}

/// Writes `secrets.json` in `dir` at once, owner-only, with the next revision
/// and `server_id`.
///
/// # Errors
///
/// [`StoreError::Structural`] when `value` can't be the document, or
/// [`StoreError::Io`].
pub fn write_secrets(dir: &Path, value: &Value, server_id: &str) -> Result<Value, StoreError> {
    let path = dir.join(SECRETS_FILE);
    let mut value = clamped(SECRETS_DEFINITION, value, &path)?;
    let revision = revision_of(&value) + 1;
    stamp(&mut value, revision, server_id);
    files::write_atomic(&path, &encode(&value), true)
        .map_err(|error| StoreError::Io { file: path, error })?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh folder under the system's temporary folder, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("folo-store-{}", new_id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[derive(Debug)]
    enum Refused {
        Stale,
        Store(StoreError),
    }

    impl From<StoreError> for Refused {
        fn from(error: StoreError) -> Self {
            Self::Store(error)
        }
    }

    #[test]
    fn an_update_sees_the_current_document_and_can_refuse() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        let seen = store.get(Doc::Zones)["revision"].as_u64().unwrap();
        let updated = store
            .update(Doc::Zones, |current| {
                assert_eq!(current["revision"], seen);
                let mut next = current.clone();
                next["zones"] = json!([{"id": "z1", "name": "Desk"}]);
                Ok::<_, Refused>(next)
            })
            .unwrap();
        assert_eq!(updated["revision"], seen + 1);
        assert_eq!(updated["zones"][0]["name"], "Desk");
        // A writer that saw an older revision is refused, and nothing changes
        let refused = store.update(Doc::Zones, |current| {
            if current["revision"] == seen {
                Ok(current.clone())
            } else {
                Err(Refused::Stale)
            }
        });
        assert!(matches!(refused, Err(Refused::Stale)));
        assert_eq!(store.get(Doc::Zones)["revision"], seen + 1);
        // A value that can't be a document is refused by the store
        let structural = store.update(Doc::Zones, |_| Ok::<_, Refused>(json!([])));
        assert!(matches!(
            structural,
            Err(Refused::Store(StoreError::Structural { .. }))
        ));
        assert_eq!(store.get(Doc::Zones)["revision"], seen + 1);
    }

    #[test]
    fn missing_documents_start_from_defaults_and_stay_unwritten() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        let zones = store.get(Doc::Zones);
        assert_eq!(zones["schema_version"], 2);
        assert_eq!(zones["revision"], 0);
        assert_eq!(zones["server_id"], store.server_id());
        assert_eq!(zones["zones"], json!([]));
        store.close().unwrap();
        assert!(!dir.0.join("zones.json").exists());
    }

    #[test]
    fn changes_are_clamped_versioned_and_written_after_the_delay() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        let written = store
            .replace(
                Doc::Providers,
                &json!({"providers": [{"kind": "hyperion", "rate_hz": 500}], "junk": 1}),
            )
            .unwrap();
        assert_eq!(written["revision"], 1);
        assert_eq!(written["providers"][0]["rate_hz"], 100);
        assert_eq!(written["providers"][0]["id"].as_str().unwrap().len(), 8);
        assert!(written.get("junk").is_none());
        let path = dir.0.join("providers.json");
        assert!(!path.exists(), "written before the delay");
        std::thread::sleep(SAVE_DELAY * 3);
        assert_eq!(read(&path), *written);
        store.replace(Doc::Providers, &written).unwrap();
        store.close().unwrap();
        assert_eq!(read(&path)["revision"], 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\n  \"") && text.ends_with("}\n"));
    }

    #[test]
    fn documents_keep_their_revision_across_opens() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        store.replace(Doc::Show, &json!({})).unwrap();
        store.replace(Doc::Show, &json!({})).unwrap();
        let first = store.server_id().to_owned();
        store.close().unwrap();
        let store = Store::open(&dir.0).unwrap();
        assert_eq!(store.get(Doc::Show)["revision"], 2);
        assert_ne!(store.server_id(), first);
        assert_eq!(store.get(Doc::Show)["server_id"], store.server_id());
    }

    #[test]
    fn structural_errors_stop_the_open_and_leave_the_file() {
        let cases: [&[u8]; 3] = [b"{not json", b"[1, 2]", br#"{"schema_version": 3}"#];
        for bytes in cases {
            let dir = TempDir::new();
            let path = dir.0.join("zones.json");
            std::fs::write(&path, bytes).unwrap();
            let error = Store::open(&dir.0).err().expect("refused");
            assert!(matches!(error, StoreError::Structural { .. }), "{error}");
            assert!(error.to_string().contains("zones.json"), "{error}");
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn reload_takes_changed_files_with_a_new_revision() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        store.replace(Doc::Sources, &json!({})).unwrap();
        store.flush().unwrap();
        assert_eq!(store.reload().unwrap(), []);
        let path = dir.0.join("sources.json");
        let mut edited = read(&path);
        edited["sources"] = json!([{"id": "local", "name": "Here"}]);
        std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
        assert_eq!(store.reload().unwrap(), [Doc::Sources]);
        let sources = store.get(Doc::Sources);
        assert_eq!(sources["sources"][0]["name"], "Here");
        assert_eq!(sources["revision"], 2);
        store.close().unwrap();
    }

    #[test]
    fn reload_refuses_a_broken_file_and_keeps_the_documents() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();
        std::fs::write(dir.0.join("show.json"), b"[]").unwrap();
        assert!(matches!(store.reload(), Err(StoreError::Structural { .. })));
        assert_eq!(store.get(Doc::Show)["revision"], 0);
        drop(store);
    }

    #[test]
    fn secrets_are_owner_only() {
        let dir = TempDir::new();
        assert_eq!(read_secrets(&dir.0).unwrap()["identity"]["public_key"], "");
        let written = write_secrets(
            &dir.0,
            &json!({"identity": {"public_key": "pk", "private_key": "sk"}}),
            "abcd1234",
        )
        .unwrap();
        assert_eq!(written["revision"], 1);
        let path = dir.0.join(SECRETS_FILE);
        assert_eq!(
            read_secrets(&dir.0).unwrap()["identity"]["private_key"],
            "sk"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            let output = std::process::Command::new("icacls")
                .arg(&path)
                .output()
                .unwrap();
            let listing = String::from_utf8_lossy(&output.stdout);
            let entries: Vec<_> = listing.lines().filter(|line| line.contains(":(")).collect();
            assert_eq!(entries.len(), 1, "{listing}");
            assert!(entries[0].contains("(F)"), "{listing}");
        }
        let written = write_secrets(&dir.0, &written, "abcd1234").unwrap();
        assert_eq!(written["revision"], 2);
    }
}
