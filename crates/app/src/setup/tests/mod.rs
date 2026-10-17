//! The first-run flows end to end, over a temporary config folder and an in-memory secret
//! store: choices round-trip through `settings.toml`, and a saved key never reaches a file or
//! the status the UI sees.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use boardai_core::setup::{
    FileState, HarnessFound, HarnessName, InputDevice, KeyName, KeyState, KeyStatus, MicChoice,
    MicPermission, OllamaStatus, SetupBlocker, SetupEdit, SetupStatus, TalkerProvider,
    ThinkingEffort, VoiceEngine,
};
use boardai_runtime::llm::catalog::ModelCatalog;
use boardai_runtime::models::ModelStore;
use boardai_runtime::settings::{MemorySecretStore, SecretName, SecretStore, Settings};

use super::harnesses::Installed;
use super::keys::KeyLedger;
use super::status::{Inputs, Probes, Seen, gather};
use super::{
    Downloads, Known, SettingsGate, SetupPaths, SetupState, add_code_folder, begin_finishing,
    end_finishing, mark_complete, remove_key, save_edit, save_key,
};

/// Nothing seen yet, before a session runs.
static NO_KEYS_SEEN: std::sync::LazyLock<KeyLedger> = std::sync::LazyLock::new(KeyLedger::default);

fn known<'a>(ollama: &'a OllamaStatus, catalog: &'a ModelCatalog) -> Known<'a> {
    Known {
        ollama,
        catalog,
        devices: &[],
        keys: &NO_KEYS_SEEN,
        session_runs: false,
    }
}

const PLANTED_KEY: &str = "sk-ant-api03-PLANTEDsetupKEYplantedSETUPkey-0123456789abcdef";
const PLANTED_ELEVENLABS: &str = "sk_PLANTEDelevenLABSkeyPLANTED0123456789";

