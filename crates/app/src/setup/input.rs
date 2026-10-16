//! What the first-run screen sends, parsed once where it enters (AGENTS.md: parse, don't
//! validate): a pasted key into a `Secret`, a model id into a `ModelId`, an edit into a
//! settings change. Refusals are plain words for the card they belong to; none quotes a key.

use boardai_core::setup::{
    InputDevice, KeyName, MicChoice, OllamaStatus, SetupEdit, TalkerProvider, VoiceEngine,
};
use boardai_runtime::audio::InputChoice;
use boardai_runtime::limits::{OLLAMA_MODEL_NAME_MAX_BYTES, SECRET_VALUE_MAX_BYTES};
use boardai_runtime::llm::Effort;
use boardai_runtime::llm::openai::OPENROUTER_BASE_URL;
use boardai_runtime::models::ModelId;
use boardai_runtime::settings::{
    LlmChoice, LlmKind, MicMode, Secret, SecretName, Settings, SettingsEdit, TtsKind,
};

/// The local model the screen picks when Ollama has it and nothing else is chosen (the
/// `converse` default, D-103).
pub const DEFAULT_OLLAMA_MODEL: &str = "gemma4:latest";
/// OpenRouter's talker, as the design names it, and the matching scribe (D-017's pair on
/// OpenRouter; the owner's call, D-103).
pub const OPENROUTER_TALKER_MODEL: &str = "anthropic/claude-opus-5.5";
pub const OPENROUTER_SCRIBE_MODEL: &str = "anthropic/claude-haiku-5.5";

/// Why a pasted key was refused. The key itself is never kept or quoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRefused {
    Empty,
    TooLong,
    /// Spaces or line breaks inside: two things were pasted, or a sentence around the key.
    HasSpaces,
}

impl KeyRefused {
    pub const fn words(self) -> &'static str {
        match self {
            Self::Empty => "Paste a key first.",
            Self::TooLong => "That is longer than any API key. Paste just the key.",
            Self::HasSpaces => "That has spaces or line breaks in it. Paste just the key.",
        }
    }
}

/// A pasted key: surrounding whitespace (a copied line break) is dropped, and the rest must
/// be one word of at most `SECRET_VALUE_MAX_BYTES`.
pub fn parse_key(value: String) -> Result<Secret, KeyRefused> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KeyRefused::Empty);
    }
    if trimmed.len() > SECRET_VALUE_MAX_BYTES {
        return Err(KeyRefused::TooLong);
    }
    if trimmed
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(KeyRefused::HasSpaces);
    }
    Ok(Secret::new(trimmed.to_owned()))
}

/// The Keychain name a key is saved under.
pub fn secret_name(key: KeyName) -> Option<SecretName> {
    SecretName::parse(key.secret_name())
}

/// A model id from `ModelId::name` (`whisper-base.en`).
pub fn parse_model(id: &str) -> Result<ModelId, String> {
    ModelId::parse(id).ok_or_else(|| "BoardAI doesn't know that model.".to_owned())
}

/// What the app knows that an edit is checked against.
#[derive(Clone, Copy, Debug)]
pub struct EditContext<'a> {
    pub current: &'a Settings,
    /// Picks the Ollama model when the screen names none.
    pub ollama: &'a OllamaStatus,
    /// An ElevenLabs key is saved.
    pub elevenlabs_key: bool,
    /// Input devices macOS lists now; a microphone is chosen by name only from these.
    pub devices: &'a [String],
}

/// The settings change an edit makes.
pub fn settings_edit(edit: &SetupEdit, known: EditContext<'_>) -> Result<SettingsEdit, String> {
    let EditContext {
        current,
        ollama,
        elevenlabs_key,
        devices,
    } = known;
    match edit {
        SetupEdit::Talker { provider, model } => {
            talker(*provider, model.as_deref(), current, ollama)
        }
        SetupEdit::Voice { engine } => match engine {
            VoiceEngine::Kokoro => Ok(SettingsEdit::Tts(TtsKind::Kokoro)),
            VoiceEngine::System => Ok(SettingsEdit::Tts(TtsKind::System)),
            VoiceEngine::ElevenLabs if elevenlabs_key => Ok(SettingsEdit::Tts(TtsKind::ElevenLabs)),
            VoiceEngine::ElevenLabs => Err("Save an ElevenLabs key first.".to_owned()),
        },
        SetupEdit::Mic { mode } => Ok(SettingsEdit::Mic(match mode {
            MicChoice::Open => MicMode::Open,
            MicChoice::PushToTalk => MicMode::PushToTalk,
        })),
        SetupEdit::Input { device } => match device {
            InputDevice::SystemDefault => Ok(SettingsEdit::Input(InputChoice::SystemDefault)),
            InputDevice::BuiltIn => Ok(SettingsEdit::Input(InputChoice::BuiltIn)),
            InputDevice::Named { name } if devices.contains(name) => {
                Ok(SettingsEdit::Input(InputChoice::Named(name.clone())))
            }
            InputDevice::Named { .. } => {
                Err("That microphone isn't connected. Plug it in and pick it again.".to_owned())
            }
        },
    }
}

