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
mod shelf;
mod store_views;
mod talker;
mod timings;
mod typed;
mod vault_sink;
mod virtual_voice;
mod work;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::CoreConfig;
use boardai_core::input::Input;
use boardai_core::protocol::UiCommand;
use tokio::task::JoinHandle;

pub use inbox::{
    InboxRefused, ProblemLink, SessionChannels, SessionClosed, SpeechInLink, SpeechOutLink,
    StoreLink,
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
pub use shelf::{MemoryShelf, ShelfError, SourceShelf, VaultShelf};
pub use talker::{
    TalkerSetupError, UnconfiguredTalker, scribe_from_settings, talker_config,
    talker_from_settings, talker_name,
};
pub use timings::estimate_timings;
pub use typed::say_typed;
pub use vault_sink::VaultSink;
pub use virtual_voice::{VirtualVoice, VirtualVoiceConfig};

use self::inbox::{Envelope, Inbox};
use self::orchestrator::{Orchestrator, OrchestratorParts, Receivers, SourceMessage};
use self::work::Work;
use crate::bridge::{BridgeHandle, UiSink};
use crate::cancel::CancelToken;
use crate::clock::TokioClock;
use crate::harness::HarnessPool;
use crate::limits::SESSION_CLOSE_TIMEOUT_MS;
use crate::llm::LlmProvider;
use crate::sources::{SourceDesk, SourceReader, ValidSource};
use crate::store::{SystemWallClock, WallClock};
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
    /// What sources need besides the tools and the harness (D-184).
    pub sources: SourceParts,
}

/// Sources' part of a session (D-184, D-185).
pub struct SourceParts {
    /// The home folder: paths are shown and written under it as `~/…`.
    pub home: PathBuf,
    /// The installed harness's name ("Claude Code"), `None` when `harness` is.
    pub harness_name: Option<String>,
    /// Where projects keep their sources and briefs.
    pub shelf: Arc<dyn SourceShelf>,
    /// The day a brief is written on.
    pub clock: Arc<dyn WallClock>,
    /// Document readers (T-031); none yet, so documents wait in the tray.
    pub readers: Vec<Arc<dyn SourceReader>>,
}

impl SourceParts {
    /// No harness, a shelf kept in memory, the system's clock: tests and the dev loops.
    #[must_use]
    pub fn in_memory(home: PathBuf) -> Self {
        Self {
            home,
            harness_name: None,
            shelf: Arc::new(MemoryShelf::default()),
            clock: Arc::new(SystemWallClock),
            readers: Vec::new(),
        }
    }
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
    parts.store.report_to(channels.store());
    let SessionChannels {
        inbox,
        receiver,
        positions,
        listening,
        ..
    } = channels;
    let closing = CancelToken::new();
    let base_roots: Vec<PathBuf> = parts
        .tools
        .roots()
        .folders()
        .map(std::path::Path::to_path_buf)
        .collect();
    let desk = SourceDesk::new(parts.sources.home, base_roots, parts.sources.harness_name);
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
            closing: closing.clone(),
            desk,
            shelf: parts.sources.shelf,
            wall: parts.sources.clock,
            readers: parts.sources.readers,
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

    /// A path the user shared natively (dropped on the window or chosen), already validated by
    /// the app (D-184). Never waits: a full inbox drops it, and the app says so.
    pub fn share(&self, source: ValidSource, keep: bool) -> Result<(), InboxRefused> {
        self.inbox
            .try_send_envelope(Envelope::Source(SourceMessage::Shared { source, keep }))
    }

    /// A drop or a choice the app refused (every path failed its checks): BoardAI says why.
    pub fn refuse(&self, detail: String) -> Result<(), InboxRefused> {
        self.inbox
            .try_send_envelope(Envelope::Source(SourceMessage::Refused { detail }))
    }

    /// Something is dragged over the window, or left it (`None`): the drop overlay (14b).
    pub fn drag(&self, drag: Option<boardai_core::sources::DragOver>) -> Result<(), InboxRefused> {
        self.inbox
            .try_send_envelope(Envelope::Source(SourceMessage::Dragging { drag }))
    }

    /// The link typed text or a speech-in actor sends utterances through.
    #[must_use]
    pub fn speech_in(&self) -> SpeechInLink {
        self.speech_in.clone()
    }

    /// Closes the session: cancels the talker, harness tasks, tools, timers and the voice,
    /// sends the last UI events and flushes the store, within `SESSION_CLOSE_TIMEOUT_MS`. Each
    /// step is bounded on its own; past the whole bound the session task is aborted, so
    /// quitting returns whatever is stuck.
    pub async fn close(mut self) -> SessionStats {
        self.closing.cancel();
        let bound = Duration::from_millis(SESSION_CLOSE_TIMEOUT_MS);
        match tokio::time::timeout(bound, &mut self.task).await {
            Ok(Ok(stats)) => stats,
            Ok(Err(error)) => {
                tracing::error!(%error, "the session task ended abnormally");
                SessionStats::default()
            }
            Err(_elapsed) => {
                tracing::error!(
                    "the session did not close within {SESSION_CLOSE_TIMEOUT_MS} ms; it was \
                     stopped"
                );
                self.task.abort();
                SessionStats::default()
            }
        }
    }
}

#[cfg(test)]
mod tests;
