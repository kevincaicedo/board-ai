//! The first-run screen's and the settings page's wire types (LLD §3.8, canvas artboards 11 · First
//! run and 12 · Settings) and the rule for when a session may start.
//!
//! The app gathers a [`SetupStatus`] (settings, which secrets exist, model files, Ollama, the
//! harnesses, microphone permission) and the UI shows it; the UI answers with [`SetupEdit`]s, a
//! key, a download request or "start talking". [`blockers`] is the one place that decides
//! whether the session can start: the app refuses to finish setup while it returns anything, and
//! the UI disables "Start talking" on the same list. No secret value ever appears here: the UI
//! only learns that a key with a given name is saved.
//!
//! These types are exported to TypeScript with the rest of the protocol (D-015).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Who does the talking (card 1). The scribe follows the talker's provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TalkerProvider {
    /// Anthropic's API, with the `anthropic` key.
    Claude,
    /// OpenRouter's OpenAI-compatible API, with the `openrouter` key.
    OpenRouter,
    /// Ollama on this Mac; no key, nothing leaves the machine.
    Ollama,
    /// `settings.toml` names another provider (a custom base URL); the screen shows it and
    /// leaves it alone unless a pill is picked.
    Other,
}

/// The keys the first-run screen can save, each stored in the Keychain under its own name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KeyName {
    Anthropic,
    OpenRouter,
    ElevenLabs,
}

impl KeyName {
    /// The Keychain account (and `secret = "…"` in settings) the key is stored under.
    #[must_use]
    pub const fn secret_name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
            Self::ElevenLabs => "elevenlabs",
        }
    }
}

/// Whether the talker's key is in the Keychain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KeyState {
    /// The provider needs no key (Ollama).
    NotNeeded,
    Saved,
    Missing,
    /// The Keychain could not be read (locked, or a prompt was denied).
    Unreadable,
    /// The session already runs, so the Keychain wasn't read for the screen (each read of an
    /// unsigned build's key can prompt).
    NotChecked,
}

/// Ollama at its local address, asked for its models with a short timeout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state")]
#[ts(export)]
pub enum OllamaStatus {
    /// Nothing answered (not installed, or not running).
    Unreachable,
    Reachable {
        models: Vec<String>,
    },
}

/// Card 1: the configured talker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TalkerSetup {
    pub provider: TalkerProvider,
    pub model: String,
    /// The talker's key, when its provider needs one.
    pub key: KeyState,
    pub ollama: OllamaStatus,
    /// How much it thinks before it answers, as configured.
    pub thinking: ThinkingEffort,
    /// OpenRouter only: the providers tried, in order, and no others (route slugs, `cerebras`;
    /// D-152). Empty: OpenRouter picks.
    pub route: Vec<String>,
    /// A BoardAI preset (OpenRouter's two, D-152, or Ollama's default model): its voice and its
    /// drawing were checked. Other models may draw less well.
    pub preset: bool,
}

/// The talker's thinking as configured (`effort`). Claude can't turn it off, so its `none` is
/// `Low` here, as it is on the wire (LLD §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ThinkingEffort {
    /// Off for speech.
    Off,
    Low,
    Medium,
    High,
    ExtraHigh,
    Max,
}

/// One key's state, by name only: a value never leaves the Keychain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KeyStatus {
    pub name: KeyName,
    /// `Saved`, `Missing` or `Unreadable` as the Keychain answered this run; `NotChecked` while a
    /// session runs and nothing was read or saved for it since the app started (D-102).
    pub state: KeyState,
}

/// ElevenLabs as it would speak, by display name: "Flash" and "Rachel" until the owner chooses
/// another voice (D-137); otherwise the id `settings.toml` names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ElevenLabsVoice {
    pub model: String,
    pub voice: String,
}

/// Who speaks (card 2). The system voice needs nothing and covers speech while Kokoro
/// downloads, so the voice never blocks a start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceEngine {
    Kokoro,
    ElevenLabs,
    System,
}