/// A folder under the system temp folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "boardai-setup-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch folder");
        Self(path)
    }

    fn paths(&self) -> SetupPaths {
        SetupPaths {
            settings: self.0.join("config/settings.toml"),
            home: self.0.join("home"),
            models: self.0.join("models"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn probes() -> Probes {
    Probes {
        ollama: OllamaStatus::Unreachable,
        microphone: MicPermission::Granted,
        harnesses: Installed {
            claude_code: HarnessFound::Found { version: None },
            pi: HarnessFound::Missing,
        },
        inputs: Inputs::default(),
        started: false,
        read_talker_key: false,
        mic_attributed_to: None,
    }
}

fn status(paths: &SetupPaths, secrets: &MemorySecretStore) -> SetupStatus {
    status_with(paths, secrets, &KeyLedger::default(), probes())
}

fn status_with(
    paths: &SetupPaths,
    secrets: &MemorySecretStore,
    ledger: &KeyLedger,
    probes: Probes,
) -> SetupStatus {
    let models = ModelStore::new(paths.models.clone()).expect("model store");
    let seen = Seen {
        downloads: &Downloads::default(),
        keys: ledger,
    };
    gather(paths, secrets, &models, seen, probes)
}

fn running() -> Probes {
    Probes {
        started: true,
        ..probes()
    }
}

fn saved(status: &SetupStatus) -> Vec<KeyName> {
    status
        .keys
        .iter()
        .filter(|key| key.state == KeyState::Saved)
        .map(|key| key.name)
        .collect()
}

/// Every file under `root`, recursively (the scratch tree is small).
fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder)
            .expect("readable folder")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_first_run_starts_from_the_defaults_and_names_what_is_missing() {
    let scratch = Scratch::new("first");
    let paths = scratch.paths();
    let secrets = MemorySecretStore::new();
    let first = status(&paths, &secrets);
    assert!(!first.complete);
    assert_eq!(first.settings_problem, None);
    assert_eq!(first.talker.provider, TalkerProvider::Claude);
    assert_eq!(first.talker.key, KeyState::Missing);
    assert_eq!(first.voice, VoiceEngine::Kokoro);
    assert_eq!(first.voice_files.len(), 3);
    assert_eq!(first.hearing_files.len(), 2);
    assert!(
        first
            .hearing_files
            .iter()
            .all(|file| file.state == FileState::Missing && file.size_bytes > 0)
    );
    assert_eq!(
        first.blockers,
        vec![
            SetupBlocker::TalkerKey {
                provider: TalkerProvider::Claude
            },
            SetupBlocker::HearingNotReady,
        ]
    );
    assert!(
        !paths.settings.exists(),
        "reading the status writes nothing"
    );
}

#[test]
fn choices_round_trip_through_settings_and_finishing_sets_the_flag() {
    let scratch = Scratch::new("round-trip");
    let paths = scratch.paths();
    let secrets = MemorySecretStore::new();
    let lock = Mutex::new(SettingsGate::default());
    let catalog = ModelCatalog::default();
    let ollama = OllamaStatus::Reachable {
        models: vec!["gemma4:latest".to_owned()],
    };
    let edits = [
        SetupEdit::Talker {
            provider: TalkerProvider::Ollama,
            model: None,
        },
        SetupEdit::Mic {
            mode: MicChoice::PushToTalk,
        },
        SetupEdit::Input {
            device: InputDevice::BuiltIn,
        },
    ];
    for edit in &edits {
        save_edit(&paths, &lock, edit, &secrets, known(&ollama, &catalog)).expect("saved");
    }
    let saved = status(&paths, &secrets);
    assert_eq!(saved.talker.provider, TalkerProvider::Ollama);
    assert_eq!(saved.talker.model, "gemma4:latest");
    assert_eq!(saved.talker.key, KeyState::NotNeeded);
    assert_eq!(saved.mic_mode, MicChoice::PushToTalk);
    assert_eq!(saved.input, InputDevice::BuiltIn);
    assert!(!saved.complete);

    mark_complete(&paths, &lock).expect("flag set");
    let after = status_with(&paths, &secrets, &KeyLedger::default(), running());
    assert!(after.complete);
    assert_eq!(
        after.talker.key,
        KeyState::NotNeeded,
        "Ollama has no key to check"
    );
    let settings = Settings::load(&paths.settings, &paths.home).expect("valid file");
    assert!(settings.setup_complete);
    assert_eq!(settings.talker.model, "gemma4:latest");
    assert_eq!(settings.scribe.model, "gemma4:latest");
    assert!(status(&paths, &secrets).complete);
}

#[test]
fn a_saved_key_reaches_the_store_and_no_file_or_status() {
    let scratch = Scratch::new("no-secret");
    let paths = scratch.paths();
    let secrets = MemorySecretStore::new();
    let lock = Mutex::new(SettingsGate::default());
    let catalog = ModelCatalog::default();
    let unreachable = OllamaStatus::Unreachable;

    let ledger = KeyLedger::default();
    save_key(
        &secrets,
        &ledger,
        &lock,
        KeyName::Anthropic,
        format!("{PLANTED_KEY}\n"),
    )
    .expect("saved");
    save_key(
        &secrets,
        &ledger,
        &lock,
        KeyName::ElevenLabs,
        PLANTED_ELEVENLABS.to_owned(),
    )
    .expect("saved");
    let claude = SetupEdit::Talker {
        provider: TalkerProvider::Claude,
        model: None,
    };
    let elevenlabs = SetupEdit::Voice {
        engine: VoiceEngine::ElevenLabs,
    };
    for edit in [&claude, &elevenlabs] {
        save_edit(&paths, &lock, edit, &secrets, known(&unreachable, &catalog)).expect("saved");
    }
    mark_complete(&paths, &lock).expect("flag set");

    let unread = status_with(&paths, &secrets, &KeyLedger::default(), running());
    assert_eq!(
        unread.talker.key,
        KeyState::NotChecked,
        "a running session skips the Keychain"
    );
    assert!(
        unread
            .keys
            .iter()
            .all(|key| key.state == KeyState::NotChecked)
    );
    let remembered = status_with(&paths, &secrets, &ledger, running());
    assert_eq!(
        remembered.talker.key,
        KeyState::Saved,
        "what this run saved is known without a read"
    );
    assert_eq!(
        saved(&remembered),
        [KeyName::Anthropic, KeyName::ElevenLabs]
    );
    let seen = status(&paths, &secrets);
    assert_eq!(seen.talker.key, KeyState::Saved);
    assert_eq!(saved(&seen), [KeyName::Anthropic, KeyName::ElevenLabs]);
    assert_eq!(seen.voice, VoiceEngine::ElevenLabs);
    let shown = format!("{seen:?}");
    for planted in [PLANTED_KEY, PLANTED_ELEVENLABS] {
        assert!(!shown.contains(planted), "the UI never sees a key");
    }

    assert_no_key_on_disk(&scratch.0, &paths.settings);
}

/// No file under `root` (settings included) holds a planted key.
fn assert_no_key_on_disk(root: &Path, settings: &Path) {
    let files = files_under(root);
    assert!(files.iter().any(|file| file == settings));
    for file in files {
        let bytes = std::fs::read(&file).expect("readable file");
        let text = String::from_utf8_lossy(&bytes);
        for planted in [PLANTED_KEY, PLANTED_ELEVENLABS] {
            assert!(
                !text.contains(&planted[..24]),
                "a key reached {}",
                file.display()
            );
        }
    }
}

#[test]
fn a_bad_key_or_an_unusable_file_changes_nothing() {
    let scratch = Scratch::new("refused");
    let paths = scratch.paths();
    let secrets = MemorySecretStore::new();
    let lock = Mutex::new(SettingsGate::default());
    let oversized = "k".repeat(boardai_runtime::limits::SECRET_VALUE_MAX_BYTES + 1);
    let ledger = KeyLedger::default();
    assert!(save_key(&secrets, &ledger, &lock, KeyName::Anthropic, oversized).is_err());
    assert_eq!(saved(&status(&paths, &secrets)), []);
    assert_eq!(
        ledger.seen(KeyName::Anthropic),
        KeyState::NotChecked,
        "a refused key records nothing"
    );

    std::fs::create_dir_all(paths.settings.parent().expect("folder")).expect("config folder");
    let hand_written = "mic = \"sometimes\"\n# my notes\n";
    std::fs::write(&paths.settings, hand_written).expect("written");
    let refused = save_edit(
        &paths,
        &lock,
        &SetupEdit::Mic {
            mode: MicChoice::Open,
        },
        &secrets,
        known(&OllamaStatus::Unreachable, &ModelCatalog::default()),
    );
    assert!(refused.is_err_and(|reason| reason.contains("`mic`")));
    assert!(mark_complete(&paths, &lock).is_err());
    assert_eq!(
        std::fs::read_to_string(&paths.settings).expect("still there"),
        hand_written
    );
    let shown = status(&paths, &secrets);
    assert!(shown.settings_problem.is_some());
    assert_eq!(shown.blockers.first(), Some(&SetupBlocker::SettingsProblem));
}

#[test]
fn saving_while_the_session_runs_asks_for_one_restart() {
    let state = SetupState::new();
    assert!(state.claim_start());
    assert!(!state.claim_start(), "the session starts once");
    let lock = Mutex::new(SettingsGate::default());
    assert_eq!(begin_finishing(&lock), Ok(false), "nothing saved yet");
    end_finishing(&lock, false);
    lock.lock().expect("lock").changed = true;
    lock.lock().expect("lock").changed = true;
    assert_eq!(begin_finishing(&lock), Ok(true));
    end_finishing(&lock, false);
    assert_eq!(
        begin_finishing(&lock),
        Ok(false),
        "one restart per round of changes"
    );
    end_finishing(&lock, true);
    assert_eq!(
        begin_finishing(&lock),
        Ok(true),
        "a refused finish keeps its change for the next"
    );
}

/// A Keychain that refuses everything (locked, or a denied prompt).
struct Locked;

impl SecretStore for Locked {
    fn get(
        &self,
        name: &SecretName,
    ) -> Result<boardai_runtime::settings::Secret, boardai_runtime::settings::SecretError> {
        Err(boardai_runtime::settings::SecretError::Unavailable {
            name: name.clone(),
            detail: "locked".to_owned(),
        })
    }

    fn set(
        &self,
        name: &SecretName,
        _secret: &boardai_runtime::settings::Secret,
    ) -> Result<(), boardai_runtime::settings::SecretError> {
        Err(boardai_runtime::settings::SecretError::Unwritable {
            name: name.clone(),
            detail: "locked".to_owned(),
        })
    }

    fn remove(&self, name: &SecretName) -> Result<(), boardai_runtime::settings::SecretError> {
        Err(boardai_runtime::settings::SecretError::Unwritable {
            name: name.clone(),
            detail: "locked".to_owned(),
        })
    }
}

#[test]
fn the_settings_page_sees_the_talker_scribe_folders_and_vault_as_configured() {
    let scratch = Scratch::new("settings-page");
    let paths = scratch.paths();
    std::fs::create_dir_all(paths.settings.parent().expect("folder")).expect("config folder");
    std::fs::write(
        &paths.settings,
        "setup_complete = true\ncode_roots = [\"~/code\", \"/opt/src\"]\n\
         [talker]\nkind = \"openai_compatible\"\nmodel = \"qwen/qwen3.8-27b\"\neffort = \"none\"\n\
         base_url = \"https://openrouter.ai/api/v1\"\nsecret = \"openrouter\"\nroute = [\"cerebras\"]\n\
         [scribe]\nkind = \"openai_compatible\"\nmodel = \"qwen/qwen3.8-27b\"\neffort = \"medium\"\n\
         base_url = \"https://openrouter.ai/api/v1\"\nsecret = \"openrouter\"\n\
         [harness]\nkind = \"pi\"\n",
    )
    .expect("written");
    let secrets = MemorySecretStore::new();
    let shown = status_with(&paths, &secrets, &KeyLedger::default(), running());
    assert_eq!(shown.talker.provider, TalkerProvider::OpenRouter);
    assert_eq!(shown.talker.thinking, ThinkingEffort::Off);
    assert_eq!(shown.talker.route, ["cerebras"]);
    assert!(shown.talker.preset);
    assert_eq!(shown.scribe_model, "qwen/qwen3.8-27b");
    assert_eq!(shown.code_folders, ["~/code", "/opt/src"]);
    assert_eq!(shown.vault, "~/BoardAI");
    assert_eq!(
        (
            shown.elevenlabs.model.as_str(),
            shown.elevenlabs.voice.as_str()
        ),
        ("Flash", "Rachel"),
        "the voice stays Rachel until the owner chooses (D-137)"
    );
    assert_eq!(
        shown.harnesses.default, None,
        "settings pick pi, which isn't installed"
    );
    assert_eq!(shown.talker.key, KeyState::NotChecked);
    assert!(
        !shown
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, SetupBlocker::TalkerKey { .. })),
        "a refresh doesn't read the Keychain; finishing reads it (tests/keychain.rs)"
    );
    let claude = status(&scratch.paths(), &secrets);
    assert_eq!(
        claude.harnesses.claude_code,
        HarnessFound::Found { version: None }
    );
    let defaults = Scratch::new("settings-page-defaults");
    let fresh = status(&defaults.paths(), &secrets);
    assert_eq!(
        fresh.talker.thinking,
        ThinkingEffort::Low,
        "Claude never turns it off"
    );
    assert!(!fresh.talker.preset);
    assert_eq!(fresh.harnesses.default, Some(HarnessName::ClaudeCode));
}

