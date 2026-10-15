//! A conversation session (LLD §3.1): the orchestrator task that owns the core, and the seams
//! the other actors plug into.
//!
//! Start one in three moves: make the [`SessionChannels`], hand each actor its link (speech in
//! gets a [`SpeechInLink`], speech out a [`SpeechOutLink`]), then [`spawn`] the session with the
//! parts. Until the speech stream lands, [`VirtualVoice`] is the speech out and typed text
//! ([`say_typed`]) is the speech in; until the store lands, [`MemoryStore`] is the store.
//!
//! The runtime's obligations to the core (LLD §2.1) are met here: inputs of a cancelled talker
//! stream never reach it, every `Synthesize` is answered, every `WakeAt` becomes a `Tick`, and
//! every `StopVoice` is answered with `VoiceStopped`.

mod deadlines;
mod inbox;
mod memory_store;
mod orchestrator;
mod ports;
mod script;
mod talker;
mod timings;
mod typed;
mod virtual_voice;
mod work;

use std::sync::Arc;

use boardai_core::CoreConfig;
use boardai_core::input::Input;
use boardai_core::protocol::UiCommand;
use tokio::task::JoinHandle;

pub use inbox::{InboxRefused, SessionChannels, SessionClosed, SpeechInLink, SpeechOutLink};
pub use memory_store::MemoryStore;
pub use orchestrator::SessionStats;
pub use ports::{SpeechOut, SpeechOutError, StoreSink};
pub use script::{
    Cue, Observed, Progress, Script, ScriptError, ScriptLine, run_script, wait_for_answer,
};
pub use talker::{
    TalkerSetupError, UnconfiguredTalker, talker_config, talker_from_settings, talker_name,
};
pub use timings::estimate_timings;
pub use typed::say_typed;
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
