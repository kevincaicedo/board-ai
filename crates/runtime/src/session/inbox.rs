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
use tokio::sync::{mpsc, watch};

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
    pub async fn voice_start(&self) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceStart).await
    }

    pub async fn voice_end(&self, voiced_ms: u32) -> Result<(), SessionClosed> {
        self.inbox.send(Input::VoiceEnd { voiced_ms }).await
    }

    /// A partial (`is_final: false`) replaces the previous partial; a final closes it.
    pub async fn transcript(&self, text: String, is_final: bool) -> Result<(), SessionClosed> {
        self.inbox.send(Input::Transcript { text, is_final }).await
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
