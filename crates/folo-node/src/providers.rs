//! The provider kinds a node registers (RB-10, RB-12, RB-13), and what it keeps of their
//! credentials in `secrets.json` ([security](../../docs/architecture/security.md)).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use folo_protocol::generated::{ProviderInstanceKind, ProviderSecrets};
use folo_provider::Registry;
use folo_provider_hue::trust::{Fingerprint, PinStore, fingerprint_hex, parse_fingerprint};
use folo_provider_hue::{HueKind, HueOptions};
use folo_provider_hyperion::HyperionKind;
use folo_provider_wled::{DEFAULT_RATE_HZ as WLED_RATE_HZ, WledKind};
use folo_store::{Store, StoreError};
use serde_json::{Map, Value};

/// The shared provider and capture secrets. Updates go one at a time, because a
/// write is a read, a change and a write of the whole file; the identity is not touched.
pub struct SecretsFile {
    dir: PathBuf,
    server_id: String,
    one_at_a_time: Mutex<()>,
    #[cfg(test)]
    update_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl SecretsFile {
    /// The secrets in the folder of `store`, stamped with its server id.
    #[must_use]
    pub fn of(store: &Store) -> Arc<Self> {
        Self::in_dir(store.dir(), store.server_id())
    }

    /// The secrets in `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path, server_id: &str) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.to_path_buf(),
            server_id: server_id.to_owned(),
            one_at_a_time: Mutex::new(()),
            #[cfg(test)]
            update_started: Mutex::new(None),
        })
    }

    /// Changes the document with `change`, and writes it only when it came out different.
    ///
    /// # Errors
    /// [`StoreError`] when the file can't be read or written.
    pub fn update(&self, change: impl FnOnce(&mut Value)) -> Result<(), StoreError> {
        #[cfg(test)]
        if let Some(observer) = self
            .update_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = observer.send(());
        }
        let _turn = self
            .one_at_a_time
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut secrets = folo_store::read_secrets(&self.dir)?;
        let before = secrets.clone();
        change(&mut secrets);
        if secrets != before {
            folo_store::write_secrets(&self.dir, &secrets, &self.server_id)?;
        }
        Ok(())
    }

    /// Lets a concurrency test observe an update before it waits for the file lock.
    #[cfg(test)]
    pub(crate) fn observe_updates(&self) -> std::sync::mpsc::Receiver<()> {
        let (send, receive) = std::sync::mpsc::channel();
        *self
            .update_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(send);
        receive
    }

    /// The document as stored.
    #[must_use]
    pub fn read(&self) -> Value {
        folo_store::read_secrets(&self.dir).unwrap_or_default()
    }

    /// Removes an instance's credentials, when it is deleted.
    pub fn forget_provider(&self, instance: &str) {
        let forgotten = self.update(|secrets| {
            object_at(secrets, "providers").remove(instance);
        });
        if let Err(error) = forgotten {
            log::warn!("provider {instance}: its credentials can't be removed: {error}");
        }
    }

    /// Keeps the credentials a provider now holds with its instance: what it has is put over
    /// what was stored, and what it doesn't have is left as it was.
    pub fn keep_provider(&self, instance: &str, held: &ProviderSecrets) {
        let Ok(Value::Object(held)) = serde_json::to_value(held) else {
            return;
        };
        let kept = self.update(|secrets| {
            let providers = object_at(secrets, "providers");
            let own = object_at_map(providers, instance);
            own.extend(held);
        });
        if let Err(error) = kept {
            log::warn!("provider {instance}: its credentials can't be kept: {error}");
        }
    }
}

fn object_at<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !value[key].is_object() {
        value[key] = Value::Object(Map::new());
    }
    value[key]
        .as_object_mut()
        .expect("it was just made an object")
}

fn object_at_map<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let entry = map
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("it was just made an object")
}

/// The certificates people confirmed for Hue bridges, kept in `secrets.json`
/// (`pinned_certificates`, by lowercase bridge id).
pub struct SecretsPins {
    secrets: Arc<SecretsFile>,
}

impl SecretsPins {
    #[must_use]
    pub fn new(secrets: Arc<SecretsFile>) -> Self {
        Self { secrets }
    }
}

impl PinStore for SecretsPins {
    fn pinned(&self, bridge_id: &str) -> Option<Fingerprint> {
        let secrets = self.secrets.read();
        let found = secrets["pinned_certificates"][bridge_id.to_ascii_lowercase()].as_str()?;
        parse_fingerprint(found)
    }

