//! The ports the orchestrator drives for the actors built in other streams: speech out and the
//! store. Each is small on purpose: wiring the real actor in is implementing one trait.
//!
//! Speech in has no trait: it is anything holding a [`SpeechInLink`](super::SpeechInLink).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::effect::{ForgetScope, Gain, LogRecord};
use boardai_core::ids::{CaptureId, ClauseId};
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolCall, ToolOutput};
use futures_util::future::BoxFuture;

use super::inbox::StoreLink;
use crate::cancel::CancelToken;

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
/// The record and memory methods run on the orchestrator task for every step, so they must not
/// block: the store queues them and writes on its own time (it flushes the log every 250 ms).
/// Memory writes, forgets and keeps are applied in the order they were sent, so "forget that"
/// always undoes what the core meant, and a change the store could not make is reported through
/// the [`StoreLink`] as `Input::MemoryFailed`. Secret redaction happens inside the store, at its
/// boundary (LLD §2.8).
pub trait StoreSink: Send + Sync {
    /// Where to report memory changes the store could not make. The session hands it over when
    /// it starts; a store that keeps nothing on disk has nothing to report.
    fn report_to(&self, link: StoreLink);

    /// An input the core stepped, at its session time, for re-simulation.
    fn input(&self, at: SessionTime, input: &Input);

    /// A UI event the core produced, for replay.
    fn ui_event(&self, at: SessionTime, event: &UiEvent);

    /// A latency mark or a drop at a limit.
    fn log(&self, at: SessionTime, record: &LogRecord);

    /// A memory the policy already decided to keep or propose.
    fn remember(&self, at: SessionTime, memory: &MemoryWrite);

    /// "Forget that" (the last capture) or "don't remember this session" (I10).
    fn forget(&self, at: SessionTime, scope: ForgetScope);

    /// "Yes, keep it": keep the memory the core proposed as `capture`.
    fn accept_proposal(&self, at: SessionTime, capture: CaptureId);

    /// A read the talker asked for (recall, a past board, a view), as its tool output. It
    /// sees every write sent before it.
    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput>;

    /// Opens a session: its log starts. Answers with its id and the context the talker starts
    /// with (where you left off, what BoardAI keeps about you).
    fn start_session(&self) -> BoxFuture<'static, SessionOpened>;

    /// Ends the session: its digest, or no trace at all when nobody spoke.
    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()>;

    /// Writes everything queued; the session is closing.
    fn flush(&self) -> BoxFuture<'static, ()>;
}

/// What the talker's store tools ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreRead {
    /// `recall`: memories, notes, digests and transcripts matching the words.
    Recall { query: String },
    /// `open_session`: a past board, by id or words from its title.
    PastSession { query: String },
    /// `show_library`.
    Library,
    /// `show_tasks`: open tasks for one project, or all.
    Tasks { project: Option<String> },
    /// `show_memory`.
    Memory,
    /// `play_session`: a session to replay, by id or words from its title.
    Replay { query: String },
}

impl StoreRead {
    /// The read a store tool call asks for; `None` for every other tool.
    #[must_use]
    pub fn of(call: &ToolCall) -> Option<Self> {
        match call {
            ToolCall::Recall { query } => Some(Self::Recall {
                query: query.clone(),
            }),
            ToolCall::OpenSession { id } => Some(Self::PastSession { query: id.clone() }),
            ToolCall::ShowLibrary => Some(Self::Library),
            ToolCall::ShowTasks { project } => Some(Self::Tasks {
                project: project
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            }),
            ToolCall::ShowMemory => Some(Self::Memory),
            ToolCall::PlaySession { id } => Some(Self::Replay { query: id.clone() }),
            ToolCall::ReadFile { .. }
            | ToolCall::SearchCode { .. }
            | ToolCall::ListDir { .. }
            | ToolCall::Research { .. }
            | ToolCall::Review { .. }
            | ToolCall::ProposeChange { .. }
            | ToolCall::Remember { .. }
            | ToolCall::AddTask { .. } => None,
        }
    }
}

/// A session the store opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOpened {
    /// The store's id for it; `None` when memory is off (the vault couldn't be used).
    pub session: Option<String>,
    /// For the talker's first request: the last session's recap and what BoardAI keeps about
    /// the user, bounded by `SESSION_CONTEXT_MAX_BYTES`.
    pub context: Option<String>,
    /// The idle line under the orb, from the last session's title ("Pick up Raft log
    /// compaction where we left off?"), within the core's `SUGGESTION_MAX_BYTES`.
    pub suggestion: Option<String>,
}

/// How a session ends.
#[derive(Clone, Debug)]
pub enum SessionClose {
    /// Nobody spoke: it leaves nothing in the vault.
    Discard,
    /// Its log is closed and its digest written: the scribe's within `deadline` (or until
    /// `cancel`), else the minimal one. `board` is the core's `.excalidraw` export.
    Digest {
        t_end: SessionTime,
        board: Option<String>,
        deadline: Duration,
        cancel: CancelToken,
    },
}

/// A shared store is a store, so a caller can keep a handle on it (the dev loop prints from it).
impl<S: StoreSink + ?Sized> StoreSink for Arc<S> {
    fn report_to(&self, link: StoreLink) {
        (**self).report_to(link);
    }

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

    fn forget(&self, at: SessionTime, scope: ForgetScope) {
        (**self).forget(at, scope);
    }

    fn accept_proposal(&self, at: SessionTime, capture: CaptureId) {
        (**self).accept_proposal(at, capture);
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        (**self).read(read)
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        (**self).start_session()
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        (**self).finish_session(close)
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        (**self).flush()
    }
}
