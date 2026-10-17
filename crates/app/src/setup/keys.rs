//! Which keys the Keychain holds, by name only, as this run has seen them (D-102). Before a
//! session runs the screen reads the Keychain on every status; once one runs it doesn't (each
//! read of an unsigned build's key can prompt), so the settings page shows what this run last
//! read, saved or removed, and `NotChecked` for a key it never touched until the owner asks to
//! check. No value is ever kept here: only `Saved`, `Missing` or `Unreadable` per name.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use boardai_core::setup::{KeyName, KeyState, KeyStatus};
use boardai_runtime::llm::catalog::ListKey;
use boardai_runtime::settings::{Secret, SecretName, SecretStore};

use super::input::secret_name;

/// Every key the screen saves, in the order it lists them.
pub const KEY_NAMES: [KeyName; 3] = [KeyName::Anthropic, KeyName::OpenRouter, KeyName::ElevenLabs];

/// What the Keychain last answered for each name this run. Bounded by [`KEY_NAMES`].
#[derive(Debug, Default)]
pub struct KeyLedger {
    // Held only to read or write one entry.
    seen: Mutex<BTreeMap<KeyName, KeyState>>,
}

impl KeyLedger {
    /// Records an answer from the Keychain (a read, a save or a removal).
    pub fn record(&self, name: KeyName, state: KeyState) {
        debug_assert!(
            matches!(
                state,
                KeyState::Saved | KeyState::Missing | KeyState::Unreadable
            ),
            "only the Keychain's answers are recorded"
        );
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.insert(name, state);
    }

    /// The last answer for `name`, or `NotChecked`.
    pub fn seen(&self, name: KeyName) -> KeyState {
        let seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.get(&name).copied().unwrap_or(KeyState::NotChecked)
    }
}

/// The screen's key a settings secret name is, if it is one (not a hand-written name).
pub fn listed(secret: &SecretName) -> Option<KeyName> {
    KEY_NAMES
        .into_iter()
        .find(|key| key.secret_name() == secret.as_str())
}

/// Asks the Keychain whether `name` holds a value (it may prompt; blocks) and records it.
pub fn read(secrets: &dyn SecretStore, ledger: &KeyLedger, name: KeyName) -> KeyState {
    let Some(secret) = secret_name(name) else {
        return KeyState::Missing;
    };
    let state = presence(secrets, &secret);
    ledger.record(name, state);
    state
}

/// The key a choice needs (ElevenLabs as the voice): while a session runs, as this run last saw
/// it; read now (and recorded) before one runs, or when this run never saw it.
pub fn for_choice(
    secrets: &dyn SecretStore,
    ledger: &KeyLedger,
    name: KeyName,
    session_runs: bool,
) -> KeyState {
    match ledger.seen(name) {
        seen @ (KeyState::Saved | KeyState::Missing | KeyState::Unreadable) if session_runs => seen,
        KeyState::Saved
        | KeyState::Missing
        | KeyState::Unreadable
        | KeyState::NotChecked
        | KeyState::NotNeeded => read(secrets, ledger, name),
    }
}

/// Every key's state: read from the Keychain now, or as this run last saw it.
pub fn states(secrets: &dyn SecretStore, ledger: &KeyLedger, read_now: bool) -> Vec<KeyStatus> {
    KEY_NAMES
        .into_iter()
        .map(|name| KeyStatus {
            name,
            state: if read_now {
                read(secrets, ledger, name)
            } else {
                ledger.seen(name)
            },
        })
        .collect()
}

/// The state of the key a role names in settings: one of the screen's keys by its state in
/// `keys`, or (a name only a hand-written file uses) read now when allowed.
pub fn named(
    secrets: &dyn SecretStore,
    keys: &[KeyStatus],
    secret: &SecretName,
    read_now: bool,
) -> KeyState {
    let listed = keys
        .iter()
        .find(|key| key.name.secret_name() == secret.as_str());
    match (listed, read_now) {
        (Some(key), _) => key.state,
        (None, true) => presence(secrets, secret),
        (None, false) => KeyState::NotChecked,
    }
}

fn presence(secrets: &dyn SecretStore, name: &SecretName) -> KeyState {
    match secrets.contains(name) {
        Ok(true) => KeyState::Saved,
        Ok(false) => KeyState::Missing,
        Err(error) => {
            tracing::warn!(%error, "the Keychain could not be read for setup");
            KeyState::Unreadable
        }
    }
}

/// The OpenRouter key for the model picker's provider lists (D-218), so OpenRouter answers with
/// its providers' speeds. Read from the store only when this run has seen the key saved (never
/// to find out, which could prompt for a key that isn't there), once, and kept until it is saved
/// again or removed ([`Self::forget`]): the picker asks for several lists at once, and each
/// Keychain read of an unsigned build can prompt.
pub struct ProviderListKey {
    store: Box<dyn SecretStore>,
    ledger: Arc<KeyLedger>,
    // Held through the store's read, on the blocking pool, so lists asked together read once.
    read: Mutex<Option<Secret>>,
}

impl ProviderListKey {
    pub fn new(store: Box<dyn SecretStore>, ledger: Arc<KeyLedger>) -> Self {
        Self {
            store,
            ledger,
            read: Mutex::new(None),
        }
    }

    /// Drops the kept key: it was saved anew or removed.
    pub fn forget(&self) {
        *self
            .read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

impl std::fmt::Debug for ProviderListKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderListKey")
    }
}