/// A model file on disk, with its size known before downloading (LLD §3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum FileState {
    Missing,
    /// A download stopped part way; the next one resumes it.
    Partial,
    /// A download is running now.
    Downloading,
    /// Downloaded and SHA-256 verified.
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelFileStatus {
    /// The model id (`whisper-base.en`), as `just models` and the download command take it.
    pub id: String,
    pub state: FileState,
    #[ts(type = "number")]
    pub have_bytes: u64,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

/// Microphone permission for this app (macOS asks the user once).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MicPermission {
    /// Not asked yet: asking shows the system prompt.
    Undetermined,
    Denied,
    Granted,
}

/// How you speak to it (card 3, D-011).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MicChoice {
    /// Just talk; echo cancellation keeps BoardAI from hearing itself.
    Open,
    /// Hold Space while you speak.
    PushToTalk,
}

/// Which microphone BoardAI listens on (`[audio] input` in settings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum InputDevice {
    SystemDefault,
    /// The Mac's own microphone, for when the default input is a virtual device.
    BuiltIn,
    /// A device by the name macOS shows.
    Named {
        name: String,
    },
}

/// The coding harnesses found on this Mac (LLD §3.5). Paths stay in the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessesFound {
    pub claude_code: HarnessFound,
    pub pi: HarnessFound,
    /// The one a task runs on: `[harness] kind` (`auto` is Claude Code first), when installed.
    pub default: Option<HarnessName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state")]
#[ts(export)]
pub enum HarnessFound {
    Missing,
    /// Installed; `version` is what `--version` printed, when it answered in time.
    Found {
        version: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum HarnessName {
    ClaudeCode,
    Pi,
}

/// Why the session can't start yet, in the order the screen asks for things.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum SetupBlocker {
    /// `settings.toml` can't be read or parsed; nothing is saved until it is fixed.
    SettingsProblem,
    /// The talker's key is not in the Keychain (or the Keychain can't be read).
    TalkerKey {
        provider: TalkerProvider,
    },
    OllamaUnreachable,
    /// Ollama answers but does not have the configured model.
    OllamaModelMissing {
        model: String,
    },
    /// `stt.model` names a Whisper model BoardAI has no file for.
    UnknownHearingModel {
        model: String,
    },
    /// The speech-to-text files (Whisper and the voice detector) are not all verified yet.
    HearingNotReady,
    MicrophoneNotAllowed,
    MicrophoneDenied,
}

/// Everything the first-run screen and the settings page show. Built by the app; `blockers`
/// comes from [`blockers`] over the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    /// Setup was finished (the flag in `settings.toml`), or the session already runs: the UI
    /// opens the stage instead of this screen.
    pub complete: bool,
    /// Why `settings.toml` can't be used; it names a field and never quotes a value.
    pub settings_problem: Option<String>,
    pub talker: TalkerSetup,
    /// The scribe's model (memory digests); it runs on the talker's provider.
    pub scribe_model: String,
    /// Every key the screen saves, in [`KeyName`] order, with its state.
    pub keys: Vec<KeyStatus>,
    pub voice: VoiceEngine,
    pub elevenlabs: ElevenLabsVoice,
    /// Kokoro's files (model, voice, lexicon), whichever engine is chosen.
    pub voice_files: Vec<ModelFileStatus>,
    /// `stt.model`, for example `base.en`.
    pub hearing_model: String,
    /// Whisper's file for `stt.model` and the voice detector's.
    pub hearing_files: Vec<ModelFileStatus>,
    pub microphone: MicPermission,
    pub mic_mode: MicChoice,
    pub input: InputDevice,
    /// Input devices macOS lists by name; empty until the audio layer can list them.
    pub input_devices: Vec<String>,
    /// The input macOS uses by default now, by name ("System default · now Shure MV7+").
    pub default_input: Option<String>,
    /// The Mac's own microphone by name, which [`InputDevice::BuiltIn`] picks.
    pub built_in_input: Option<String>,
    pub harnesses: HarnessesFound,
    /// Where "open the boardai repo" looks (`code_roots`), as written: `~/…` or absolute.
    pub code_folders: Vec<String>,
    /// The vault (`~/BoardAI`), as written.
    pub vault: String,
    pub blockers: Vec<SetupBlocker>,
    /// The app macOS asks about the microphone instead of BoardAI, because BoardAI was started
    /// from it (a terminal or an editor, "Visual Studio Code"); `None` when macOS asks about
    /// BoardAI itself, or it can't be told (D-149).
    pub mic_attributed_to: Option<String>,
}

