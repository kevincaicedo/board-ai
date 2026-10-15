//! The ports the orchestrator drives for the actors built in other streams: speech out and the
//! store. Each is small on purpose: wiring the real actor in is implementing one trait.
//!
//! Speech in has no trait: it is anything holding a [`SpeechInLink`](super::SpeechInLink).

use std::fmt;
use std::sync::Arc;

use boardai_core::effect::{Gain, LogRecord};
use boardai_core::ids::ClauseId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use futures_util::future::BoxFuture;

/// Speech out (LLD §3.1, §3.4): synthesizes clauses and plays them in order.
///
/// The orchestrator calls it on its own task, so every method returns at once and the work
/// happens on the implementation's actor. The actor answers through its
/// [`SpeechOutLink`](super::SpeechOutLink):
/// - every accepted `synthesize` with `clause_ready` or `synthesis_failed`, never both;
/// - positions with `playback_at` while audio plays (at most one per 20 ms), and `drained`
///   once everything synthesized has played;
/// - every accepted `stop` with `voice_stopped`, after the 30 ms fade.
pub trait SpeechOut: Send {
    /// Queues a clause's speakable text. On `Err` the actor will not answer, and the
    /// orchestrator answers `SynthesisFailed` itself so the reply never waits for it.
    fn synthesize(&mut self, clause: ClauseId, text: String) -> Result<(), SpeechOutError>;

    /// Sets the voice's gain: ducked while a barge-in is decided, then full again.
    fn duck(&mut self, gain: Gain);

    /// Fades out and drops everything queued. On `Err` the orchestrator answers
    /// `VoiceStopped { last: None }` itself; the core then commits up to the last position it
    /// was told, which never runs ahead of the audio.
    fn stop(&mut self) -> Result<(), SpeechOutError>;

    /// Drops everything without answering: the session is closing.
    fn reset(&mut self);
}

/// Why speech out did not take a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechOutError {
    /// Its queue is full.
    Full,
    /// Its actor has stopped.
    Closed,
}

impl fmt::Display for SpeechOutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("the voice's queue is full"),
            Self::Closed => formatter.write_str("the voice has stopped"),
        }
    }
}

/// The store (LLD §3.6): the only writer of the event log, the vault and the index.
///
/// The record methods run on the orchestrator task for every step, so they must not block:
/// the store actor queues them and writes on its own time (it flushes every 250 ms). Secret
/// redaction happens inside the store, at its boundary (LLD §2.8).
pub trait StoreSink: Send + Sync {
    /// An input the core stepped, at its session time, for re-simulation.
    fn input(&self, at: SessionTime, input: &Input);

    /// A UI event the core produced, for replay.
    fn ui_event(&self, at: SessionTime, event: &UiEvent);

    /// A latency mark or a drop at a limit.
    fn log(&self, at: SessionTime, record: &LogRecord);

    /// A memory the policy already decided to keep or propose.
    fn remember(&self, at: SessionTime, memory: &MemoryWrite);

    /// The `recall` tool: memories, notes and digests matching `query`.
    fn recall(&self, query: String) -> BoxFuture<'static, ToolOutput>;

    /// The `open_session` tool: a past session's board.
    fn open_session(&self, id: String) -> BoxFuture<'static, ToolOutput>;

    /// Writes everything queued; the session is closing.
    fn flush(&self) -> BoxFuture<'static, ()>;
}

/// A shared store is a store, so a caller can keep a handle on it (the dev loop prints from it).
impl<S: StoreSink + ?Sized> StoreSink for Arc<S> {
    fn input(&self, at: SessionTime, input: &Input) {
        (**self).input(at, input);
    }

    fn ui_event(&self, at: SessionTime, event: &UiEvent) {
        (**self).ui_event(at, event);
    }

    fn log(&self, at: SessionTime, record: &LogRecord) {
        (**self).log(at, record);
    }

    fn remember(&self, at: SessionTime, memory: &MemoryWrite) {
        (**self).remember(at, memory);
    }

    fn recall(&self, query: String) -> BoxFuture<'static, ToolOutput> {
        (**self).recall(query)
    }

    fn open_session(&self, id: String) -> BoxFuture<'static, ToolOutput> {
        (**self).open_session(id)
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        (**self).flush()
    }
}
