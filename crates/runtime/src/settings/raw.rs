//! The file's shape, parsed into [`Settings`]. Enumerations are read as strings so that every
//! error is ours: it names the field and lists the allowed values, and never echoes a value.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{
    AudioSettings, HarnessChoice, HarnessSettings, LlmKind, LlmSettings, MicMode, SecretName,
    Settings, SettingsError, SttKind, SttSettings, TtsKind, TtsSettings, VoiceSettings,
};
use crate::audio::{InputChoice, OutputChoice};
use crate::limits::OPENROUTER_ROUTE_MAX;
use crate::limits::{LLM_MAX_TOKENS_MAX, SCRIBE_MAX_TOKENS_DEFAULT, TALKER_MAX_TOKENS_DEFAULT};
use crate::llm::base_url::is_loopback;
use crate::llm::openai::OLLAMA_BASE_URL;
use crate::llm::openrouter::{ProviderSlug, RouteError, is_openrouter, parse_route};
use crate::llm::{DEFAULT_SCRIBE_MODEL, DEFAULT_TALKER_MODEL, Effort};

pub(super) const ANTHROPIC_SECRET: &str = "anthropic";
pub(super) const ELEVENLABS_SECRET: &str = "elevenlabs";
const DEFAULT_STT_MODEL: &str = "base.en";
/// `[voice] id` when settings name none: each engine's own default voice.
pub const DEFAULT_VOICE: &str = "default";