/// One change from the screen, saved to `settings.toml` at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum SetupEdit {
    /// Picks a provider for the talker and the scribe. `model` is the talker's model: an Ollama
    /// model (`None` keeps the current one or picks a found one), or an OpenRouter model id from
    /// [`ModelListing::OpenRouter`] (`None` is the default preset). Claude uses its defaults.
    Talker {
        provider: TalkerProvider,
        model: Option<String>,
    },
    Voice {
        engine: VoiceEngine,
    },
    Mic {
        mode: MicChoice,
    },
    Input {
        device: InputDevice,
    },
    /// The scribe's model, on the talker's provider: an OpenRouter model id from the list, or an
    /// Ollama model. Claude's scribe stays Claude Haiku (D-017).
    Scribe {
        model: String,
    },
    /// Removes the code folder at `index` in [`SetupStatus::code_folders`]. Folders are added
    /// only through the system's folder chooser (`setup_add_code_folder`), so no path ever
    /// comes from the UI.
    RemoveCodeFolder {
        index: u32,
    },
}

/// What "Rebuild the index" did: the index emptied and filled again from one scan of the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IndexRebuilt {
    #[ts(type = "number")]
    pub files_seen: u64,
    #[ts(type = "number")]
    pub reindexed: u64,
    /// Units left out because a file couldn't be read; the next scan tries them again.
    #[ts(type = "number")]
    pub skipped: u64,
    /// The scan stopped at its bound; files past it are not indexed.
    pub truncated: bool,
}

/// Why a download stopped, for the voice card to say in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum DownloadProblem {
    /// No connection, or the server went silent; the partial file is kept.
    Network,
    /// The server answered with an error status.
    Server { status: u16 },
    /// The server sent more bytes than the file has, or resumed from the wrong place.
    Unexpected,
    /// The file failed its SHA-256 check and was deleted.
    Corrupt,
    /// The file could not be written.
    Disk,
}

/// Progress of one model download, on the channel the download command was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum DownloadEvent {
    Progress {
        #[ts(type = "number")]
        received_bytes: u64,
        #[ts(type = "number")]
        total_bytes: u64,
    },
    /// Verified and in place.
    Done,
    Failed {
        problem: DownloadProblem,
    },
}

/// What the model picker asks for (`setup_models`, D-153). Lists are bounded and cached by the
/// app for a while, so asking again is cheap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum ModelQuery {
    /// OpenRouter's models that can call tools (BoardAI's talker needs them).
    OpenRouter,
    /// The models the local Ollama has.
    Ollama,
    /// Who serves one OpenRouter model: its providers, their limits and prices.
    Providers { model: String },
}

/// The answer to a [`ModelQuery`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum ModelListing {
    /// BoardAI's presets first, then the rest in OpenRouter's order. `left_out` counts models
    /// past the list's bound, or with an id or name too long to keep.
    OpenRouter {
        models: Vec<ModelInfo>,
        left_out: u32,
    },
    Ollama {
        models: Vec<ModelInfo>,
    },
    Providers {
        model: String,
        providers: Vec<ModelProvider>,
        left_out: u32,
    },
}