fn talker(
    provider: TalkerProvider,
    model: Option<&str>,
    current: &Settings,
    ollama: &OllamaStatus,
) -> Result<SettingsEdit, String> {
    let (talker, scribe) = match provider {
        TalkerProvider::Claude => {
            let claude = LlmChoice {
                kind: LlmKind::Anthropic,
                model: None,
                effort: None,
                base_url: None,
                secret: secret_name(KeyName::Anthropic),
            };
            (claude.clone(), claude)
        }
        TalkerProvider::OpenRouter => {
            let open_router = |model: &str| LlmChoice {
                kind: LlmKind::OpenAiCompatible,
                model: Some(model.to_owned()),
                effort: None,
                base_url: Some(OPENROUTER_BASE_URL.to_owned()),
                secret: secret_name(KeyName::OpenRouter),
            };
            (
                open_router(OPENROUTER_TALKER_MODEL),
                open_router(OPENROUTER_SCRIBE_MODEL),
            )
        }
        TalkerProvider::Ollama => {
            let model = match model {
                Some(name) => ollama_model_name(name)?,
                None => pick_ollama_model(current, ollama),
            };
            // Local thinking models need effort `none` for a responsive voice (llm::Effort).
            let local = LlmChoice {
                kind: LlmKind::OpenAiCompatible,
                model: Some(model),
                effort: Some(Effort::None),
                base_url: None,
                secret: None,
            };
            (local.clone(), local)
        }
        TalkerProvider::Other => {
            return Err("Pick Claude, OpenRouter or Ollama.".to_owned());
        }
    };
    Ok(SettingsEdit::Llm { talker, scribe })
}

fn ollama_model_name(name: &str) -> Result<String, String> {
    let valid = !name.trim().is_empty()
        && name.len() <= OLLAMA_MODEL_NAME_MAX_BYTES
        && !name.chars().any(char::is_control);
    if valid {
        Ok(name.to_owned())
    } else {
        Err("That is not an Ollama model name.".to_owned())
    }
}

