//! Which keys the Keychain holds, by name only, as this run has seen them (D-102). Before a
//! session runs the screen reads the Keychain on every status; once one runs it doesn't (each
//! read of an unsigned build's key can prompt), so the settings page shows what this run last
//! read, saved or removed, and `NotChecked` for a key it never touched until the owner asks to
//! check. No value is ever kept here: only `Saved`, `Missing` or `Unreadable` per name.

use std::collections::BTreeMap;
use std::sync::Mutex;

use boardai_core::setup::{KeyName, KeyState, KeyStatus};
use boardai_runtime::settings::{SecretName, SecretStore};

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

/// Asks the Keychain whether `name` holds a value (it may prompt; blocks) and records it.
pub fn read(secrets: &dyn SecretStore, ledger: &KeyLedger, name: KeyName) -> KeyState {
    let Some(secret) = secret_name(name) else {
        return KeyState::Missing;
    };
    let state = presence(secrets, &secret);
    ledger.record(name, state);
    state
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

#[cfg(test)]
mod tests {
    use boardai_core::setup::{KeyName, KeyState};
    use boardai_runtime::settings::{
        MemorySecretStore, Secret, SecretError, SecretName, SecretStore,
    };

    use super::{KeyLedger, named, states};

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
