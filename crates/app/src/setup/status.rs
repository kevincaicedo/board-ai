//! Building the first-run screen's and the settings page's [`SetupStatus`] from what is on this
//! Mac: `settings.toml`, which keys the Keychain holds (names only), the model files, and the
//! probes the command took (Ollama, microphone permission and inputs, harnesses). Reading the
//! Keychain can block, so this runs on the blocking pool.

use std::path::{Path, PathBuf};

use boardai_core::setup::{
    ElevenLabsVoice, FileState, InputDevice, KeyState, MicChoice, MicPermission, ModelFileStatus,
    OllamaStatus, SetupStatus, TalkerProvider, TalkerSetup, ThinkingEffort, VoiceEngine, blockers,
};
use boardai_runtime::audio::InputChoice;
use boardai_runtime::audio::devices::DeviceSnapshot;
use boardai_runtime::llm::Effort;
use boardai_runtime::llm::base_url::same_endpoint;
use boardai_runtime::llm::ollama::OLLAMA_DEFAULT_MODEL;
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use boardai_runtime::llm::openrouter;
use boardai_runtime::models::{ModelId, ModelStatus, ModelStore};
use boardai_runtime::settings::{
    DEFAULT_VOICE, LlmKind, LlmSettings, MicMode, SecretStore, Settings, TtsKind,
};
use boardai_runtime::speech_out::{
    ELEVENLABS_DEFAULT_MODEL, ELEVENLABS_DEFAULT_MODEL_NAME, ELEVENLABS_DEFAULT_VOICE_NAME,
    kokoro_files,
};

use super::downloads::Downloads;
use super::folders::as_written;
use super::harnesses::{self, Installed};
use super::keys::{self, KeyLedger};

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
    pub harnesses: Installed,
    pub inputs: Inputs,
    /// The session already runs (setup finished, or a dev script skipped it).
    pub started: bool,
    /// The start rule is being checked (finishing): the talker's key is read from the Keychain
    /// even while a session runs, when this run never saw it.
    pub read_talker_key: bool,
    /// The app macOS asks about the microphone instead of BoardAI (`permission::attributed_to`).
    pub mic_attributed_to: Option<String>,
}

/// The microphones macOS has now, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub names: Vec<String>,
    /// The system's default input.
    pub default: Option<String>,
    /// The Mac's own microphone.
    pub built_in: Option<String>,
}

impl Inputs {
    pub fn from_snapshot(snapshot: &DeviceSnapshot) -> Self {
        let inputs: Vec<_> = snapshot
            .devices
            .iter()
            .filter(|device| device.has_input)
            .collect();
        Self {
            names: inputs.iter().map(|device| device.name.clone()).collect(),
            default: snapshot.default_input.and_then(|id| {
                inputs
                    .iter()
                    .find(|device| device.id == id)
                    .map(|device| device.name.clone())
            }),
            built_in: inputs
                .iter()
                .find(|device| device.built_in)
                .map(|device| device.name.clone()),
        }
    }
}

/// Where setup reads and writes, and what the run has learned (keys, harness versions).
#[derive(Clone, Copy)]
pub struct Seen<'a> {
    pub downloads: &'a Downloads,
    pub keys: &'a KeyLedger,
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
    seen: Seen<'_>,
    probes: Probes,
) -> SetupStatus {
    let (settings, settings_problem) = load(paths);
    // Once the session runs, the Keychain is read only when the owner asks (D-102), or for the
    // start rule: a talker key this run never saw must not pass it as `NotChecked`.
    let read_keys = !probes.started;
    let read_talker_key = read_keys || probes.read_talker_key;
    if let Some(listed) = settings.talker.secret.as_ref().and_then(keys::listed)
        && probes.read_talker_key
        && seen.keys.seen(listed) == KeyState::NotChecked
    {
        keys::read(secrets, seen.keys, listed);
    }
    let keys = keys::states(secrets, seen.keys, read_keys);
    let hearing: Vec<ModelId> = ModelId::whisper(&settings.stt.model)
        .into_iter()
        .chain([ModelId::SileroVad])
        .collect();
    let files = |ids: &[ModelId]| -> Vec<ModelFileStatus> {
        ids.iter()
            .map(|id| file_status(models, seen.downloads, *id))
            .collect()
    };
    let talker_key = settings
        .talker
        .secret
        .as_ref()
        .map_or(KeyState::NotNeeded, |name| {
            keys::named(secrets, &keys, name, read_talker_key)
        });
    let mut status = SetupStatus {
        complete: settings.setup_complete || probes.started,
        settings_problem,
        talker: talker(&settings.talker, talker_key, probes.ollama),
        scribe_model: settings.scribe.model.clone(),
        keys,
        voice: match settings.tts.kind {
            TtsKind::Kokoro => VoiceEngine::Kokoro,
            TtsKind::ElevenLabs => VoiceEngine::ElevenLabs,
            TtsKind::System => VoiceEngine::System,
        },
        elevenlabs: elevenlabs(&settings),
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
        input_devices: probes.inputs.names,
        default_input: probes.inputs.default,
        built_in_input: probes.inputs.built_in,
        harnesses: harnesses::found(probes.harnesses, settings.harness.kind),
        code_folders: settings
            .code_roots
            .iter()
            .map(|root| as_written(root, &paths.home))
            .collect(),
        vault: as_written(&settings.vault, &paths.home),
        blockers: Vec::new(),
        mic_attributed_to: probes.mic_attributed_to,
    };
    status.blockers = blockers(&status);
    status
}

