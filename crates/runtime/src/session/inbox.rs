//! The orchestrator's inbox and the typed links actors hold to it (LLD §3.1).
//!
//! Every input reaches the core through one bounded inbox, except playback positions, which go
//! on a `watch` so the latest replaces the rest instead of queuing. Actors get a link that can
//! send only their own inputs: speech in sends voice activity and transcripts, speech out sends
//! synthesis and playback reports. The talker's inputs carry their stream's generation, so the
//! orchestrator can drop those of a cancelled stream (D-034).

use std::fmt;
use std::sync::Arc;

use boardai_core::ids::ClauseId;
use boardai_core::input::{Input, PlaybackPosition, WordTimings};
use boardai_core::memory::MemoryChange;
use tokio::sync::{mpsc, watch};

use super::listening::Listening;
use crate::limits::SESSION_INBOX_CAPACITY;

/// One message for the orchestrator.
#[derive(Debug)]
pub(crate) enum Envelope {
    Input(Input),
    /// From talker stream `generation`; stepped only while that stream is the current one.
    Talker {
        generation: u64,
        input: Input,
    },
    /// A runtime failure the user should know about (no audio, no microphone access), shown
    /// as a `Problem` without going through the core.
    Problem {
        spoken: String,
        detail: String,
    },
    /// From the app (a drop, a drag) or one of the orchestrator's own source jobs (D-184).
    Source(super::orchestrator::SourceMessage),
}

/// The session has ended; nothing more is delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionClosed;

impl fmt::Display for SessionClosed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the session has ended")
    }
}

impl std::error::Error for SessionClosed {}

/// Why an input that could not wait was not delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxRefused {
    /// The inbox is at [`SESSION_INBOX_CAPACITY`]; the input was dropped.
    Full,
    Closed,
}

/// A sender into the orchestrator's inbox. Cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct Inbox {
    sender: mpsc::Sender<Envelope>,
}

impl Inbox {
    /// Waits for room: an actor on a task slows down rather than lose an input.
    pub(crate) async fn send(&self, input: Input) -> Result<(), SessionClosed> {
        self.sender
            .send(Envelope::Input(input))
            .await
            .map_err(|_| SessionClosed)
    }

    pub(crate) async fn send_talker(
        &self,
        generation: u64,
        input: Input,
    ) -> Result<(), SessionClosed> {
        self.sender
            .send(Envelope::Talker { generation, input })
            .await
            .map_err(|_| SessionClosed)
    }

    /// Waits for room, like `send`, for any envelope (a source job's answer).
    pub(crate) async fn send_envelope(&self, envelope: Envelope) -> Result<(), SessionClosed> {
        self.sender.send(envelope).await.map_err(|_| SessionClosed)
    }

    /// Never waits; a full inbox drops the envelope.
    pub(crate) fn try_send_envelope(&self, envelope: Envelope) -> Result<(), InboxRefused> {
        self.sender.try_send(envelope).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => InboxRefused::Full,
            mpsc::error::TrySendError::Closed(_) => InboxRefused::Closed,
        })
    }

    /// For callers that must not wait; a full inbox drops the input.
    pub(crate) fn try_send(&self, input: Input) -> Result<(), InboxRefused> {
        self.sender
            .try_send(Envelope::Input(input))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => InboxRefused::Full,
                mpsc::error::TrySendError::Closed(_) => InboxRefused::Closed,
            })
    }
}

/// The latest playback position, shared by speech out (writer) and the orchestrator (reader).
type Playback = watch::Sender<Option<PlaybackPosition>>;

/// The channels of one session, made before its actors so each can be handed its link.
#[derive(Debug)]
pub struct SessionChannels {
    pub(crate) inbox: Inbox,
    pub(crate) receiver: mpsc::Receiver<Envelope>,
    playback: Arc<Playback>,
    pub(crate) positions: watch::Receiver<Option<PlaybackPosition>>,
    /// What speech in should do; the orchestrator writes it after every step.
    pub(crate) listening: Arc<watch::Sender<Listening>>,
}

impl SessionChannels {
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(SESSION_INBOX_CAPACITY);
        let (playback, positions) = watch::channel(None);
        Self {
            inbox: Inbox { sender },
            receiver,
            playback: Arc::new(playback),
            positions,
            listening: Arc::new(watch::Sender::new(Listening::default())),
        }
    }

    /// What speech in should do as the conversation goes: whether the mic is heard, and the
    /// terms BoardAI drew. Latest wins.
    #[must_use]
    pub fn listening(&self) -> watch::Receiver<Listening> {
        self.listening.subscribe()
    }

    /// The link the runtime reports its own failures through.
    #[must_use]
    pub fn problems(&self) -> ProblemLink {
        ProblemLink {
            inbox: self.inbox.clone(),
        }
    }

    /// The link a speech-in source (VAD and STT, or typed text) sends through.
    #[must_use]
    pub fn speech_in(&self) -> SpeechInLink {
        SpeechInLink {
            inbox: self.inbox.clone(),
        }
    }

    /// The link a speech-out implementation answers through.
    #[must_use]
    pub fn speech_out(&self) -> SpeechOutLink {
        SpeechOutLink {
            inbox: self.inbox.clone(),
            playback: Arc::clone(&self.playback),
        }
    }

    /// The link the store reports memory changes it could not make through.
    #[must_use]
    pub fn store(&self) -> StoreLink {
        StoreLink {
            inbox: self.inbox.clone(),
        }
    }
}