impl ListKey for ProviderListKey {
    fn open_router(&self) -> Option<Secret> {
        if self.ledger.seen(KeyName::OpenRouter) != KeyState::Saved {
            return None;
        }
        let mut read = self
            .read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(secret) = read.as_ref() {
            return Some(secret.clone());
        }
        let name = secret_name(KeyName::OpenRouter)?;
        match self.store.get(&name) {
            Ok(secret) => {
                *read = Some(secret.clone());
                Some(secret)
            }
            Err(error) => {
                tracing::info!(%error, "the OpenRouter key wasn't read; provider lists go without it");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::setup::{KeyName, KeyState};
    use boardai_runtime::settings::{
        MemorySecretStore, Secret, SecretError, SecretName, SecretStore,
    };

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use boardai_runtime::llm::catalog::ListKey;

    use super::{KeyLedger, ProviderListKey, named, states};

    /// A Keychain that is locked: every read fails.
    struct Locked;

    impl SecretStore for Locked {
        fn get(&self, name: &SecretName) -> Result<Secret, SecretError> {
            Err(SecretError::Unavailable {
                name: name.clone(),
                detail: "locked".to_owned(),
            })
        }

        fn set(&self, name: &SecretName, _secret: &Secret) -> Result<(), SecretError> {
            Err(SecretError::Unwritable {
                name: name.clone(),
                detail: "locked".to_owned(),
            })
        }

        fn remove(&self, name: &SecretName) -> Result<(), SecretError> {
            Err(SecretError::Unwritable {
                name: name.clone(),
                detail: "locked".to_owned(),
            })
        }
    }

    fn name(text: &str) -> SecretName {
        SecretName::parse(text).unwrap_or_else(|| panic!("{text} is a name"))
    }

    #[test]
    fn a_running_session_shows_what_this_run_saw_and_never_reads() {
        let mut secrets = MemorySecretStore::new();
        secrets.insert(name("openrouter"), Secret::new("sk-or-x".to_owned()));
        let ledger = KeyLedger::default();
        let unread: Vec<KeyState> = states(&Locked, &ledger, false)
            .iter()
            .map(|key| key.state)
            .collect();
        assert_eq!(
            unread,
            [KeyState::NotChecked; 3],
            "the locked store was never asked"
        );
        let read: Vec<KeyState> = states(&secrets, &ledger, true)
            .iter()
            .map(|key| key.state)
            .collect();
        assert_eq!(
            read,
            [KeyState::Missing, KeyState::Saved, KeyState::Missing]
        );
        let remembered: Vec<KeyState> = states(&Locked, &ledger, false)
            .iter()
            .map(|key| key.state)
            .collect();
        assert_eq!(remembered, read);
        ledger.record(KeyName::OpenRouter, KeyState::Missing);
        assert_eq!(ledger.seen(KeyName::OpenRouter), KeyState::Missing);
    }

    /// A store that counts its reads and holds one OpenRouter key.
    struct Counting {
        reads: std::sync::Arc<std::sync::atomic::AtomicU32>,
        fails: bool,
    }

    impl SecretStore for Counting {
        fn get(&self, name: &SecretName) -> Result<Secret, SecretError> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fails {
                return Locked.get(name);
            }
            Ok(Secret::new("sk-or-list".to_owned()))
        }

        fn set(&self, name: &SecretName, secret: &Secret) -> Result<(), SecretError> {
            Locked.set(name, secret)
        }

        fn remove(&self, name: &SecretName) -> Result<(), SecretError> {
            Locked.remove(name)
        }
    }

    fn list_key(fails: bool) -> (ProviderListKey, Arc<KeyLedger>, Arc<AtomicU32>) {
        let reads = Arc::new(AtomicU32::new(0));
        let ledger = Arc::new(KeyLedger::default());
        let store = Counting {
            reads: Arc::clone(&reads),
            fails,
        };
        let key = ProviderListKey::new(Box::new(store), Arc::clone(&ledger));
        (key, ledger, reads)
    }

    /// D-218: provider lists carry the key only once this run has seen it saved, read it once
    /// for all of them, and read it again after it is saved anew or removed.
    #[test]
    fn provider_lists_read_a_saved_key_once() {
        let (key, ledger, reads) = list_key(false);
        assert_eq!(key.open_router(), None, "never read to find out");
        ledger.record(KeyName::OpenRouter, KeyState::Missing);
        assert_eq!(key.open_router(), None);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        ledger.record(KeyName::OpenRouter, KeyState::Saved);
        for _ in 0..3 {
            assert_eq!(
                key.open_router().map(|secret| secret.expose().to_owned()),
                Some("sk-or-list".to_owned())
            );
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        key.forget();
        assert!(key.open_router().is_some());
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        let (locked, ledger, reads) = list_key(true);
        ledger.record(KeyName::OpenRouter, KeyState::Saved);
        assert_eq!(locked.open_router(), None, "a locked Keychain sends no key");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(format!("{locked:?}"), "ProviderListKey");
    }

    #[test]
    fn a_locked_keychain_is_unreadable_not_missing() {
        let ledger = KeyLedger::default();
        let keys = states(&Locked, &ledger, true);
        assert!(keys.iter().all(|key| key.state == KeyState::Unreadable));
        assert_eq!(
            named(&Locked, &keys, &name("anthropic"), false),
            KeyState::Unreadable
        );
        assert_eq!(
            named(&Locked, &keys, &name("my-proxy"), true),
            KeyState::Unreadable,
            "a hand-written secret name is read directly"
        );
        assert_eq!(
            named(&Locked, &keys, &name("my-proxy"), false),
            KeyState::NotChecked
        );
    }
}