#[test]
fn removing_a_key_forgets_it_and_takes_elevenlabs_off_the_voice() {
    let scratch = Scratch::new("remove-key");
    let paths = scratch.paths();
    let secrets = MemorySecretStore::new();
    let lock = Mutex::new(SettingsGate::default());
    let ledger = KeyLedger::default();
    let catalog = ModelCatalog::default();
    let unreachable = OllamaStatus::Unreachable;
    save_key(
        &secrets,
        &ledger,
        &lock,
        KeyName::ElevenLabs,
        PLANTED_ELEVENLABS.to_owned(),
    )
    .expect("ok");
    let elevenlabs = SetupEdit::Voice {
        engine: VoiceEngine::ElevenLabs,
    };
    save_edit(
        &paths,
        &lock,
        &elevenlabs,
        &secrets,
        known(&unreachable, &catalog),
    )
    .expect("ok");
    remove_key(&paths, &lock, &secrets, &ledger, KeyName::ElevenLabs).expect("removed");
    let after = status_with(&paths, &secrets, &ledger, running());
    assert_eq!(
        after.voice,
        VoiceEngine::Kokoro,
        "the next session still speaks"
    );
    assert!(after.keys.contains(&KeyStatus {
        name: KeyName::ElevenLabs,
        state: KeyState::Missing,
    }));
    // Removing a key that isn't there is done already; the talker's key goes the same way.
    remove_key(&paths, &lock, &secrets, &ledger, KeyName::Anthropic).expect("nothing to remove");
    let claude = status_with(&paths, &secrets, &ledger, running());
    assert_eq!(claude.talker.key, KeyState::Missing);
    assert_eq!(
        claude.blockers.first(),
        Some(&SetupBlocker::TalkerKey {
            provider: TalkerProvider::Claude
        }),
        "going back to talking waits for a key"
    );
}

