//! A conversation session (LLD §3.1): the orchestrator task that owns the core, and the seams
//! the other actors plug into.
//!
//! Start one in three moves: make the [`SessionChannels`], hand each actor its link (speech in
//! gets a [`SpeechInLink`] and follows [`SessionChannels::listening`], speech out a
//! [`SpeechOutLink`]), then [`spawn`] the session with the parts. [`LiveVoice`] puts the real
//! speech-out actor behind the [`SpeechOut`] port and [`LiveListener`] connects real speech in;
//! [`VirtualVoice`] and typed text ([`say_typed`]) stand in where there is no audio. All of them
//! outlive the conversation sessions the orchestrator opens and rests: each new session's core
//! gets the same voice, microphone and links. The store is a [`VaultSink`] over the vault, or a
//! [`MemoryStore`] when the vault can't be used (and in the text dev loop). Failures around the
//! conversation (no audio, no microphone access) are reported through
//! [`SessionChannels::problems`].
//!
//! The runtime's obligations to the core (LLD §2.1) are met here: inputs of a cancelled talker
//! stream never reach it, every `Synthesize` is answered, every `WakeAt` becomes a `Tick`, and
//! every `StopVoice` is answered with `VoiceStopped`.

mod deadlines;
mod inbox;
mod listening;
mod live_listener;
mod live_voice;
mod memory_store;
mod orchestrator;
mod ports;
mod script;
mod store_views;
mod talker;
mod timings;
mod typed;
mod vault_sink;
mod virtual_voice;
mod work;

use std::sync::Arc;

use boardai_core::CoreConfig;
use boardai_core::input::Input;
use boardai_core::protocol::UiCommand;
use tokio::task::JoinHandle;

pub use inbox::{
    InboxRefused, ProblemLink, SessionChannels, SessionClosed, SpeechInLink, SpeechOutLink,
};
pub use listening::{Listening, mic_open};
pub use live_listener::LiveListener;
pub use live_voice::LiveVoice;
pub use memory_store::MemoryStore;
pub use orchestrator::SessionStats;
pub use ports::{SessionClose, SessionOpened, SpeechOut, SpeechOutError, StoreRead, StoreSink};
pub use script::{
    Cue, Observed, Progress, Script, ScriptError, ScriptLine, run_script, wait_for_answer,
};
pub use talker::{
    TalkerSetupError, UnconfiguredTalker, scribe_from_settings, talker_config,
    talker_from_settings, talker_name,
};
pub use timings::estimate_timings;
pub use typed::say_typed;
pub use vault_sink::VaultSink;
pub use virtual_voice::{VirtualVoice, VirtualVoiceConfig};

use self::inbox::Inbox;
use self::orchestrator::{Orchestrator, OrchestratorParts, Receivers};
use self::work::Work;
use crate::bridge::{BridgeHandle, UiSink};
use crate::cancel::CancelToken;
use crate::clock::TokioClock;
use crate::harness::HarnessPool;
use crate::llm::LlmProvider;
use crate::tools::RepoTools;

/// Everything a session drives besides the core.
pub struct SessionParts {
    pub core: CoreConfig,
    /// Session time for every input, and the timers behind `WakeAt`.
    pub clock: TokioClock,
    pub talker: Arc<dyn LlmProvider>,
    /// The repo tools over the folders opened for this session (none is fine: the tools then
    /// answer that nothing is open).
    pub tools: RepoTools,
    /// `None` when no harness is installed or no repo is open: harness tools then fail out
    /// loud with `NotInstalled`.
    pub harness: Option<HarnessPool>,
    pub speech: Box<dyn SpeechOut>,
    pub store: Arc<dyn StoreSink>,
    /// Where the bridge sends UI batches.
    pub ui: Arc<dyn UiSink>,
}

/// A running session. Dropping it leaves the session running until [`SessionHandle::close`].
#[derive(Debug)]
pub struct SessionHandle {
    inbox: Inbox,
    speech_in: SpeechInLink,
    closing: CancelToken,
    task: JoinHandle<SessionStats>,
}

/// Starts the session's orchestrator and bridge on the current tokio runtime.
#[must_use]
pub fn spawn(channels: SessionChannels, parts: SessionParts) -> SessionHandle {
    let speech_in = channels.speech_in();
    let SessionChannels {
        inbox,
        receiver,
        positions,
        listening,
        ..
    } = channels;
    let closing = CancelToken::new();
    let orchestrator = Orchestrator::new(
        parts.core,
        parts.clock,
        OrchestratorParts {
            inbox: inbox.clone(),
            talker: parts.talker,
            work: Work::new(parts.tools, parts.harness),
            speech: parts.speech,
            store: parts.store,
            bridge: BridgeHandle::spawn(parts.ui),
            listening,
        },
    );
    let receivers = Receivers {
        inbox: receiver,
        positions,
        closing: closing.clone(),
    };
    let task = tokio::spawn(orchestrator.run(receivers));
    SessionHandle {
        inbox,
        speech_in,
        closing,
        task,
    }
}

impl SessionHandle {
    /// A command from the UI. It never waits: when the inbox is full it is dropped, since a
    /// stale pointer or mic toggle is worse than a missing one (D-027).
    pub fn command(&self, command: UiCommand) -> Result<(), InboxRefused> {
        self.inbox.try_send(Input::Command(command))
    }

    /// The link typed text or a speech-in actor sends utterances through.
    #[must_use]
    pub fn speech_in(&self) -> SpeechInLink {
        self.speech_in.clone()
    }

    /// Closes the session: cancels the talker, harness tasks, tools, timers and the voice,
    /// sends the last UI events and flushes the store.
    pub async fn close(self) -> SessionStats {
        self.closing.cancel();
        match self.task.await {
            Ok(stats) => stats,
            Err(error) => {
                tracing::error!(%error, "the session task ended abnormally");
                SessionStats::default()
            }
        }
    }
}

#[cfg(test)]
mod tests;
