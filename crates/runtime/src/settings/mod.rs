//! Settings (LLD §3.8): `settings.toml` in the app's config folder, parsed once into typed
//! settings with defaults, and errors that name the field. Secrets are referenced by name only
//! and resolved through a [`SecretStore`]; a key pasted into the file is an error, and no error
//! ever quotes a value from the file. The first-run screen changes the file with
//! [`Settings::edit`].

mod edit;
mod raw;
mod secrets;
mod write;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::audio::InputChoice;
use crate::limits::SETTINGS_FILE_MAX_BYTES;
use crate::llm::Effort;

pub use edit::{LlmChoice, SettingsEdit};
pub use secrets::{MemorySecretStore, Secret, SecretError, SecretName, SecretStore};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub talker: LlmSettings,
    pub scribe: LlmSettings,
    pub stt: SttSettings,
    pub tts: TtsSettings,
    pub harness: HarnessSettings,
    pub voice: VoiceSettings,
    pub audio: AudioSettings,
    /// The vault, the source of truth for sessions, notes and memory (`~/BoardAI`).
    pub vault: PathBuf,
    /// Where "open the boardai repo" looks.
    pub code_roots: Vec<PathBuf>,
    pub mic: MicMode,
    /// `setup_complete = true`: the first-run screen was finished, so the app opens on the
    /// stage. Absent (a new or hand-written file) shows the first-run screen.
    pub setup_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmKind {
    Anthropic,
    OpenAiCompatible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmSettings {
    pub kind: LlmKind,
    pub model: String,
    pub effort: Effort,
    pub max_tokens: u32,
    /// `None` means the provider's own endpoint.
    pub base_url: Option<String>,
    /// Required for Anthropic; optional for OpenAI-compatible servers (Ollama needs none).
    pub secret: Option<SecretName>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SttKind {
    /// whisper.cpp, local.
    Whisper,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SttSettings {
    pub kind: SttKind,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TtsKind {
    /// Kokoro, local.
    Kokoro,
    ElevenLabs,
    /// The macOS system voice.
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtsSettings {
    pub kind: TtsKind,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub secret: Option<SecretName>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarnessChoice {
    /// Whichever is installed, Claude Code first.
    Auto,
    ClaudeCode,
    Pi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessSettings {
    pub kind: HarnessChoice,
    /// Passed to the harness as its model; `None` keeps the harness's own default.
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceSettings {
    /// A voice id of the configured text-to-speech engine; `default` is its default voice.
    pub id: String,
}

/// `[audio]`: which microphone BoardAI listens on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioSettings {
    /// `input = "system_default"` (the default), `"built_in"`, or a device's name as macOS shows
    /// it. A name that matches no device falls back to the system default, out loud.
    pub input: InputChoice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicMode {
    /// Always listening, with echo cancellation (D-011).
    Open,
    PushToTalk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Unreadable {
        detail: String,
    },
    TooLarge {
        size_bytes: u64,
    },
    /// The file is not valid TOML, or a field has the wrong type. Quoted values are removed
    /// from the message.
    Syntax {
        line: Option<usize>,
        message: String,
    },
    Invalid {
        field: String,
        reason: String,
    },
    /// Saving failed; the file on disk is unchanged.
    Unwritable {
        detail: String,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { detail } => write!(formatter, "settings.toml: {detail}"),
            Self::TooLarge { size_bytes } => write!(
                formatter,
                "settings.toml is {size_bytes} bytes; the limit is {SETTINGS_FILE_MAX_BYTES}"
            ),
            Self::Syntax {
                line: Some(line),
                message,
            } => write!(formatter, "settings.toml line {line}: {message}"),
            Self::Syntax {
                line: None,
                message,
            } => write!(formatter, "settings.toml: {message}"),
            Self::Invalid { field, reason } => {
                write!(formatter, "settings.toml `{field}`: {reason}")
            }
            Self::Unwritable { detail } => {
                write!(formatter, "settings.toml could not be saved: {detail}")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// What a first run starts from.
    #[must_use]
    pub fn defaults(home: &Path) -> Self {
        raw::defaults(home)
    }

    /// Parses `settings.toml`. `home` expands `~`.
    pub fn parse(text: &str, home: &Path) -> Result<Self, SettingsError> {
        raw::parse(text, home)
    }

    /// Reads and parses the file; a missing file means the defaults.
    pub fn load(path: &Path, home: &Path) -> Result<Self, SettingsError> {
        match read_text(path)? {
            Some(text) => Self::parse(&text, home),
            None => Ok(Self::defaults(home)),
        }
    }

    /// Applies `edits` to the file at `path`, creating it (and its folder) when missing. Fields
    /// the edits don't touch are kept as written; comments are not. The new text must parse
    /// with [`Settings::parse`] before it replaces the file, atomically, so a failed edit
    /// leaves the file as it was. A file that doesn't parse is never overwritten.
    pub fn edit(path: &Path, home: &Path, edits: &[SettingsEdit]) -> Result<Self, SettingsError> {
        edit::apply(path, home, edits)
    }
}

/// The file's text, bounded; `None` when there is no file.
fn read_text(path: &Path) -> Result<Option<String>, SettingsError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SettingsError::Unreadable {
                detail: error.to_string(),
            });
        }
    };
    if metadata.len() > SETTINGS_FILE_MAX_BYTES {
        return Err(SettingsError::TooLarge {
            size_bytes: metadata.len(),
        });
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| SettingsError::Unreadable {
            detail: error.to_string(),
        })
}