fn talker(llm: &LlmSettings, key: KeyState, ollama: OllamaStatus) -> TalkerSetup {
    let provider = provider(llm);
    let preset = match provider {
        TalkerProvider::OpenRouter => openrouter::preset(&llm.model).is_some(),
        TalkerProvider::Ollama => llm.model == OLLAMA_DEFAULT_MODEL,
        TalkerProvider::Claude | TalkerProvider::Other => false,
    };
    TalkerSetup {
        provider,
        model: llm.model.clone(),
        key,
        ollama,
        thinking: thinking(llm),
        route: llm
            .route
            .iter()
            .map(|slug| slug.as_str().to_owned())
            .collect(),
        preset,
    }
}

/// The effort as the model gets it: Claude can't turn thinking off, so its `none` is `low`.
fn thinking(llm: &LlmSettings) -> ThinkingEffort {
    match (llm.effort, llm.kind) {
        (Effort::None, LlmKind::OpenAiCompatible) => ThinkingEffort::Off,
        (Effort::None | Effort::Low, LlmKind::Anthropic)
        | (Effort::Low, LlmKind::OpenAiCompatible) => ThinkingEffort::Low,
        (Effort::Medium, _) => ThinkingEffort::Medium,
        (Effort::High, _) => ThinkingEffort::High,
        (Effort::ExtraHigh, _) => ThinkingEffort::ExtraHigh,
        (Effort::Max, _) => ThinkingEffort::Max,
    }
}

/// ElevenLabs by display name: its default model and voice by name (D-137), else as written.
fn elevenlabs(settings: &Settings) -> ElevenLabsVoice {
    let chosen = settings.tts.kind == TtsKind::ElevenLabs;
    let model = match settings.tts.model.as_deref() {
        Some(model) if chosen && model != ELEVENLABS_DEFAULT_MODEL => model.to_owned(),
        Some(_) | None => ELEVENLABS_DEFAULT_MODEL_NAME.to_owned(),
    };
    let voice = if chosen && settings.voice.id != DEFAULT_VOICE {
        settings.voice.id.clone()
    } else {
        ELEVENLABS_DEFAULT_VOICE_NAME.to_owned()
    };
    ElevenLabsVoice { model, voice }
}

/// Which pill the configured talker is, by the endpoint its base URL names (a trailing slash
/// or `127.0.0.1` for `localhost` is the same endpoint). A base URL the screen doesn't offer is
/// `Other`.
pub fn provider(talker: &LlmSettings) -> TalkerProvider {
    match (talker.kind, talker.base_url.as_deref()) {
        (LlmKind::Anthropic, None) => TalkerProvider::Claude,
        (LlmKind::OpenAiCompatible, None) => TalkerProvider::Ollama,
        (LlmKind::OpenAiCompatible, Some(url)) if same_endpoint(url, OLLAMA_BASE_URL) => {
            TalkerProvider::Ollama
        }
        (LlmKind::OpenAiCompatible, Some(url)) if same_endpoint(url, OPENROUTER_BASE_URL) => {
            TalkerProvider::OpenRouter
        }
        (LlmKind::Anthropic | LlmKind::OpenAiCompatible, Some(_)) => TalkerProvider::Other,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use boardai_core::setup::TalkerProvider;
    use boardai_runtime::settings::{LlmKind, Settings};

    use super::provider;

    /// Review 2, finding 10: the pill matched the base URL as an exact string, so
    /// `https://openrouter.ai/api/v1/` showed as Other.
    #[test]
    fn the_provider_pill_reads_the_url_not_its_spelling() {
        let mut talker = Settings::defaults(Path::new("/Users/me")).talker;
        talker.kind = LlmKind::OpenAiCompatible;
        for (url, pill) in [
            ("https://openrouter.ai/api/v1", TalkerProvider::OpenRouter),
            ("https://openrouter.ai/api/v1/", TalkerProvider::OpenRouter),
            ("https://OpenRouter.ai/api/v1", TalkerProvider::OpenRouter),
            ("http://localhost:11434/v1", TalkerProvider::Ollama),
            ("http://localhost:11434/v1/", TalkerProvider::Ollama),
            ("http://127.0.0.1:11434/v1", TalkerProvider::Ollama),
            ("https://openrouter.ai/api/v2", TalkerProvider::Other),
            ("https://example.com/v1", TalkerProvider::Other),
            ("http://localhost:1234/v1", TalkerProvider::Other),
        ] {
            talker.base_url = Some(url.to_owned());
            assert_eq!(provider(&talker), pill, "{url}");
        }
    }
}
