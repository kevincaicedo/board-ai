//! Changing `settings.toml` from the first-run screen (LLD §3.8). An edit works on the file's
//! fields as written, so whatever it doesn't touch stays exactly as the user set it (or left
//! it to its default). The result is checked with the same parser the app starts with before
//! it replaces the file, and the replacement is atomic: a temporary file in the same folder,
//! synced, then renamed over the old one.
//!
//! Secrets never pass through here: an edit names a secret, and `SecretName` cannot hold a key.

use std::io::Write as _;
use std::path::Path;

use super::raw::{
    self, ANTHROPIC_SECRET, ELEVENLABS_SECRET, MIC_OPEN, MIC_PUSH_TO_TALK, RawAudio, RawLlm,
    RawProvider, RawSettings,
};
use super::{LlmKind, MicMode, SecretName, Settings, SettingsError, TtsKind, read_text, write};
use crate::audio::InputChoice;
use crate::llm::Effort;
use crate::llm::openrouter::ProviderSlug;

/// One change to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsEdit {
    /// Replaces `[talker]` and `[scribe]`: a new provider starts from its own defaults.
    Llm {
        talker: LlmChoice,
        scribe: LlmChoice,
    },
    /// Replaces `[tts]` with this engine and its defaults.
    Tts(TtsKind),
    Mic(MicMode),
    Input(InputChoice),
    /// The first-run screen was finished.
    SetupComplete,
}

/// A role's provider as the first-run screen picks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmChoice {
    pub kind: LlmKind,
    /// `None` keeps the role's default (Anthropic's D-017 models).
    pub model: Option<String>,
    /// `None` keeps the role's default effort.
    pub effort: Option<Effort>,
    /// `None` keeps the role's default output budget.
    pub max_tokens: Option<u32>,
    /// `None` is the provider's own endpoint (Ollama's for `OpenAiCompatible`).
    pub base_url: Option<String>,
    pub secret: Option<SecretName>,
    /// OpenRouter's provider route; empty writes none.
    pub route: Vec<ProviderSlug>,
}

pub(super) fn apply(
    path: &Path,
    home: &Path,
    edits: &[SettingsEdit],
) -> Result<Settings, SettingsError> {
    let mut raw = match read_text(path)? {
        Some(text) => {
            // A file the app can't start from is never overwritten: the user may be halfway
            // through editing it by hand.
            Settings::parse(&text, home)?;
            raw::read_raw(&text)?
        }
        None => RawSettings::default(),
    };
    for edit in edits {
        change(&mut raw, edit);
    }
    let text = write::to_toml(&raw);
    let settings = Settings::parse(&text, home)?;
    replace(path, &text)?;
    Ok(settings)
}

fn change(raw: &mut RawSettings, edit: &SettingsEdit) {
    match edit {
        SettingsEdit::Llm { talker, scribe } => {
            raw.talker = Some(llm(talker));
            raw.scribe = Some(llm(scribe));
        }
        SettingsEdit::Tts(kind) => {
            raw.tts = Some(RawProvider {
                kind: Some(
                    match kind {
                        TtsKind::Kokoro => "kokoro",
                        TtsKind::ElevenLabs => "elevenlabs",
                        TtsKind::System => "system",
                    }
                    .to_owned(),
                ),
                secret: match kind {
                    TtsKind::ElevenLabs => Some(ELEVENLABS_SECRET.to_owned()),
                    TtsKind::Kokoro | TtsKind::System => None,
                },
                ..RawProvider::default()
            });
        }
        SettingsEdit::Mic(mode) => {
            raw.mic = Some(
                match mode {
                    MicMode::Open => MIC_OPEN,
                    MicMode::PushToTalk => MIC_PUSH_TO_TALK,
                }
                .to_owned(),
            );
        }
        SettingsEdit::Input(input) => {
            // The output is not chosen on the first-run screen; a hand-written one stays.
            let output = raw.audio.take().and_then(|audio| audio.output);
            raw.audio = Some(RawAudio {
                input: Some(input.as_setting().to_owned()),
                output,
            });
        }
        SettingsEdit::SetupComplete => raw.setup_complete = Some(true),
    }
}

fn llm(choice: &LlmChoice) -> RawLlm {
    RawLlm {
        kind: Some(
            match choice.kind {
                LlmKind::Anthropic => "anthropic",
                LlmKind::OpenAiCompatible => "openai_compatible",
            }
            .to_owned(),
        ),
        model: choice.model.clone(),
        effort: choice.effort.map(|effort| effort.as_str().to_owned()),
        max_tokens: choice.max_tokens.map(i64::from),
        base_url: choice.base_url.clone(),
        secret: choice.secret.as_ref().map_or_else(
            || match choice.kind {
                // Written out so the file says where the key comes from.
                LlmKind::Anthropic => Some(ANTHROPIC_SECRET.to_owned()),
                LlmKind::OpenAiCompatible => None,
            },
            |name| Some(name.as_str().to_owned()),
        ),
        route: (!choice.route.is_empty()).then(|| {
            choice
                .route
                .iter()
                .map(|slug| slug.as_str().to_owned())
                .collect()
        }),
    }
}

/// Writes `text` to a temporary file beside `path`, syncs it, and renames it over `path`.
fn replace(path: &Path, text: &str) -> Result<(), SettingsError> {
    let unwritable = |error: std::io::Error| SettingsError::Unwritable {
        detail: error.to_string(),
    };
    let folder = path.parent().ok_or_else(|| SettingsError::Unwritable {
        detail: "the settings path has no folder".to_owned(),
    })?;
    std::fs::create_dir_all(folder).map_err(unwritable)?;
    let name = path
        .file_name()
        .map_or_else(|| "settings.toml".into(), |name| name.to_string_lossy());
    let temporary = folder.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&temporary, path));
    if let Err(error) = renamed {
        let _ = std::fs::remove_file(&temporary);
        return Err(unwritable(error));
    }
    Ok(())
}
