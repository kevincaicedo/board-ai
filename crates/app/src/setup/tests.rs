//! The first-run flows end to end, over a temporary config folder and an in-memory secret
//! store: choices round-trip through `settings.toml`, and a saved key never reaches a file or
//! the status the UI sees.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use boardai_core::setup::{
    FileState, HarnessesFound, InputDevice, KeyName, KeyState, MicChoice, MicPermission,
    OllamaStatus, SetupBlocker, SetupEdit, SetupStatus, TalkerProvider, VoiceEngine,
};
use boardai_runtime::llm::catalog::ModelCatalog;
use boardai_runtime::models::ModelStore;
use boardai_runtime::settings::{MemorySecretStore, Settings};

use super::status::{Probes, gather};
use super::{Downloads, Known, SetupPaths, SetupState, mark_complete, save_edit, save_key};

fn known<'a>(ollama: &'a OllamaStatus, catalog: &'a ModelCatalog) -> Known<'a> {
    Known {
        ollama,
        catalog,
        devices: &[],
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
        harnesses: HarnessesFound {
            claude_code: true,
            pi: false,
        },
        input_devices: Vec::new(),
        started: false,
        mic_attributed_to: None,
    }
}

fn status(paths: &SetupPaths, secrets: &MemorySecretStore) -> SetupStatus {
    let models = ModelStore::new(paths.models.clone()).expect("model store");
    gather(paths, secrets, &models, &Downloads::default(), probes())
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
    let lock = Mutex::new(());
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
    let models = ModelStore::new(paths.models.clone()).expect("model store");
    let running = Probes {
        started: true,
        ..probes()
    };
    let after = gather(&paths, &secrets, &models, &Downloads::default(), running);
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
    let lock = Mutex::new(());
    let catalog = ModelCatalog::default();
    let unreachable = OllamaStatus::Unreachable;

    save_key(&secrets, KeyName::Anthropic, format!("{PLANTED_KEY}\n")).expect("key saved");
    save_key(&secrets, KeyName::ElevenLabs, PLANTED_ELEVENLABS.to_owned()).expect("key saved");
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

    let models = ModelStore::new(paths.models.clone()).expect("model store");
    let running = Probes {
        started: true,
        ..probes()
    };
    let unread = gather(&paths, &secrets, &models, &Downloads::default(), running);
    assert_eq!(
        unread.talker.key,
        KeyState::NotChecked,
        "a running session skips the Keychain"
    );
    assert!(unread.keys.is_empty());
    let seen = status(&paths, &secrets);
    assert_eq!(seen.talker.key, KeyState::Saved);
    assert_eq!(seen.keys, vec![KeyName::Anthropic, KeyName::ElevenLabs]);
    assert_eq!(seen.voice, VoiceEngine::ElevenLabs);
    let shown = format!("{seen:?}");
    for planted in [PLANTED_KEY, PLANTED_ELEVENLABS] {
        assert!(!shown.contains(planted), "the UI never sees a key");
    }

    let files = files_under(&scratch.0);
    assert!(files.iter().any(|file| file == &paths.settings));
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
    let lock = Mutex::new(());
    let oversized = "k".repeat(boardai_runtime::limits::SECRET_VALUE_MAX_BYTES + 1);
    assert!(save_key(&secrets, KeyName::Anthropic, oversized).is_err());
    assert_eq!(status(&paths, &secrets).keys, Vec::new());

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
    state.mark_changed();
    assert!(
        !state.take_changed(),
        "before the session starts, saving is setup itself"
    );
    assert!(state.claim_start());
    assert!(!state.claim_start(), "the session starts once");
    state.mark_changed();
    state.mark_changed();
    assert!(state.take_changed());
    assert!(!state.take_changed(), "one restart per round of changes");
}