impl Default for SessionChannels {
    fn default() -> Self {
        Self::new()
    }
}

/// What speech in may tell the core: voice activity and transcripts (LLD §2.2).
#[derive(Clone, Debug)]
pub struct SpeechInLink {
    inbox: Inbox,
}

impl SpeechInLink {
    /// The voice detector's first speech window: the voice ducks now, speech is confirmed later.
    pub async fn voice_onset(&self) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceOnset).await
    }

    pub async fn voice_start(&self) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceStart).await
    }

    /// `silence_ms`: the silence the detector heard before calling the end.
    pub async fn voice_end(&self, voiced_ms: u32, silence_ms: u32) -> Result<(), SessionClosed> {
        let end = Input::VoiceEnd {
            voiced_ms,
            silence_ms,
        };
        self.inbox.send(end).await
    }

    /// A partial (`is_final: false`) replaces the previous partial; a final closes it.
    pub async fn transcript(&self, text: String, is_final: bool) -> Result<(), SessionClosed> {
        self.inbox.send(Input::Transcript { text, is_final }).await
    }

    /// The final as decoded after a short quiet inside speech (D-262).
    pub async fn early_final(&self, text: String) -> Result<(), SessionClosed> {
        self.inbox.send(Input::EarlyFinal { text }).await
    }

    /// Speech came back after an early final: it no longer stands.
    pub async fn voice_resumed(&self) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceResumed).await
    }
}

/// What speech out tells the core (LLD §2.1, §2.4). Every accepted `Synthesize` is answered
/// with [`Self::clause_ready`] or [`Self::synthesis_failed`], every accepted `StopVoice` with
/// [`Self::voice_stopped`].
#[derive(Clone, Debug)]
pub struct SpeechOutLink {
    inbox: Inbox,
    playback: Arc<Playback>,
}

impl SpeechOutLink {
    pub async fn clause_ready(
        &self,
        clause: ClauseId,
        timings: WordTimings,
    ) -> Result<(), SessionClosed> {
        self.inbox
            .send(Input::ClauseReady { clause, timings })
            .await
    }

    pub async fn synthesis_failed(&self, clause: ClauseId) -> Result<(), SessionClosed> {
        self.inbox.send(Input::SynthesisFailed { clause }).await
    }

    /// Where playback is, from the audio clock. Never queued: the orchestrator steps only the
    /// latest, so publish at most one per 20 ms. Never blocks.
    pub fn playback_at(&self, position: PlaybackPosition) {
        self.playback.send_replace(Some(position));
    }

    /// Everything synthesized so far has played.
    pub async fn drained(&self) -> Result<(), SessionClosed> {
        self.inbox.send(Input::PlaybackDrained).await
    }

    /// The answer to `StopVoice`: the last position that actually played, or `None` when
    /// nothing was playing.
    pub async fn voice_stopped(&self, last: Option<PlaybackPosition>) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceStopped { last }).await
    }
}

/// Where the runtime reports what failed around the conversation: audio that would not start,
/// a microphone without permission, speech models not downloaded. Each becomes a `Problem`
/// caption, stamped and logged like the core's own UI events.
#[derive(Clone, Debug)]
pub struct ProblemLink {
    inbox: Inbox,
}

impl ProblemLink {
    /// Never waits: a problem reported while the inbox is full is dropped and logged, since a
    /// session that busy is already being heard from.
    pub fn report(&self, spoken: impl Into<String>, detail: impl Into<String>) {
        let problem = Envelope::Problem {
            spoken: spoken.into(),
            detail: detail.into(),
        };
        if self.inbox.sender.try_send(problem).is_err() {
            tracing::warn!("a runtime problem was not shown: the session is full or closed");
        }
    }
}

/// What the store tells the core: a memory change it could not make (LLD §2.8), which BoardAI
/// already said was done.
#[derive(Clone, Debug)]
pub struct StoreLink {
    inbox: Inbox,
}

impl StoreLink {
    /// Never waits: the store's writer must never wait on the orchestrator, which may itself be
    /// waiting for the writer to drain (a read, a session's start or end). A full inbox drops
    /// the report; the caller logs it.
    pub fn memory_failed(&self, change: MemoryChange, detail: String) -> Result<(), InboxRefused> {
        self.inbox.try_send(Input::MemoryFailed { change, detail })
    }
}
