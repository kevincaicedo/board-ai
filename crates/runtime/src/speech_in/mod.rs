//! Speech in (LLD §3.1, §3.4): voice activity detection and speech-to-text on the mic ring,
//! producing `VoiceOnset`, `VoiceStart`, `VoiceEnd { voiced_ms }` and
//! `Transcript { text, is_final }`.
//!
//! Two dedicated threads, never async tasks, since both run inference:
//!
//! - **VAD thread**: reads the mic ring in 32 ms windows, runs the detector, and feeds the
//!   threshold-and-hangover machine ([`vad::VadMachine`]). It sends voice starts and ends to the
//!   orchestrator and the utterance's audio (with [`SPEECH_PREROLL_MS`] before the onset) to the
//!   speech-to-text thread. Barge-in latency depends on this thread alone, so Whisper never
//!   delays it.
//! - **Speech-to-text thread**: decodes a partial every [`STT_PARTIAL_INTERVAL_MS`] over the
//!   current utterance only (a partial whose turn comes during a decode is skipped) and the
//!   final at the end of the utterance.
//!
//! [`SPEECH_PREROLL_MS`]: crate::limits::SPEECH_PREROLL_MS
//! [`STT_PARTIAL_INTERVAL_MS`]: crate::limits::STT_PARTIAL_INTERVAL_MS

mod setup;
mod silero;
mod stt_thread;
pub mod vad;
mod vocabulary;
mod whisper;
mod worker;

use std::fmt;

pub use setup::{SpeechInSetupError, load_speech_in};
pub use silero::SileroVad;
pub use vocabulary::repo_terms;
pub use whisper::WhisperStt;
pub use worker::{SpeechIn, SpeechInCounters};

/// Samples per VAD window at 16 kHz (32 ms): what Silero v5 takes.
pub const VAD_WINDOW_FRAMES: usize = 512;

/// A voice activity detector: the probability that a 32 ms window holds speech. Stateful (the
/// windows are consecutive), so it runs on one thread.
pub trait VoiceDetector: Send {
    fn probability(&mut self, window: &[f32; VAD_WINDOW_FRAMES]) -> Result<f32, VadError>;
    /// Forgets the stream so far (after a mute, say).
    fn reset(&mut self);
}

/// Speech-to-text over one utterance at a time (LLD §3.2). It runs on its own thread.
pub trait SpeechToText: Send {
    /// Appends 16 kHz mono audio to the current utterance.
    fn push(&mut self, pcm: &[f32]);
    /// Decodes the utterance so far; `None` when there is nothing to say yet.
    fn partial(&mut self) -> Result<Option<String>, SttError>;
    /// Decodes the whole utterance and starts a new one.
    fn finish(&mut self) -> Result<String, SttError>;
    /// Drops the utterance without decoding it.
    fn discard(&mut self);
    /// Repo symbols and recent terms to bias recognition; the engine bounds how many it keeps.
    fn set_vocabulary(&mut self, terms: &[String]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VadError {
    /// The model file could not be loaded.
    Model { detail: String },
    /// Inference failed or returned an unexpected shape.
    Inference { detail: String },
}

impl fmt::Display for VadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model { detail } => write!(formatter, "voice detector model: {detail}"),
            Self::Inference { detail } => write!(formatter, "voice detector failed: {detail}"),
        }
    }
}

impl std::error::Error for VadError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SttError {
    Model { detail: String },
    Decode { detail: String },
}

impl fmt::Display for SttError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model { detail } => write!(formatter, "speech-to-text model: {detail}"),
            Self::Decode { detail } => write!(formatter, "speech-to-text failed: {detail}"),
        }
    }
}

impl std::error::Error for SttError {}
