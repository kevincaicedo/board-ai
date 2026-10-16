//! Building the first-run screen's [`SetupStatus`] from what is on this Mac: `settings.toml`,
//! which keys the Keychain holds (names only), the model files, and the probes the command
//! took (Ollama, microphone permission, harnesses). Reading the Keychain can block, so this
//! runs on the blocking pool.

use std::path::{Path, PathBuf};

use boardai_core::setup::{
    FileState, HarnessesFound, InputDevice, KeyName, KeyState, MicChoice, MicPermission,
    ModelFileStatus, OllamaStatus, SetupStatus, TalkerProvider, TalkerSetup, VoiceEngine, blockers,
};
use boardai_runtime::audio::InputChoice;
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use boardai_runtime::models::{ModelId, ModelStatus, ModelStore};
use boardai_runtime::settings::{LlmKind, LlmSettings, MicMode, SecretStore, Settings, TtsKind};
use boardai_runtime::speech_out::kokoro_files;

use super::downloads::Downloads;
use super::input::secret_name;

/// Where setup reads and writes.
#[derive(Clone, Debug)]
pub struct SetupPaths {
    pub settings: PathBuf,
    pub home: PathBuf,
    pub models: PathBuf,
}

/// What the command probed before building the status.
#[derive(Clone, Debug)]
pub struct Probes {
    pub ollama: OllamaStatus,
    pub microphone: MicPermission,
    pub harnesses: HarnessesFound,
    pub input_devices: Vec<String>,
    /// The session already runs (setup finished, or a dev script skipped it).
    pub started: bool,
}

/// The settings as the app would start with them, or why it can't.
pub fn load(paths: &SetupPaths) -> (Settings, Option<String>) {
    match Settings::load(&paths.settings, &paths.home) {
        Ok(settings) => (settings, None),
        Err(error) => (Settings::defaults(&paths.home), Some(error.to_string())),
    }
}

pub fn gather(
    paths: &SetupPaths,
    secrets: &dyn SecretStore,
    models: &ModelStore,
    downloads: &Downloads,
    probes: Probes,
) -> SetupStatus {
    let (settings, settings_problem) = load(paths);
    // Once the session runs the screen isn't shown; the Keychain is left alone.
    let read_keys = !probes.started;
    let keys = [KeyName::Anthropic, KeyName::OpenRouter, KeyName::ElevenLabs]
        .into_iter()
        .filter(|key| {
            read_keys
                && secret_name(*key).is_some_and(|name| secrets.contains(&name).unwrap_or(false))
        })
        .collect();
    let hearing: Vec<ModelId> = ModelId::whisper(&settings.stt.model)
        .into_iter()
        .chain([ModelId::SileroVad])
        .collect();
    let files = |ids: &[ModelId]| -> Vec<ModelFileStatus> {
        ids.iter()
            .map(|id| file_status(models, downloads, *id))
            .collect()
    };
    let mut status = SetupStatus {
        complete: settings.setup_complete || probes.started,
        settings_problem,
        talker: TalkerSetup {
            provider: provider(&settings.talker),
            model: settings.talker.model.clone(),
            key: key_state(&settings.talker, secrets, read_keys),
            ollama: probes.ollama,
        },
        keys,
        voice: match settings.tts.kind {
            TtsKind::Kokoro => VoiceEngine::Kokoro,
            TtsKind::ElevenLabs => VoiceEngine::ElevenLabs,
            TtsKind::System => VoiceEngine::System,
        },
        voice_files: files(&kokoro_files(&settings.tts)),
        hearing_model: settings.stt.model.clone(),
        hearing_files: files(&hearing),
        microphone: probes.microphone,
        mic_mode: match settings.mic {
            MicMode::Open => MicChoice::Open,
            MicMode::PushToTalk => MicChoice::PushToTalk,
        },
        input: match &settings.audio.input {
            InputChoice::SystemDefault => InputDevice::SystemDefault,
            InputChoice::BuiltIn => InputDevice::BuiltIn,
            InputChoice::Named(name) => InputDevice::Named { name: name.clone() },
        },
        input_devices: probes.input_devices,
        harnesses: probes.harnesses,
        blockers: Vec::new(),
    };
    status.blockers = blockers(&status);
    status
}

/// Which pill the configured talker is. A base URL the screen doesn't offer is `Other`.
pub fn provider(talker: &LlmSettings) -> TalkerProvider {
    match (talker.kind, talker.base_url.as_deref()) {
        (LlmKind::Anthropic, None) => TalkerProvider::Claude,
        (LlmKind::OpenAiCompatible, None | Some(OLLAMA_BASE_URL)) => TalkerProvider::Ollama,
        (LlmKind::OpenAiCompatible, Some(OPENROUTER_BASE_URL)) => TalkerProvider::OpenRouter,
        (LlmKind::Anthropic | LlmKind::OpenAiCompatible, Some(_)) => TalkerProvider::Other,
    }
}

fn key_state(talker: &LlmSettings, secrets: &dyn SecretStore, read: bool) -> KeyState {
    let Some(name) = &talker.secret else {
        return KeyState::NotNeeded;
    };
    if !read {
        return KeyState::NotChecked;
    }
    match secrets.contains(name) {
        Ok(true) => KeyState::Saved,
        Ok(false) => KeyState::Missing,
        Err(error) => {
            tracing::warn!(%error, "the Keychain could not be read for setup");
            KeyState::Unreadable
        }
    }
}

fn file_status(models: &ModelStore, downloads: &Downloads, id: ModelId) -> ModelFileStatus {
    let running = downloads.is_running(id);
    let (state, have_bytes, size_bytes) = match models.status(&id.file()) {
        ModelStatus::Ready { .. } => (FileState::Ready, id.file().size_bytes, id.file().size_bytes),
        ModelStatus::Partial {
            have_bytes,
            size_bytes,
        } if running => (FileState::Downloading, have_bytes, size_bytes),
        ModelStatus::Partial {
            have_bytes,
            size_bytes,
        } => (FileState::Partial, have_bytes, size_bytes),
        ModelStatus::Missing { size_bytes } if running => (FileState::Downloading, 0, size_bytes),
        ModelStatus::Missing { size_bytes } => (FileState::Missing, 0, size_bytes),
    };
    ModelFileStatus {
        id: id.name().to_owned(),
        state,
        have_bytes,
        size_bytes,
    }
}

/// The models folder for a home folder (`just models` uses the same one).
pub fn models_root(home: &Path) -> PathBuf {
    ModelStore::default_root(home)
}
