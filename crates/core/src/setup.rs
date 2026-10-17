//! The first-run screen's wire types (LLD §3.8, canvas artboard 11 · First run) and the rule for
//! when a session may start.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessesFound {
    pub claude_code: bool,
    pub pi: bool,
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

/// Everything the first-run screen shows. Built by the app; `blockers` comes from
/// [`blockers`] over the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    /// Setup was finished (the flag in `settings.toml`), or the session already runs: the UI
    /// opens the stage instead of this screen.
    pub complete: bool,
    /// Why `settings.toml` can't be used; it names a field and never quotes a value.
    pub settings_problem: Option<String>,
    pub talker: TalkerSetup,
    /// Keys with a value in the Keychain, by name only.
    pub keys: Vec<KeyName>,
    pub voice: VoiceEngine,
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
    pub harnesses: HarnessesFound,
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
mod tests {
    use super::{
        FileState, HarnessesFound, InputDevice, KeyName, KeyState, MicChoice, MicPermission,
        ModelFileStatus, ModelInfo, ModelListing, ModelPrice, ModelQuery, ModelReasoning,
        OllamaStatus, SetupBlocker, SetupEdit, SetupStatus, TalkerProvider, TalkerSetup,
        VoiceEngine, blockers,
    };

    fn file(id: &str, state: FileState) -> ModelFileStatus {
        ModelFileStatus {
            id: id.to_owned(),
            state,
            have_bytes: 0,
            size_bytes: 10,
        }
    }

    fn ready() -> SetupStatus {
        SetupStatus {
            complete: false,
            settings_problem: None,
            talker: TalkerSetup {
                provider: TalkerProvider::Claude,
                model: "claude-opus-5-5".to_owned(),
                key: KeyState::Saved,
                ollama: OllamaStatus::Unreachable,
            },
            keys: Vec::new(),
            voice: VoiceEngine::Kokoro,
            voice_files: vec![file("kokoro", FileState::Missing)],
            hearing_model: "base.en".to_owned(),
            hearing_files: vec![
                file("whisper-base.en", FileState::Ready),
                file("silero-vad", FileState::Ready),
            ],
            microphone: MicPermission::Granted,
            mic_mode: MicChoice::Open,
            input: InputDevice::SystemDefault,
            input_devices: Vec::new(),
            harnesses: HarnessesFound {
                claude_code: true,
                pi: false,
            },
            blockers: Vec::new(),
            mic_attributed_to: None,
        }
    }

    #[test]
    fn a_saved_key_ready_hearing_and_the_mic_start_even_while_the_voice_downloads() {
        assert_eq!(blockers(&ready()), Vec::new());
    }

    #[test]
    fn every_missing_piece_is_named_in_order() {
        let mut status = ready();
        status.settings_problem = Some("settings.toml `mic`: must be …".to_owned());
        status.talker.key = KeyState::Missing;
        status.hearing_files[0].state = FileState::Downloading;
        status.microphone = MicPermission::Denied;
        assert_eq!(
            blockers(&status),
            vec![
                SetupBlocker::SettingsProblem,
                SetupBlocker::TalkerKey {
                    provider: TalkerProvider::Claude
                },
                SetupBlocker::HearingNotReady,
                SetupBlocker::MicrophoneDenied,
            ]
        );
        status.microphone = MicPermission::Undetermined;
        assert!(blockers(&status).contains(&SetupBlocker::MicrophoneNotAllowed));
    }

    #[test]
    fn push_to_talk_needs_the_hearing_files_too() {
        let mut status = ready();
        status.mic_mode = MicChoice::PushToTalk;
        status.hearing_files[1].state = FileState::Partial;
        assert_eq!(blockers(&status), vec![SetupBlocker::HearingNotReady]);
    }