#[test]
fn a_locked_keychain_refuses_a_key_in_plain_words_and_changes_nothing() {
    let scratch = Scratch::new("locked");
    let paths = scratch.paths();
    let lock = Mutex::new(SettingsGate::default());
    let ledger = KeyLedger::default();
    let refused = save_key(
        &Locked,
        &ledger,
        &Mutex::new(SettingsGate::default()),
        KeyName::OpenRouter,
        "sk-or-v1-abc".to_owned(),
    );
    assert!(refused.is_err_and(|words| words.contains("Unlock") && !words.contains("sk-or")));
    let refused = remove_key(&paths, &lock, &Locked, &ledger, KeyName::OpenRouter);
    assert!(refused.is_err_and(|words| words.contains("Unlock")));
    assert_eq!(ledger.seen(KeyName::OpenRouter), KeyState::NotChecked);
    assert!(!paths.settings.exists(), "nothing was written");
}

#[test]
fn a_chosen_code_folder_is_added_once_as_written() {
    let scratch = Scratch::new("folders");
    let paths = scratch.paths();
    let lock = Mutex::new(SettingsGate::default());
    let folder = scratch.0.join("home/code/boardai");
    std::fs::create_dir_all(&folder).expect("a folder");
    add_code_folder(&paths, &lock, &folder).expect("added");
    let shown = status(&paths, &MemorySecretStore::new());
    assert_eq!(shown.code_folders, ["~/code/boardai"]);
    assert!(
        add_code_folder(&paths, &lock, &folder).is_err(),
        "already listed"
    );
    let remove = SetupEdit::RemoveCodeFolder { index: 0 };
    let catalog = ModelCatalog::default();
    let secrets = MemorySecretStore::new();
    save_edit(
        &paths,
        &lock,
        &remove,
        &secrets,
        known(&OllamaStatus::Unreachable, &catalog),
    )
    .expect("removed");
    assert!(status(&paths, &secrets).code_folders.is_empty());
}

#[test]
fn a_list_that_does_not_arrive_or_an_unfinished_rebuild_is_said_in_plain_words() {
    use boardai_core::setup::ModelQuery;
    use boardai_runtime::llm::ProviderError;
    use boardai_runtime::llm::catalog::CatalogError;

    use super::commands::{index_rebuild_answer, model_list_problem};

    let network = CatalogError::Provider(ProviderError::Network {
        detail: "timed out".to_owned(),
    });
    assert!(model_list_problem(&ModelQuery::OpenRouter, &network).contains("didn't arrive"));
    assert!(model_list_problem(&ModelQuery::Ollama, &network).contains("Ollama isn't answering"));
    assert!(
        index_rebuild_answer(None).is_err_and(|words| words.contains("Still rebuilding")),
        "a rebuild past its wait is not reported as done"
    );
    let failed = index_rebuild_answer(Some(Err(boardai_runtime::store::StoreError::Stopped)));
    assert!(failed.is_err_and(|words| words.contains("old one is kept")));
}

mod finish;
mod keychain;
