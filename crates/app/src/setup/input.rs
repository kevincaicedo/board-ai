//! What the first-run screen sends, parsed once where it enters (AGENTS.md: parse, don't
//! validate): a pasted key into a `Secret`, a model id into a `ModelId`, an edit into a
//! settings change. Refusals are plain words for the card they belong to; none quotes a key.

use boardai_core::setup::{
    InputDevice, KeyName, MicChoice, ModelInfo, ModelReasoning, OllamaStatus, SetupEdit,
    TalkerProvider, VoiceEngine,
};
use boardai_runtime::audio::InputChoice;
use boardai_runtime::limits::{
    OLLAMA_MODEL_NAME_MAX_BYTES, OPENROUTER_TALKER_MAX_TOKENS, SECRET_VALUE_MAX_BYTES,
};
use boardai_runtime::llm::catalog::{OpenRouterModelId, scribe_effort, talker_effort};
use boardai_runtime::llm::ollama::OLLAMA_DEFAULT_MODEL;
use boardai_runtime::llm::openai::OPENROUTER_BASE_URL;
use boardai_runtime::llm::openrouter::{self, Preset, SCRIBE_PRESET, TALKER_PRESET};
use boardai_runtime::llm::{Effort, LlmRole};
use boardai_runtime::models::ModelId;
use boardai_runtime::settings::{
    LlmChoice, LlmKind, MicMode, Secret, SecretName, Settings, SettingsEdit, TtsKind,
};

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
    /// What the model picker last listed about the chosen OpenRouter model (its thinking);
    /// `None` when the picker never listed it.
    pub open_router_model: Option<&'a ModelInfo>,
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
        open_router_model,
        elevenlabs_key,
        devices,
    } = known;
    match edit {
        SetupEdit::Talker { provider, model } => {
            let listed = Listed {
                ollama,
                open_router_model,
            };
            talker(*provider, model.as_deref(), current, listed)
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
        SetupEdit::Scribe { model } => scribe(model, current, open_router_model),
        SetupEdit::RemoveCodeFolder { index } => usize::try_from(*index)
            .ok()
            .filter(|index| *index < current.code_roots.len())
            .map(SettingsEdit::RemoveCodeRoot)
            .ok_or_else(|| "That folder is no longer in the list.".to_owned()),
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

/// What the screen's lists said, for picking a model.
#[derive(Clone, Copy, Debug)]
struct Listed<'a> {
    ollama: &'a OllamaStatus,
    open_router_model: Option<&'a ModelInfo>,
}

fn talker(
    provider: TalkerProvider,
    model: Option<&str>,
    current: &Settings,
    listed: Listed<'_>,
) -> Result<SettingsEdit, String> {
    let (talker, scribe) = match provider {
        TalkerProvider::Claude => {
            let claude = LlmChoice {
                kind: LlmKind::Anthropic,
                model: None,
                effort: None,
                max_tokens: None,
                base_url: None,
                secret: secret_name(KeyName::Anthropic),
                route: Vec::new(),
            };
            (claude.clone(), claude)
        }
        TalkerProvider::OpenRouter => {
            let talker = match model {
                None => open_router_preset(TALKER_PRESET, LlmRole::Talker),
                Some(id) => open_router_model(id, listed.open_router_model)?,
            };
            let scribe = open_router_preset(SCRIBE_PRESET, LlmRole::Scribe);
            (talker, scribe)
        }
        TalkerProvider::Ollama => {
            let model = match model {
                Some(name) => ollama_model_name(name)?,
                None => pick_ollama_model(current, listed.ollama),
            };
            // Local thinking models need effort `none` for a responsive voice (llm::Effort).
            let local = LlmChoice {
                kind: LlmKind::OpenAiCompatible,
                model: Some(model),
                effort: Some(Effort::None),
                max_tokens: None,
                base_url: None,
                secret: None,
                route: Vec::new(),
            };
            (local.clone(), local)
        }
        TalkerProvider::Other => {
            return Err("Pick Claude, OpenRouter or Ollama.".to_owned());
        }
    };
    Ok(SettingsEdit::Llm { talker, scribe })
}

/// The scribe alone, on the talker's provider: an OpenRouter preset keeps its scribe settings,
/// another listed model thinks at the scribe's effort (`catalog::scribe_effort`) with no route;
/// an Ollama model runs without thinking, like the talker.
fn scribe(
    model: &str,
    current: &Settings,
    listed: Option<&ModelInfo>,
) -> Result<SettingsEdit, String> {
    let choice = match super::status::provider(&current.talker) {
        TalkerProvider::OpenRouter => {
            let id = OpenRouterModelId::parse(model)
                .ok_or_else(|| "That is not an OpenRouter model id.".to_owned())?;
            match openrouter::preset(id.as_str()) {
                Some(preset) => open_router_preset(preset, LlmRole::Scribe),
                None => {
                    let reasoning = listed
                        .filter(|listed| listed.id == id.as_str())
                        .map_or(&ModelReasoning::Unknown, |listed| &listed.reasoning);
                    LlmChoice {
                        kind: LlmKind::OpenAiCompatible,
                        model: Some(id.as_str().to_owned()),
                        effort: Some(scribe_effort(reasoning)),
                        max_tokens: None,
                        base_url: Some(OPENROUTER_BASE_URL.to_owned()),
                        secret: secret_name(KeyName::OpenRouter),
                        route: Vec::new(),
                    }
                }
            }
        }
        TalkerProvider::Ollama => LlmChoice {
            kind: LlmKind::OpenAiCompatible,
            model: Some(ollama_model_name(model)?),
            effort: Some(Effort::None),
            max_tokens: None,
            base_url: None,
            secret: None,
            route: Vec::new(),
        },
        TalkerProvider::Claude => {
            return Err("With Claude, memory digests use Claude Haiku.".to_owned());
        }
        TalkerProvider::Other => {
            return Err("settings.toml names this provider; change the scribe there.".to_owned());
        }
    };
    Ok(SettingsEdit::Scribe(choice))
}

/// A preset as it was measured for `role`: its thinking, output budget and route (D-152).
fn open_router_preset(preset: Preset, role: LlmRole) -> LlmChoice {
    let settings = preset.for_role(role);
    LlmChoice {
        kind: LlmKind::OpenAiCompatible,
        model: Some(preset.model.to_owned()),
        effort: Some(settings.effort),
        max_tokens: Some(settings.max_tokens),
        base_url: Some(OPENROUTER_BASE_URL.to_owned()),
        secret: secret_name(KeyName::OpenRouter),
        route: preset.route(),
    }
}

/// A model picked from OpenRouter's list: a preset keeps its settings; another model thinks
/// as little as the list says it allows, and OpenRouter routes it (D-152).
fn open_router_model(id: &str, listed: Option<&ModelInfo>) -> Result<LlmChoice, String> {
    let id = OpenRouterModelId::parse(id)
        .ok_or_else(|| "That is not an OpenRouter model id.".to_owned())?;
    if let Some(preset) = openrouter::preset(id.as_str()) {
        return Ok(open_router_preset(preset, LlmRole::Talker));
    }
    let reasoning = listed
        .filter(|model| model.id == id.as_str())
        .map(|model| &model.reasoning);
    let effort = reasoning.map_or(Effort::Low, talker_effort);
    Ok(LlmChoice {
        kind: LlmKind::OpenAiCompatible,
        model: Some(id.as_str().to_owned()),
        effort: Some(effort),
        max_tokens: Some(OPENROUTER_TALKER_MAX_TOKENS),
        base_url: Some(OPENROUTER_BASE_URL.to_owned()),
        secret: secret_name(KeyName::OpenRouter),
        route: Vec::new(),
    })
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
    if listed.iter().any(|name| name == OLLAMA_DEFAULT_MODEL) {
        return OLLAMA_DEFAULT_MODEL.to_owned();
    }
    listed
        .first()
        .cloned()
        .unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use boardai_core::setup::{
        InputDevice, KeyName, ModelInfo, ModelReasoning, OllamaStatus, SetupEdit, TalkerProvider,
        VoiceEngine,
    };
    use boardai_runtime::audio::InputChoice;
    use boardai_runtime::limits::SECRET_VALUE_MAX_BYTES;
    use boardai_runtime::llm::Effort;
    use boardai_runtime::llm::openai::OPENROUTER_BASE_URL;
    use boardai_runtime::llm::openrouter::{QWEN_3_8_27B, SCRIBE_PRESET, TALKER_PRESET};
    use boardai_runtime::settings::{LlmChoice, LlmKind, Settings, SettingsEdit};

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
            open_router_model: None,
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

    fn open_router(model: Option<&str>) -> SetupEdit {
        SetupEdit::Talker {
            provider: TalkerProvider::OpenRouter,
            model: model.map(str::to_owned),
        }
    }

    fn picked(edit: &SetupEdit, listed: Option<&ModelInfo>) -> (LlmChoice, LlmChoice) {
        let unreachable = OllamaStatus::Unreachable;
        let current = defaults();
        let mut context = known(&current, &unreachable, false, &[]);
        context.open_router_model = listed;
        let Ok(SettingsEdit::Llm { talker, scribe }) = settings_edit(edit, context) else {
            panic!("an OpenRouter talker for {edit:?}");
        };
        (talker, scribe)
    }

    #[test]
    fn openrouter_starts_from_its_presets_with_their_route_and_thinking() {
        let (talker, scribe) = picked(&open_router(None), None);
        assert_eq!(talker.model.as_deref(), Some(TALKER_PRESET.model));
        assert_eq!(talker.effort, Some(TALKER_PRESET.talker.effort));
        assert_eq!(talker.route, TALKER_PRESET.route());
        assert_eq!(talker.max_tokens, Some(TALKER_PRESET.talker.max_tokens));
        assert_eq!(talker.secret, super::secret_name(KeyName::OpenRouter));
        assert_eq!(talker.base_url.as_deref(), Some(OPENROUTER_BASE_URL));
        assert_eq!(scribe.model.as_deref(), Some(SCRIBE_PRESET.model));
        assert_eq!(scribe.route, SCRIBE_PRESET.route());
        assert_eq!(scribe.max_tokens, Some(SCRIBE_PRESET.scribe.max_tokens));
        assert_eq!(scribe.effort, Some(Effort::Medium));
        let (qwen, _) = picked(&open_router(Some(QWEN_3_8_27B.model)), None);
        assert_eq!(qwen.effort, Some(Effort::None));
        assert_eq!(qwen.route, QWEN_3_8_27B.route());
    }

    #[test]
    fn another_openrouter_model_thinks_as_little_as_its_listing_allows() {
        let listing = |id: &str, reasoning: ModelReasoning| ModelInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            context_tokens: None,
            price: None,
            reasoning,
            preset: false,
            route: Vec::new(),
        };
        let optional = listing("x/optional", ModelReasoning::Optional);
        let (talker, _) = picked(&open_router(Some("x/optional")), Some(&optional));
        assert_eq!(talker.effort, Some(Effort::None));
        assert!(
            talker.route.is_empty(),
            "OpenRouter routes models it was not measured on"
        );
        let always = listing(
            "x/always",
            ModelReasoning::Always {
                least_effort: "medium".to_owned(),
            },
        );
        let (talker, _) = picked(&open_router(Some("x/always")), Some(&always));
        assert_eq!(talker.effort, Some(Effort::Medium));
        // Unlisted, or a listing for another model: `low`, which every thinking model takes.
        let (talker, _) = picked(&open_router(Some("x/unlisted")), Some(&always));
        assert_eq!(talker.effort, Some(Effort::Low));
        let unreachable = OllamaStatus::Unreachable;
        for bad in ["gemma4:latest", "../../etc", "x/a b"] {
            let edit = open_router(Some(bad));
            assert!(settings_edit(&edit, known(&defaults(), &unreachable, false, &[])).is_err());
        }
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

    #[test]
    fn the_scribe_changes_on_the_talkers_provider_only() {
        let unreachable = OllamaStatus::Unreachable;
        let qwen = SetupEdit::Scribe {
            model: QWEN_3_8_27B.model.to_owned(),
        };
        let claude = defaults();
        assert!(
            settings_edit(&qwen, known(&claude, &unreachable, false, &[])).is_err(),
            "Claude's digests stay on Claude Haiku"
        );
        let mut open_router = defaults();
        open_router.talker.kind = LlmKind::OpenAiCompatible;
        open_router.talker.base_url = Some(OPENROUTER_BASE_URL.to_owned());
        let Ok(SettingsEdit::Scribe(preset)) =
            settings_edit(&qwen, known(&open_router, &unreachable, false, &[]))
        else {
            panic!("a scribe on OpenRouter");
        };
        assert_eq!(preset.route, QWEN_3_8_27B.route());
        assert_eq!(preset.effort, Some(Effort::Medium));
        let other = SetupEdit::Scribe {
            model: "x/never-thinks".to_owned(),
        };
        let listed = ModelInfo {
            id: "x/never-thinks".to_owned(),
            name: "x".to_owned(),
            context_tokens: None,
            price: None,
            reasoning: ModelReasoning::Never,
            preset: false,
            route: Vec::new(),
        };
        let mut context = known(&open_router, &unreachable, false, &[]);
        context.open_router_model = Some(&listed);
        let Ok(SettingsEdit::Scribe(plain)) = settings_edit(&other, context) else {
            panic!("a listed model");
        };
        assert_eq!(plain.effort, Some(Effort::None));
        assert!(plain.route.is_empty());
        assert_eq!(plain.max_tokens, None, "the scribe's own budget");
        for bad in ["gemma4:latest", "../x"] {
            let edit = SetupEdit::Scribe {
                model: bad.to_owned(),
            };
            assert!(settings_edit(&edit, known(&open_router, &unreachable, false, &[])).is_err());
        }
        let mut local = defaults();
        local.talker.kind = LlmKind::OpenAiCompatible;
        local.talker.base_url = Some(boardai_runtime::llm::openai::OLLAMA_BASE_URL.to_owned());
        let small = SetupEdit::Scribe {
            model: "qwen3.5:2b".to_owned(),
        };
        let Ok(SettingsEdit::Scribe(ollama)) =
            settings_edit(&small, known(&local, &unreachable, false, &[]))
        else {
            panic!("an Ollama scribe");
        };
        assert_eq!(ollama.model.as_deref(), Some("qwen3.5:2b"));
        assert_eq!(ollama.effort, Some(Effort::None));
    }

    #[test]
    fn a_code_folder_is_removed_by_its_place_in_the_list() {
        let unreachable = OllamaStatus::Unreachable;
        let mut current = defaults();
        current.code_roots = vec![
            std::path::PathBuf::from("/a"),
            std::path::PathBuf::from("/b"),
        ];
        let second = SetupEdit::RemoveCodeFolder { index: 1 };
        assert_eq!(
            settings_edit(&second, known(&current, &unreachable, false, &[])),
            Ok(SettingsEdit::RemoveCodeRoot(1))
        );
        let gone = SetupEdit::RemoveCodeFolder { index: 2 };
        assert!(settings_edit(&gone, known(&current, &unreachable, false, &[])).is_err());
    }
}
