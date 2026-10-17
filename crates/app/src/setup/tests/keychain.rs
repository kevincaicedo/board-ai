//! When setup reads a key's value from the Keychain: the read that can prompt on an unsigned
//! build (D-102). A save that has nothing to do with a key never reads one, and while a session
//! runs the ledger answers instead (review 2, finding 3).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use boardai_core::setup::{
    KeyName, KeyState, MicChoice, OllamaStatus, SetupBlocker, SetupEdit, VoiceEngine,
};
use boardai_runtime::llm::catalog::ModelCatalog;
use boardai_runtime::settings::{MemorySecretStore, Secret, SecretError, SecretName, SecretStore};

use super::{PLANTED_ELEVENLABS, PLANTED_KEY, Scratch, known, probes, running};
use crate::setup::keys::KeyLedger;
use crate::setup::status::{Probes, Seen, gather};
use crate::setup::{
    Downloads, FINISHING_REFUSAL, Known, SettingsGate, begin_finishing, end_finishing, save_edit,
    save_key,
};

/// A Keychain that counts reads of a value (`get`).
struct CountingSecrets {
    inner: MemorySecretStore,
    reads: AtomicU32,
}

impl CountingSecrets {
    fn new() -> Self {
        Self {
            inner: MemorySecretStore::new(),
            reads: AtomicU32::new(0),
        }
    }

    fn reads(&self) -> u32 {
        self.reads.load(Ordering::SeqCst)
    }

    fn with(name: &str, value: &str) -> Self {
        let mut inner = MemorySecretStore::new();
        let name = SecretName::parse(name).unwrap_or_else(|| panic!("{name} is a name"));
        inner.insert(name, Secret::new(value.to_owned()));
        Self {
            inner,
            reads: AtomicU32::new(0),
        }
    }
}

impl SecretStore for CountingSecrets {
    fn get(&self, name: &SecretName) -> Result<Secret, SecretError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(name)
    }

    fn set(&self, name: &SecretName, secret: &Secret) -> Result<(), SecretError> {
        self.inner.set(name, secret)
    }

    fn remove(&self, name: &SecretName) -> Result<(), SecretError> {
        self.inner.remove(name)
    }
}

#[test]
fn a_save_unrelated_to_a_key_never_reads_the_keychain() {
    let scratch = Scratch::new("keychain-reads");
    let paths = scratch.paths();
    let secrets = CountingSecrets::new();
    let lock = Mutex::new(SettingsGate::default());
    let catalog = ModelCatalog::default();
    let ollama = OllamaStatus::Unreachable;
    let edit = SetupEdit::Mic {
        mode: MicChoice::PushToTalk,
    };
    save_edit(&paths, &lock, &edit, &secrets, known(&ollama, &catalog)).expect("saved");
    assert_eq!(
        secrets.reads(),
        0,
        "picking the mic mode read a key's value from the Keychain"
    );
}

#[test]
fn elevenlabs_as_the_voice_takes_the_key_this_run_saw_while_a_session_runs() {
    let scratch = Scratch::new("keychain-elevenlabs");
    let paths = scratch.paths();
    let secrets = CountingSecrets::with("elevenlabs", PLANTED_ELEVENLABS);
    let lock = Mutex::new(SettingsGate::default());
    let catalog = ModelCatalog::default();
    let ollama = OllamaStatus::Unreachable;
    let elevenlabs = SetupEdit::Voice {
        engine: VoiceEngine::ElevenLabs,
    };
    let ledger = KeyLedger::default();
    let before = Known {
        keys: &ledger,
        ..known(&ollama, &catalog)
    };
    save_edit(&paths, &lock, &elevenlabs, &secrets, before).expect("saved");
    assert_eq!(
        secrets.reads(),
        1,
        "before a session runs the choice reads its key"
    );
    let during = Known {
        session_runs: true,
        ..before
    };
    save_edit(&paths, &lock, &elevenlabs, &secrets, during).expect("saved");
    assert_eq!(
        secrets.reads(),
        1,
        "while one runs, what this run saw answers"
    );
    ledger.record(KeyName::ElevenLabs, KeyState::Unreadable);
    let locked = save_edit(&paths, &lock, &elevenlabs, &secrets, during);
    assert!(
        locked.is_err_and(|words| words.contains("Unlock")),
        "a Keychain that didn't answer is said as such, not as a missing key"
    );
    assert_eq!(secrets.reads(), 1);
}