/// The file as written: `None` is a field the file leaves out (its default). Editing keeps
/// this shape, so a saved file holds only what was chosen (`super::edit`).
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSettings {
    pub(super) setup_complete: Option<bool>,
    pub(super) vault: Option<String>,
    pub(super) code_roots: Option<Vec<String>>,
    pub(super) mic: Option<String>,
    pub(super) voice: Option<RawVoice>,
    pub(super) audio: Option<RawAudio>,
    pub(super) talker: Option<RawLlm>,
    pub(super) scribe: Option<RawLlm>,
    pub(super) stt: Option<RawProvider>,
    pub(super) tts: Option<RawProvider>,
    pub(super) harness: Option<RawHarness>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawLlm {
    pub(super) kind: Option<String>,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) max_tokens: Option<i64>,
    pub(super) base_url: Option<String>,
    pub(super) secret: Option<String>,
    pub(super) route: Option<Vec<String>>,
    pub(super) early_start: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProvider {
    pub(super) kind: Option<String>,
    pub(super) model: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) secret: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawHarness {
    pub(super) kind: Option<String>,
    pub(super) model: Option<String>,
    pub(super) research_model: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawVoice {
    pub(super) id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawAudio {
    pub(super) input: Option<String>,
    pub(super) output: Option<String>,
}

#[derive(Clone, Copy)]
enum Role {
    Talker,
    Scribe,
}

pub(super) fn defaults(home: &Path) -> Settings {
    let empty = RawSettings::default();
    // The defaults are valid by construction; the fallback only guards a broken constant.
    parse_raw(empty, home).unwrap_or_else(|error| {
        debug_assert!(false, "default settings are invalid: {error}");
        Settings {
            talker: fallback_llm(Role::Talker),
            scribe: fallback_llm(Role::Scribe),
            stt: SttSettings {
                kind: SttKind::Whisper,
                model: DEFAULT_STT_MODEL.to_owned(),
            },
            tts: TtsSettings {
                kind: TtsKind::Kokoro,
                model: None,
                base_url: None,
                secret: None,
            },
            harness: HarnessSettings {
                kind: HarnessChoice::Auto,
                model: None,
                research_model: None,
            },
            voice: VoiceSettings {
                id: DEFAULT_VOICE.to_owned(),
            },
            audio: AudioSettings::default(),
            vault: home.join("BoardAI"),
            code_roots: Vec::new(),
            mic: MicMode::Open,
            setup_complete: false,
        }
    })
}

fn fallback_llm(role: Role) -> LlmSettings {
    let (model, effort, max_tokens) = role_defaults(role);
    LlmSettings {
        kind: LlmKind::Anthropic,
        model: model.to_owned(),
        effort,
        max_tokens,
        base_url: None,
        secret: SecretName::parse(ANTHROPIC_SECRET),
        route: Vec::new(),
        early_start: true,
    }
}

pub(super) fn parse(text: &str, home: &Path) -> Result<Settings, SettingsError> {
    parse_raw(read_raw(text)?, home)
}

/// The file's fields as written, before defaults and checks.
pub(super) fn read_raw(text: &str) -> Result<RawSettings, SettingsError> {
    toml::from_str(text).map_err(|error| SettingsError::Syntax {
        line: error.span().map(|span| line_of(text, span.start)),
        message: without_quoted_values(error.message()),
    })
}

fn parse_raw(raw: RawSettings, home: &Path) -> Result<Settings, SettingsError> {
    Ok(Settings {
        talker: llm(raw.talker.unwrap_or_default(), Role::Talker)?,
        scribe: llm(raw.scribe.unwrap_or_default(), Role::Scribe)?,
        stt: stt(raw.stt.unwrap_or_default())?,
        tts: tts(raw.tts.unwrap_or_default())?,
        harness: harness(raw.harness.unwrap_or_default())?,
        voice: VoiceSettings {
            id: non_empty("voice.id", raw.voice.and_then(|voice| voice.id))?
                .unwrap_or_else(|| DEFAULT_VOICE.to_owned()),
        },
        audio: audio(raw.audio.unwrap_or_default())?,
        vault: match raw.vault {
            Some(vault) => path("vault", &vault, home)?,
            None => home.join("BoardAI"),
        },
        code_roots: raw
            .code_roots
            .unwrap_or_default()
            .iter()
            .map(|root| path("code_roots", root, home))
            .collect::<Result<_, _>>()?,
        mic: match raw.mic.as_deref() {
            None | Some(MIC_OPEN) => MicMode::Open,
            Some(MIC_PUSH_TO_TALK) => MicMode::PushToTalk,
            Some(_) => return Err(invalid("mic", "must be `open` or `push_to_talk`")),
        },
        setup_complete: raw.setup_complete.unwrap_or(false),
    })
}

pub(super) const MIC_OPEN: &str = "open";
pub(super) const MIC_PUSH_TO_TALK: &str = "push_to_talk";

fn role_defaults(role: Role) -> (&'static str, Effort, u32) {
    match role {
        Role::Talker => (DEFAULT_TALKER_MODEL, Effort::Low, TALKER_MAX_TOKENS_DEFAULT),
        Role::Scribe => (
            DEFAULT_SCRIBE_MODEL,
            Effort::Medium,
            SCRIBE_MAX_TOKENS_DEFAULT,
        ),
    }
}

fn llm(raw: RawLlm, role: Role) -> Result<LlmSettings, SettingsError> {
    let section = match role {
        Role::Talker => "talker",
        Role::Scribe => "scribe",
    };
    let field = |name: &str| format!("{section}.{name}");
    let (default_model, default_effort, default_max_tokens) = role_defaults(role);
    let kind = match raw.kind.as_deref() {
        None | Some("anthropic") => LlmKind::Anthropic,
        Some("openai_compatible") => LlmKind::OpenAiCompatible,
        Some(_) => {
            return Err(invalid(
                &field("kind"),
                "must be `anthropic` or `openai_compatible`",
            ));
        }
    };
    let model = non_empty(&field("model"), raw.model)?;
    let base_url = raw
        .base_url
        .map(|url| base_url(&field("base_url"), &url))
        .transpose()?;
    let secret = raw
        .secret
        .map(|name| secret_name(&field("secret"), &name))
        .transpose()?;
    let (model, base_url, secret) = match kind {
        LlmKind::Anthropic => (
            model.unwrap_or_else(|| default_model.to_owned()),
            base_url,
            Some(secret.map_or_else(|| secret_name(&field("secret"), ANTHROPIC_SECRET), Ok)?),
        ),
        LlmKind::OpenAiCompatible => (
            model.ok_or_else(|| invalid(&field("model"), "is required for `openai_compatible`"))?,
            Some(base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_owned())),
            secret,
        ),
    };
    if let (Some(url), Some(_)) = (&base_url, &secret) {
        carries_a_key(&field("base_url"), url)?;
    }
    let route = route(&field("route"), raw.route.as_deref(), base_url.as_deref())?;
    let early_start = match (role, raw.early_start) {
        (Role::Scribe, Some(_)) => {
            return Err(invalid(&field("early_start"), "applies to the talker only"));
        }
        (Role::Talker | Role::Scribe, value) => value.unwrap_or(true),
    };
    Ok(LlmSettings {
        kind,
        model,
        effort: effort(&field("effort"), raw.effort.as_deref(), default_effort)?,
        max_tokens: max_tokens(&field("max_tokens"), raw.max_tokens, default_max_tokens)?,
        base_url,
        secret,
        route,
        early_start,
    })
}

/// A provider route, which only OpenRouter takes (D-152).
fn route(
    field: &str,
    value: Option<&[String]>,
    base_url: Option<&str>,
) -> Result<Vec<ProviderSlug>, SettingsError> {
    let Some(slugs) = value else {
        return Ok(Vec::new());
    };
    if !base_url.is_some_and(is_openrouter) {
        return Err(invalid(field, "applies to OpenRouter only"));
    }
    parse_route(slugs).map_err(|error| match error {
        RouteError::TooLong => invalid(
            field,
            &format!("names at most {OPENROUTER_ROUTE_MAX} providers"),
        ),
        RouteError::BadSlug { index } => invalid(
            &format!("{field}[{index}]"),
            "must be a provider slug of lowercase letters, digits, `-`, `_`, `.` or `/`",
        ),
    })
}

fn effort(field: &str, value: Option<&str>, default: Effort) -> Result<Effort, SettingsError> {
    match value {
        None => Ok(default),
        Some("none") => Ok(Effort::None),
        Some("low") => Ok(Effort::Low),
        Some("medium") => Ok(Effort::Medium),
        Some("high") => Ok(Effort::High),
        Some("xhigh") => Ok(Effort::ExtraHigh),
        Some("max") => Ok(Effort::Max),
        Some(_) => Err(invalid(
            field,
            "must be none, low, medium, high, xhigh or max",
        )),
    }
}

fn max_tokens(field: &str, value: Option<i64>, default: u32) -> Result<u32, SettingsError> {
    let Some(value) = value else {
        return Ok(default);
    };
    u32::try_from(value)
        .ok()
        .filter(|tokens| (1..=LLM_MAX_TOKENS_MAX).contains(tokens))
        .ok_or_else(|| {
            invalid(
                field,
                &format!("must be between 1 and {LLM_MAX_TOKENS_MAX}"),
            )
        })
}

fn audio(raw: RawAudio) -> Result<AudioSettings, SettingsError> {
    Ok(AudioSettings {
        input: non_empty("audio.input", raw.input)?
            .map_or_else(InputChoice::default, |input| InputChoice::parse(&input)),
        output: non_empty("audio.output", raw.output)?
            .map_or_else(OutputChoice::default, |output| OutputChoice::parse(&output)),
    })
}

fn stt(raw: RawProvider) -> Result<SttSettings, SettingsError> {
    match raw.kind.as_deref() {
        None | Some("whisper") => {}
        Some(_) => return Err(invalid("stt.kind", "must be `whisper`")),
    }
    local_only("stt", &raw)?;
    Ok(SttSettings {
        kind: SttKind::Whisper,
        model: non_empty("stt.model", raw.model)?.unwrap_or_else(|| DEFAULT_STT_MODEL.to_owned()),
    })
}

fn tts(raw: RawProvider) -> Result<TtsSettings, SettingsError> {
    let kind = match raw.kind.as_deref() {
        None | Some("kokoro") => TtsKind::Kokoro,
        Some("elevenlabs") => TtsKind::ElevenLabs,
        Some("system") => TtsKind::System,
        Some(_) => {
            return Err(invalid(
                "tts.kind",
                "must be `kokoro`, `elevenlabs` or `system`",
            ));
        }
    };
    let model = non_empty("tts.model", raw.model.clone())?;
    match kind {
        TtsKind::Kokoro | TtsKind::System => {
            local_only("tts", &raw)?;
            Ok(TtsSettings {
                kind,
                model,
                base_url: None,
                secret: None,
            })
        }
        TtsKind::ElevenLabs => {
            let base_url = raw
                .base_url
                .map(|url| base_url("tts.base_url", &url))
                .transpose()?;
            if let Some(url) = &base_url {
                carries_a_key("tts.base_url", url)?;
            }
            Ok(TtsSettings {
                kind,
                model,
                base_url,
                secret: Some(secret_name(
                    "tts.secret",
                    raw.secret.as_deref().unwrap_or(ELEVENLABS_SECRET),
                )?),
            })
        }
    }
}

/// Local engines take no endpoint and no key; setting one is a mistake worth saying.
fn local_only(section: &str, raw: &RawProvider) -> Result<(), SettingsError> {
    if raw.base_url.is_some() {
        return Err(invalid(
            &format!("{section}.base_url"),
            "does not apply to a local engine",
        ));
    }
    if raw.secret.is_some() {
        return Err(invalid(
            &format!("{section}.secret"),
            "does not apply to a local engine",
        ));
    }
    Ok(())
}

fn harness(raw: RawHarness) -> Result<HarnessSettings, SettingsError> {
    let kind = match raw.kind.as_deref() {
        None | Some("auto") => HarnessChoice::Auto,
        Some("claude_code") => HarnessChoice::ClaudeCode,
        Some("pi") => HarnessChoice::Pi,
        Some(_) => {
            return Err(invalid(
                "harness.kind",
                "must be `auto`, `claude_code` or `pi`",
            ));
        }
    };
    Ok(HarnessSettings {
        kind,
        model: non_empty("harness.model", raw.model)?,
        research_model: non_empty("harness.research_model", raw.research_model)?,
    })
}

fn non_empty(field: &str, value: Option<String>) -> Result<Option<String>, SettingsError> {
    match value {
        Some(value) if value.trim().is_empty() => Err(invalid(field, "must not be empty")),
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn base_url(field: &str, value: &str) -> Result<String, SettingsError> {
    let parsed = reqwest::Url::parse(value).map_err(|_| invalid(field, "is not a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(invalid(field, "must be an http or https URL"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid(field, "must not carry credentials; use `secret`"));
    }
    Ok(value.to_owned())
}

/// A base URL a key goes to (the role names a `secret`): https, unless it is this Mac, so a key
/// never crosses a network in cleartext (law 10). `url` already parsed in [`base_url`].
fn carries_a_key(field: &str, url: &str) -> Result<(), SettingsError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| invalid(field, "is not a valid URL"))?;
    if parsed.scheme() == "https" || is_loopback(&parsed) {
        return Ok(());
    }
    Err(invalid(
        field,
        "must be https when a key goes with it (plain http only to this Mac)",
    ))
}

/// A secret's name. A value that looks like the key itself gets its own error, so the user
/// learns where keys go.
fn secret_name(field: &str, value: &str) -> Result<SecretName, SettingsError> {
    if value.starts_with("sk-") || value.len() > 40 {
        return Err(invalid(
            field,
            "looks like a key; keys live in the Keychain and settings name them (`anthropic`)",
        ));
    }
    SecretName::parse(value).ok_or_else(|| {
        invalid(
            field,
            "must be a name of lowercase letters, digits, `.`, `_` or `-`",
        )
    })
}

/// An absolute path, or one under `~/`.
fn path(field: &str, value: &str, home: &Path) -> Result<PathBuf, SettingsError> {
    if value == "~" {
        return Ok(home.to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return Ok(home.join(rest));
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(invalid(field, "must be an absolute path or start with ~/"))
    }
}

fn invalid(field: &str, reason: &str) -> SettingsError {
    SettingsError::Invalid {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .map_or(0, |before| before.matches('\n').count())
        + 1
}

/// Serde and TOML messages can quote a string value (`invalid type: string "…"`); the value
/// could be a pasted key, so quoted text is dropped.
fn without_quoted_values(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut quoted = false;
    for character in message.chars() {
        if character == '"' {
            if !quoted {
                out.push_str("\"…\"");
            }
            quoted = !quoted;
        } else if !quoted {
            out.push(character);
        }
    }
    out
}