/// One model in a picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelInfo {
    /// What settings store and the provider is asked for (`qwen/qwen3.8-27b`, `gemma4:latest`).
    pub id: String,
    /// The provider's display name (`Qwen: Qwen3.8 27B`); the id where there is none.
    pub name: String,
    pub context_tokens: Option<u32>,
    /// `None` for local models, or when the provider lists no fixed price.
    pub price: Option<ModelPrice>,
    pub reasoning: ModelReasoning,
    /// A BoardAI preset: measured with its voice and drawing, with a provider route and
    /// thinking set for speed (D-152). Other models may draw less well or answer more slowly.
    pub preset: bool,
    /// A preset's route (slugs, `cerebras`): the only providers tried. Empty: OpenRouter picks.
    pub route: Vec<String>,
}

/// Prices in millionths of a US dollar per million tokens (`240000` is $0.24 per million).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelPrice {
    #[ts(type = "number")]
    pub input_micro_usd_per_million_tokens: u64,
    #[ts(type = "number")]
    pub output_micro_usd_per_million_tokens: u64,
}

/// Whether a model thinks before it answers, which costs the voice time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum ModelReasoning {
    /// The list doesn't say (Ollama's list never does).
    Unknown,
    /// The model takes no reasoning setting.
    Never,
    /// Thinking can be turned off; BoardAI turns it off for the voice.
    Optional,
    /// The model always thinks; BoardAI asks for the least it allows (`low`).
    Always { least_effort: String },
}

/// One provider serving an OpenRouter model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelProvider {
    /// As OpenRouter shows it (`Cerebras`).
    pub name: String,
    /// What a route names it by (`cerebras/fp16`).
    pub slug: String,
    pub context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub price: Option<ModelPrice>,
    /// `fp16`, `fp8`, `int4`…, when listed.
    pub quantization: Option<String>,
    /// The provider takes tool calls for this model.
    pub tools: bool,
    /// Time to the first token, median over OpenRouter's last 30 minutes, when its list gives
    /// it. Its public list (no key) gives none (D-153, recorded 2026-10).
    pub latency_ms_p50: Option<u32>,
    /// Output speed, median over OpenRouter's last 30 minutes, when its list gives it.
    pub throughput_tokens_per_second_p50: Option<u32>,
}

/// What stands between the screen and a session (D-101): a usable talker, the hearing files,
/// and the microphone. The voice never blocks: the system voice speaks until Kokoro is ready.
/// Push-to-talk needs the same files as an open mic, since both turn speech into text with
/// Whisper; only how a turn starts and ends differs.
#[must_use]
pub fn blockers(status: &SetupStatus) -> Vec<SetupBlocker> {
    let mut found = Vec::new();
    if status.settings_problem.is_some() {
        found.push(SetupBlocker::SettingsProblem);
    }
    let talker = &status.talker;
    match talker.key {
        KeyState::Missing | KeyState::Unreadable => found.push(SetupBlocker::TalkerKey {
            provider: talker.provider,
        }),
        KeyState::NotNeeded | KeyState::Saved | KeyState::NotChecked => {}
    }
    if talker.provider == TalkerProvider::Ollama {
        match &talker.ollama {
            OllamaStatus::Unreachable => found.push(SetupBlocker::OllamaUnreachable),
            OllamaStatus::Reachable { models } => {
                if !models.contains(&talker.model) {
                    found.push(SetupBlocker::OllamaModelMissing {
                        model: talker.model.clone(),
                    });
                }
            }
        }
    }
    if status.hearing_files.len() < HEARING_FILES {
        found.push(SetupBlocker::UnknownHearingModel {
            model: status.hearing_model.clone(),
        });
    } else if status
        .hearing_files
        .iter()
        .any(|file| file.state != FileState::Ready)
    {
        found.push(SetupBlocker::HearingNotReady);
    }
    match status.microphone {
        MicPermission::Granted => {}
        MicPermission::Undetermined => found.push(SetupBlocker::MicrophoneNotAllowed),
        MicPermission::Denied => found.push(SetupBlocker::MicrophoneDenied),
    }
    found
}

/// Whisper's model and the voice detector.
const HEARING_FILES: usize = 2;

#[cfg(test)]
mod tests;