    fn pin(&self, bridge_id: &str, fingerprint: Fingerprint) {
        let id = bridge_id.to_ascii_lowercase();
        let kept = self.secrets.update(|secrets| {
            object_at(secrets, "pinned_certificates")
                .insert(id.clone(), Value::String(fingerprint_hex(&fingerprint)));
        });
        if let Err(error) = kept {
            log::warn!("bridge {id}: its confirmed certificate can't be kept: {error}");
        }
    }

    fn unpin(&self, bridge_id: &str) {
        let id = bridge_id.to_ascii_lowercase();
        let kept = self.secrets.update(|secrets| {
            object_at(secrets, "pinned_certificates").remove(&id);
        });
        if let Err(error) = kept {
            log::warn!("bridge {id}: its confirmed certificate can't be forgotten: {error}");
        }
    }
}

/// The kinds a node runs: Hyperion, WLED and Hue.
#[must_use]
pub fn built_in(secrets: &Arc<SecretsFile>) -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(HyperionKind));
    registry.register(Arc::new(WledKind::new()));
    let options = HueOptions {
        pins: Arc::new(SecretsPins::new(Arc::clone(secrets))),
        ..HueOptions::default()
    };
    registry.register(Arc::new(HueKind::new(options)));
    registry
}

/// The output rate of a new instance of `kind` whose creation names none, when the kind's run
/// book sets one other than the contract's 40.
#[must_use]
pub fn default_rate_hz(kind: &str) -> Option<u32> {
    (kind == kind_name(ProviderInstanceKind::Wled)).then_some(WLED_RATE_HZ)
}

fn kind_name(kind: ProviderInstanceKind) -> &'static str {
    match kind {
        ProviderInstanceKind::Hyperion => "hyperion",
        ProviderInstanceKind::Wled => "wled",
        ProviderInstanceKind::Hue => "hue",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn secrets_in_new_dir() -> (PathBuf, Arc<SecretsFile>) {
        let dir = std::env::temp_dir().join(format!("folo-pins-{}", folo_store::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), SecretsFile::in_dir(&dir, "abcd1234"))
    }

    #[test]
    fn pins_are_kept_by_lowercase_bridge_id_and_forgotten() {
        let (dir, secrets) = secrets_in_new_dir();
        let pins = SecretsPins::new(Arc::clone(&secrets));
        assert_eq!(pins.pinned("ECB5FAFFFE000001"), None);
        pins.pin("ECB5FAFFFE000001", [7; 32]);
        // a new store over the same file sees it: it was written, not held
        let again = SecretsPins::new(SecretsFile::in_dir(&dir, "abcd1234"));
        assert_eq!(again.pinned("ecb5fafffe000001"), Some([7; 32]));
        let stored = folo_store::read_secrets(&dir).unwrap();
        assert_eq!(
            stored["pinned_certificates"]["ecb5fafffe000001"],
            "07".repeat(32)
        );
        pins.unpin("ecb5fafffe000001");
        assert_eq!(again.pinned("ecb5fafffe000001"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_providers_credentials_are_merged_into_its_entry() {
        let (dir, secrets) = secrets_in_new_dir();
        let first: ProviderSecrets =
            serde_json::from_value(json!({"app_key": "key", "client_key": "psk"})).unwrap();
        secrets.keep_provider("hue", &first);
        let later: ProviderSecrets = serde_json::from_value(json!({"app_key": "new"})).unwrap();
        secrets.keep_provider("hue", &later);
        let stored = folo_store::read_secrets(&dir).unwrap();
        assert_eq!(stored["providers"]["hue"]["app_key"], "new");
        assert_eq!(stored["providers"]["hue"]["client_key"], "psk");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_built_in_kinds_are_hyperion_wled_and_hue() {
        let (dir, secrets) = secrets_in_new_dir();
        let registry = built_in(&secrets);
        for kind in [
            ProviderInstanceKind::Hyperion,
            ProviderInstanceKind::Wled,
            ProviderInstanceKind::Hue,
        ] {
            assert!(registry.is_implemented(kind), "{kind:?}");
        }
        assert_eq!(default_rate_hz("wled"), Some(60));
        assert_eq!(default_rate_hz("hue"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