/// The talker's model when it is already local, else the default when Ollama has it, else the
/// first model Ollama lists, else the default (the screen then says to pull it).
fn pick_ollama_model(current: &Settings, ollama: &OllamaStatus) -> String {
    let listed: &[String] = match ollama {
        OllamaStatus::Reachable { models } => models,
        OllamaStatus::Unreachable => &[],
    };
    let local_now = current.talker.kind == LlmKind::OpenAiCompatible
        && current.talker.base_url.as_deref()
            == Some(boardai_runtime::llm::openai::OLLAMA_BASE_URL);
    if local_now && (listed.is_empty() || listed.contains(&current.talker.model)) {
        return current.talker.model.clone();
    }
    if listed.iter().any(|name| name == DEFAULT_OLLAMA_MODEL) {
        return DEFAULT_OLLAMA_MODEL.to_owned();
    }
    listed
        .first()
        .cloned()
        .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use boardai_core::setup::{
        InputDevice, KeyName, OllamaStatus, SetupEdit, TalkerProvider, VoiceEngine,
    };
    use boardai_runtime::audio::InputChoice;
    use boardai_runtime::limits::SECRET_VALUE_MAX_BYTES;
    use boardai_runtime::settings::{LlmKind, Settings, SettingsEdit};

    use super::{EditContext, KeyRefused, parse_key, parse_model, settings_edit};

    fn defaults() -> Settings {
        Settings::defaults(Path::new("/Users/someone"))
    }

    fn known<'a>(
        current: &'a Settings,
        ollama: &'a OllamaStatus,
        elevenlabs_key: bool,
        devices: &'a [String],
    ) -> EditContext<'a> {
        EditContext {
            current,
            ollama,
            elevenlabs_key,
            devices,
        }
    }

    #[test]
    fn a_pasted_key_is_trimmed_and_bounded() {
        let key = parse_key("  sk-ant-api03-abc\n".to_owned()).map(|secret| secret.expose().len());
        assert_eq!(key, Ok("sk-ant-api03-abc".len()));
        assert_eq!(parse_key(" \n".to_owned()).err(), Some(KeyRefused::Empty));
        let oversized = "k".repeat(SECRET_VALUE_MAX_BYTES + 1);
        assert_eq!(parse_key(oversized).err(), Some(KeyRefused::TooLong));
        let exactly = "k".repeat(SECRET_VALUE_MAX_BYTES);
        assert!(parse_key(exactly).is_ok());
        assert_eq!(
            parse_key("sk-ant one two".to_owned()).err(),
            Some(KeyRefused::HasSpaces)
        );
        assert_eq!(
            parse_key("sk-ant\u{0}x".to_owned()).err(),
            Some(KeyRefused::HasSpaces)
        );
    }

    #[test]
    fn refusals_never_quote_the_key() {
        for refused in [
            KeyRefused::Empty,
            KeyRefused::TooLong,
            KeyRefused::HasSpaces,
        ] {
            assert!(!refused.words().contains("sk-"));
        }
    }

    #[test]
    fn unknown_model_ids_are_refused() {
        assert!(parse_model("whisper-base.en").is_ok());
        assert!(parse_model("whisper-large").is_err());
        assert!(parse_model("../../etc/passwd").is_err());
        assert!(parse_model("").is_err());
    }

    #[test]
    fn a_provider_outside_the_screen_is_refused() {
        let unreachable = OllamaStatus::Unreachable;
        let other = SetupEdit::Talker {
            provider: TalkerProvider::Other,
            model: None,
        };
        // An unknown provider name never gets this far: `SetupEdit` fails to deserialize at
        // the command boundary (core::setup tests).
        assert!(settings_edit(&other, known(&defaults(), &unreachable, false, &[])).is_err());
    }

    #[test]
    fn ollama_picks_a_listed_model_and_turns_thinking_off() {
        let listed = OllamaStatus::Reachable {
            models: vec!["qwen3.5:2b".to_owned(), "gemma4:latest".to_owned()],
        };
        let pick = SetupEdit::Talker {
            provider: TalkerProvider::Ollama,
            model: None,
        };
        let Ok(SettingsEdit::Llm { talker, scribe }) =
            settings_edit(&pick, known(&defaults(), &listed, false, &[]))
        else {
            panic!("an Ollama talker");
        };
        assert_eq!(talker.kind, LlmKind::OpenAiCompatible);
        assert_eq!(talker.model.as_deref(), Some("gemma4:latest"));
        assert_eq!(scribe.model.as_deref(), Some("gemma4:latest"));
        assert_eq!(talker.secret, None);
        let named = SetupEdit::Talker {
            provider: TalkerProvider::Ollama,
            model: Some("x".repeat(500)),
        };
        assert!(settings_edit(&named, known(&defaults(), &listed, false, &[])).is_err());
    }

    #[test]
    fn elevenlabs_needs_its_key_and_a_named_microphone_must_be_connected() {
        let unreachable = OllamaStatus::Unreachable;
        let voice = SetupEdit::Voice {
            engine: VoiceEngine::ElevenLabs,
        };
        assert!(settings_edit(&voice, known(&defaults(), &unreachable, false, &[])).is_err());
        assert!(settings_edit(&voice, known(&defaults(), &unreachable, true, &[])).is_ok());
        let named = SetupEdit::Input {
            device: InputDevice::Named {
                name: "Studio Display Microphone".to_owned(),
            },
        };
        let current = defaults();
        assert!(settings_edit(&named, known(&current, &unreachable, false, &[])).is_err());
        let devices = [
            "MacBook Pro Microphone".to_owned(),
            "Studio Display Microphone".to_owned(),
        ];
        assert_eq!(
            settings_edit(&named, known(&current, &unreachable, false, &devices)),
            Ok(SettingsEdit::Input(InputChoice::Named(
                "Studio Display Microphone".to_owned()
            )))
        );
        assert_eq!(
            super::secret_name(KeyName::OpenRouter).map(|name| name.to_string()),
            Some("openrouter".to_owned())
        );
    }
}