fn finishing_status(
    secrets: &dyn SecretStore,
    paths: &crate::setup::SetupPaths,
    ledger: &KeyLedger,
) -> boardai_core::setup::SetupStatus {
    let models = boardai_runtime::models::ModelStore::new(paths.models.clone()).expect("models");
    let seen = Seen {
        downloads: &Downloads::default(),
        keys: ledger,
    };
    let probes = Probes {
        read_talker_key: true,
        ..running()
    };
    gather(paths, secrets, &models, seen, probes)
}

/// Review 2, finding 2: while a session runs a key this run never read is `NotChecked`, which
/// the start rule lets through; finishing reads the talker's key first, so a missing one keeps
/// the page open.
#[test]
fn finishing_reads_the_talker_key_this_run_never_saw() {
    let scratch = Scratch::new("keychain-finish");
    let paths = scratch.paths();
    let ledger = KeyLedger::default();
    let empty = CountingSecrets::new();
    let refreshed = super::status_with(&paths, &MemorySecretStore::new(), &ledger, running());
    assert_eq!(
        refreshed.talker.key,
        KeyState::NotChecked,
        "a refresh never reads"
    );
    let missing = finishing_status(&empty, &paths, &ledger);
    assert_eq!(missing.talker.key, KeyState::Missing);
    assert!(
        missing
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, SetupBlocker::TalkerKey { .. })),
        "{:?}",
        missing.blockers
    );
    assert_eq!(empty.reads(), 1);
    let saved = CountingSecrets::with("anthropic", PLANTED_KEY);
    let fresh = KeyLedger::default();
    let found = finishing_status(&saved, &paths, &fresh);
    assert_eq!(found.talker.key, KeyState::Saved);
    assert_eq!(saved.reads(), 1);
    let again = finishing_status(&saved, &paths, &fresh);
    assert_eq!(again.talker.key, KeyState::Saved);
    assert_eq!(saved.reads(), 1, "a key this run saw isn't read again");
    let unused = probes();
    assert!(!unused.read_talker_key);
}

/// Review 2, finding 2: from the start rule's check until the new session has read its
/// settings, a change is refused and writes nothing; afterwards it lands and counts.
#[test]
fn a_change_while_finishing_is_refused_and_lands_after() {
    let scratch = Scratch::new("keychain-gate");
    let paths = scratch.paths();
    let secrets = CountingSecrets::new();
    let lock = Mutex::new(SettingsGate::default());
    let catalog = ModelCatalog::default();
    let ollama = OllamaStatus::Unreachable;
    let mode = SetupEdit::Mic {
        mode: MicChoice::PushToTalk,
    };
    assert_eq!(begin_finishing(&lock), Ok(false));
    assert!(begin_finishing(&lock).is_err(), "one finish at a time");
    let refused = save_edit(&paths, &lock, &mode, &secrets, known(&ollama, &catalog));
    assert_eq!(refused, Err(FINISHING_REFUSAL.to_owned()));
    let key = save_key(
        &secrets,
        &KeyLedger::default(),
        &lock,
        KeyName::Anthropic,
        PLANTED_KEY.to_owned(),
    );
    assert_eq!(key, Err(FINISHING_REFUSAL.to_owned()));
    assert!(!paths.settings.exists(), "nothing was written");
    end_finishing(&lock, false);
    save_edit(&paths, &lock, &mode, &secrets, known(&ollama, &catalog)).expect("saved");
    assert_eq!(
        begin_finishing(&lock),
        Ok(true),
        "the change asks for a restart"
    );
}
