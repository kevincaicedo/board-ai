//! Speech out (LLD §2.4, §3.1, §3.4): text-to-speech engines behind one seam, and the actor that
//! turns `Synthesize`, `Duck` and `StopVoice` effects into voice and reports playback.
//!
//! Engines take a clause's speakable text and return 24 kHz mono audio plus word timings as
//! byte offsets into that text. The actor synthesizes ahead within the lookahead limits, fills
//! the voice ring in clause order, and reports `ClauseReady`, `PlaybackDrained` and
//! `VoiceStopped` on a bounded channel and the playback position on a `watch` (latest wins).

mod actor;
mod elevenlabs;
mod engine;
mod kokoro;
mod system;
mod timeline;
pub mod timings;

use std::fmt;
use std::time::Duration;

use boardai_core::ids::ClauseId;
use boardai_core::input::WordTiming;
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

pub use actor::{SpeechOut, SpeechOutChannels, SpeechOutCommand, SpeechOutConfig};
pub use elevenlabs::{
    ELEVENLABS_BASE_URL, ELEVENLABS_DEFAULT_VOICE_ID, ElevenLabsOptions, ElevenLabsVoice,
};
pub use engine::{KOKORO_QUANTIZED_MODEL, engine_for};
pub use kokoro::{KOKORO_VOICE, KokoroFiles, KokoroVoice};
pub use system::{SystemVoice, SystemVoiceInfo, system_voices};

/// A clause as the core cut it: its id and its speakable text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClauseText {
    pub id: ClauseId,
    pub text: String,
}

/// A voice of the configured engine. `default` is the engine's own default voice.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VoiceId(String);

impl VoiceId {
    pub const DEFAULT: &'static str = "default";

    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }
}

impl Default for VoiceId {
    fn default() -> Self {
        Self(Self::DEFAULT.to_owned())
    }
}

/// One piece of a clause's synthesis, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum TtsChunk {
    /// 24 kHz mono samples continuing the clause's audio.
    Audio(Vec<f32>),
    /// Exact word starts: byte offsets into the clause's text, milliseconds from the start of
    /// the clause's audio. A clause with no `Words` chunk gets estimated timings.
    Words(Vec<WordTiming>),
}

/// A clause's synthesis. Dropping it cancels the work.
pub type TtsStream = BoxStream<'static, Result<TtsChunk, TtsError>>;

/// A text-to-speech engine (LLD §3.2). Per clause, so dynamic dispatch is fine here.
pub trait TextToSpeech: Send + Sync {
    /// Synthesizes one clause. The stream ends after the last chunk or after an error, and
    /// stops early (with no further item) when `cancel` fires.
    fn synthesize(&self, clause: &ClauseText, voice: &VoiceId, cancel: CancelToken) -> TtsStream;
}

/// Why a synthesis failed; the category survives to the log and the fallback decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TtsError {
    /// A missing or rejected key.
    Auth {
        detail: String,
    },
    RateLimited {
        retry_after: Option<Duration>,
        detail: String,
    },
    Network {
        detail: String,
    },
    Timeout,
    /// The engine sent something we could not read.
    BadOutput {
        detail: String,
    },
    /// The provider rejected the request (a bad voice id, quota, a server error).
    Provider {
        status: Option<u16>,
        detail: String,
    },
    /// The engine cannot run here: its model is missing, its voice is not installed, or (for
    /// the system voice) the main thread runs no event loop.
    Unavailable {
        detail: String,
    },
}

impl fmt::Display for TtsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth { detail } => write!(formatter, "text-to-speech key rejected: {detail}"),
            Self::RateLimited { detail, .. } => {
                write!(formatter, "text-to-speech rate limited: {detail}")
            }
            Self::Network { detail } => write!(formatter, "text-to-speech network error: {detail}"),
            Self::Timeout => formatter.write_str("text-to-speech took too long"),
            Self::BadOutput { detail } => {
                write!(formatter, "unreadable text-to-speech output: {detail}")
            }
            Self::Provider {
                status: Some(status),
                detail,
            } => write!(formatter, "text-to-speech rejected ({status}): {detail}"),
            Self::Provider {
                status: None,
                detail,
            } => write!(formatter, "text-to-speech rejected: {detail}"),
            Self::Unavailable { detail } => {
                write!(formatter, "text-to-speech unavailable: {detail}")
            }
        }
    }
}

impl std::error::Error for TtsError {}