    #[test]
    fn ollama_needs_to_answer_with_the_configured_model() {
        let mut status = ready();
        status.talker = TalkerSetup {
            provider: TalkerProvider::Ollama,
            model: "gemma4:latest".to_owned(),
            key: KeyState::NotNeeded,
            ollama: OllamaStatus::Unreachable,
        };
        assert_eq!(blockers(&status), vec![SetupBlocker::OllamaUnreachable]);
        status.talker.ollama = OllamaStatus::Reachable {
            models: vec!["qwen3.5:2b".to_owned()],
        };
        assert_eq!(
            blockers(&status),
            vec![SetupBlocker::OllamaModelMissing {
                model: "gemma4:latest".to_owned()
            }]
        );
        status.talker.ollama = OllamaStatus::Reachable {
            models: vec!["qwen3.5:2b".to_owned(), "gemma4:latest".to_owned()],
        };
        assert_eq!(blockers(&status), Vec::new());
    }

    #[test]
    fn an_unknown_whisper_model_is_named() {
        let mut status = ready();
        status.hearing_model = "large".to_owned();
        status.hearing_files.truncate(1);
        assert_eq!(
            blockers(&status),
            vec![SetupBlocker::UnknownHearingModel {
                model: "large".to_owned()
            }]
        );
    }

    #[test]
    fn edits_from_the_ui_are_parsed_and_unknown_ones_refused() {
        let edit: Result<SetupEdit, _> = serde_json::from_str(
            r#"{"kind":"Talker","provider":"Ollama","model":"gemma4:latest"}"#,
        );
        assert!(matches!(
            edit,
            Ok(SetupEdit::Talker {
                provider: TalkerProvider::Ollama,
                ..
            })
        ));
        for bad in [
            r#"{"kind":"Talker","provider":"Gemini","model":null}"#,
            r#"{"kind":"Delete"}"#,
            r#"{"kind":"Mic","mode":"Always"}"#,
            r#"{"kind":"Input","device":{"kind":"Named"}}"#,
        ] {
            assert!(serde_json::from_str::<SetupEdit>(bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<KeyName>(r#""GitHub""#).is_err());
    }

    #[test]
    fn model_queries_from_the_ui_are_parsed_and_unknown_ones_refused() {
        let providers: Result<ModelQuery, _> =
            serde_json::from_str(r#"{"kind":"Providers","model":"qwen/qwen3.8-27b"}"#);
        assert!(matches!(providers, Ok(ModelQuery::Providers { .. })));
        assert_eq!(
            serde_json::from_str::<ModelQuery>(r#"{"kind":"OpenRouter"}"#).ok(),
            Some(ModelQuery::OpenRouter)
        );
        for bad in [
            r#"{"kind":"Anthropic"}"#,
            r#"{"kind":"Providers"}"#,
            r#"{"kind":"Providers","model":7}"#,
        ] {
            assert!(serde_json::from_str::<ModelQuery>(bad).is_err(), "{bad}");
        }
        let listing = ModelListing::OpenRouter {
            models: vec![ModelInfo {
                id: "openai/gpt-oss-120b".to_owned(),
                name: "OpenAI: gpt-oss-120b".to_owned(),
                context_tokens: Some(131_072),
                price: Some(ModelPrice {
                    input_micro_usd_per_million_tokens: 150_000,
                    output_micro_usd_per_million_tokens: 600_000,
                }),
                reasoning: ModelReasoning::Always {
                    least_effort: "low".to_owned(),
                },
                preset: true,
            }],
            left_out: 0,
        };
        let wire = serde_json::to_value(&listing).unwrap_or_default();
        assert_eq!(wire["kind"], "OpenRouter");
        assert_eq!(wire["models"][0]["reasoning"]["kind"], "Always");
        assert_eq!(
            wire["models"][0]["price"]["input_micro_usd_per_million_tokens"],
            150_000
        );
    }

    #[test]
    fn an_unreadable_keychain_blocks_like_a_missing_key() {
        let mut status = ready();
        status.talker.key = KeyState::Unreadable;
        assert_eq!(
            blockers(&status),
            vec![SetupBlocker::TalkerKey {
                provider: TalkerProvider::Claude
            }]
        );
    }
}
