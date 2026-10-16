//! The orchestrator task (LLD §3.1): the one owner of [`Core`] (law 3). It steps the core with
//! each input at the clock's session time, then applies the effects in order.
//!
//! The loop serves, in this order: closing, the inbox, the latest playback position, the
//! earliest deadline, the idle timeout, and reaping finished tasks. Answers it gives itself (a
//! clause speech out refused, a tool that can't run) are stepped before it waits again.
//!
//! It also runs the session's life (LLD §3.6): the store opens a session when the loop starts,
//! and the core starts with its context. After `SESSION_IDLE_TIMEOUT_MS` without input the
//! session ends (its digest is written in the background) and the loop rests; the next words
//! open a new session with a fresh core, and session time starts again from zero. Closing ends
//! the session within a bounded time, so quitting never hangs.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Effect, LogRecord, Milestone};
use boardai_core::input::{Input, PlaybackPosition};
use boardai_core::protocol::{MicAction, UiCommand, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::{Core, CoreConfig};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::deadlines::Deadlines;
use super::inbox::{Envelope, Inbox};
use super::listening::{Listening, ListeningPublisher};
use super::ports::{SessionClose, SessionOpened, SpeechOut, StoreSink};
use super::talker::Talker;
use super::work::Work;
use crate::bridge::BridgeHandle;
use crate::cancel::CancelToken;
use crate::clock::{Clock, TokioClock};
use crate::limits::{
    SCRIBE_DIGEST_DEADLINE_MS, SESSION_FINISH_CANCEL_WAIT_MS, SESSION_FINISH_WRITE_MS,
    SESSION_FLUSH_TIMEOUT_MS, SESSION_IDLE_TIMEOUT_MS, SESSION_QUIT_DIGEST_MS,
    SESSION_START_TIMEOUT_MS,
};
use crate::llm::LlmProvider;

/// Counters the session reports when it closes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub steps: u64,
    /// Inputs from cancelled talker streams, dropped before the core (D-034).
    pub stale_talker_inputs: u64,
    /// Sessions opened: the first, and one per return after an idle timeout.
    pub sessions: u64,
    /// Inputs that arrived while resting after an idle timeout and did not open a session
    /// (noise, playback reports, a pointer): dropped.
    pub resting_inputs: u64,
}

/// Where the session's life is.
enum Phase {
    /// A session is open. A session nobody spoke in leaves nothing in the vault.
    Active {
        spoke: bool,
        last_activity: SessionTime,
        store: StoreSession,
    },
    /// The last session ended after an idle timeout; the next words open a new one. A muted
    /// session stays muted: only an unmute opens the next.
    Resting {
        muted: bool,
        finishing: Option<Finishing>,
    },
}

/// Whether the store answered the session's start. Only a session it opened is ended there: a
/// store that answers late must never end (or forget) some other session in its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreSession {
    Opened,
    Unanswered,
}

/// A session's digest being written in the background.
struct Finishing {
    cancel: CancelToken,
    task: JoinHandle<()>,
}

pub(crate) struct Orchestrator {
    pub(crate) core: Core,
    /// Each session's core starts from this, plus the store's context.
    config: CoreConfig,
    phase: Phase,
    pub(crate) clock: TokioClock,
    pub(crate) inbox: Inbox,
    pub(crate) talker: Talker,
    pub(crate) work: Work,
    pub(crate) speech: Box<dyn SpeechOut>,
    pub(crate) store: Arc<dyn StoreSink>,
    pub(crate) bridge: BridgeHandle,
    pub(crate) listening: ListeningPublisher,
    /// The session's close, which the long waits of opening a session race against.
    closing: CancelToken,
    pub(crate) deadlines: Deadlines,
    pub(crate) own_answers: VecDeque<Input>,
    pub(crate) effects: Vec<Effect>,
    pub(crate) ui_events: Vec<UiEvent>,
    pub(crate) stats: SessionStats,
    /// Counts user turns, so latency marks of one turn can be grouped.
    pub(crate) turn: u64,
}

/// The receiving ends the loop waits on.
pub(crate) struct Receivers {
    pub(crate) inbox: mpsc::Receiver<Envelope>,
    pub(crate) positions: watch::Receiver<Option<PlaybackPosition>>,
    pub(crate) closing: CancelToken,
}

impl Orchestrator {
    pub(crate) fn new(config: CoreConfig, clock: TokioClock, parts: OrchestratorParts) -> Self {
        Self {
            listening: ListeningPublisher::new(config.mic, parts.listening),
            core: Core::new(config.clone()),
            config,
            // Until the store answers the first start.
            phase: Phase::Active {
                spoke: false,
                last_activity: SessionTime::ZERO,
                store: StoreSession::Unanswered,
            },
            closing: parts.closing,
            clock,
            inbox: parts.inbox,
            talker: Talker::new(parts.talker),
            work: parts.work,
            speech: parts.speech,
            store: parts.store,
            bridge: parts.bridge,
            deadlines: Deadlines::default(),
            own_answers: VecDeque::new(),
            effects: Vec::new(),
            ui_events: Vec::new(),
            stats: SessionStats::default(),
            turn: 0,
        }
    }

    /// Runs the session until it is closed, then stops everything it started.
    pub(crate) async fn run(mut self, mut receivers: Receivers) -> SessionStats {
        self.open_session().await;
        let mut playing = true;
        loop {
            while let Some(input) = self.own_answers.pop_front() {
                self.step(input).await;
            }
            let deadline = self
                .deadlines
                .earliest()
                .map(|at| self.clock.instant_at(at));
            let idle_at = self.idle_at();
            tokio::select! {
                biased;
                () = receivers.closing.cancelled() => break,
                envelope = receivers.inbox.recv() => match envelope {
                    Some(envelope) => self.receive(envelope).await,
                    None => break,
                },
                changed = receivers.positions.changed(), if playing => match changed {
                    Ok(()) => {
                        let latest = *receivers.positions.borrow_and_update();
                        if let Some(position) = latest
                            && matches!(self.phase, Phase::Active { .. })
                        {
                            self.step(Input::PlaybackAt { position }).await;
                        }
                    }
                    // Speech out is gone; nothing will play again.
                    Err(_) => playing = false,
                },
                () = sleep_until(deadline), if deadline.is_some() => {
                    if self.deadlines.take_due(self.clock.now()) {
                        self.step(Input::Tick).await;
                    }
                }
                () = sleep_until(idle_at), if idle_at.is_some() => self.on_idle().await,
                Some(()) = self.work.reap() => {}
            }
        }
        self.close().await
    }

    /// Opens a session in the store and starts a core for it. The first session uses the
    /// clock the session was spawned with; a later one restarts session time at zero.
    ///
    /// The store gets `SESSION_START_TIMEOUT_MS` to answer; past it the session opens without
    /// its context and unrecorded. A close that arrives meanwhile returns at once, with nothing
    /// opened: quitting never waits on a stalled vault.
    async fn open_session(&mut self) {
        let closing = self.closing.clone();
        let bound = Duration::from_millis(SESSION_START_TIMEOUT_MS);
        let answered = tokio::select! {
            biased;
            () = closing.cancelled() => return,
            answered = tokio::time::timeout(bound, self.store.start_session()) => answered,
        };
        let (opened, store) = match answered {
            Ok(opened) => (opened, StoreSession::Opened),
            Err(_elapsed) => {
                tracing::error!(
                    "the store did not open the session within {SESSION_START_TIMEOUT_MS} ms; it \
                     runs unrecorded"
                );
                (SessionOpened::default(), StoreSession::Unanswered)
            }
        };
        if self.stats.sessions > 0 {
            self.clock = TokioClock::start();
        }
        let mut config = self.config.clone();
        config.session_context = match (config.session_context.take(), opened.context) {
            (Some(own), Some(stored)) => Some(format!("{own}\n\n{stored}")),
            (own, stored) => own.or(stored),
        };
        config.suggestion = opened.suggestion.or(config.suggestion);
        self.core = Core::new(config);
        self.stats.sessions += 1;
        let now = self.clock.now();
        self.phase = Phase::Active {
            spoke: false,
            last_activity: now,
            store,
        };
        let start = UiEvent::SessionStart {
            session: opened.session,
        };
        self.store.ui_event(now, &start);
        self.ui_events.push(start);
        let mut effects = std::mem::take(&mut self.effects);
        self.core.start(now, &mut effects);
        self.apply_all(now, &mut effects).await;
        self.effects = effects;
    }

    async fn receive(&mut self, envelope: Envelope) {
        match envelope {
            Envelope::Input(input) => {
                if let Phase::Resting { muted, .. } = &self.phase {
                    if !opens_a_session(&input, *muted) {
                        self.stats.resting_inputs += 1;
                        return;
                    }
                    self.wake().await;
                    // Closing came first: no session opened for these words.
                    if matches!(self.phase, Phase::Resting { .. }) {
                        return;
                    }
                }
                self.step(input).await;
            }
            Envelope::Talker { generation, input } => {
                if !self.talker.accepts(generation) {
                    self.stats.stale_talker_inputs += 1;
                    return;
                }
                if matches!(input, Input::TalkerDone { .. }) {
                    self.talker.finished(generation);
                }
                self.step(input).await;
            }
            Envelope::Problem { spoken, detail } => {
                let now = self.clock.now();
                let problem = UiEvent::Problem { spoken, detail };
                self.store.ui_event(now, &problem);
                if self.bridge.send(now, vec![problem]).await.is_err() {
                    tracing::error!("the UI bridge has stopped; UI events are lost");
                }
            }
        }
    }

    /// Steps the core with one input at the current session time and applies its effects.
    pub(crate) async fn step(&mut self, input: Input) {
        let now = self.clock.now();
        self.note_activity(now, &input);
        self.stats.steps += 1;
        self.store.input(now, &input);
        let mut effects = std::mem::take(&mut self.effects);
        self.core.step(now, input, &mut effects);
        self.apply_all(now, &mut effects).await;
        self.effects = effects;
    }

    async fn apply_all(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        for effect in effects.drain(..) {
            self.apply(now, effect);
        }
        let events = std::mem::take(&mut self.ui_events);
        self.listening.after_step(self.core.state(), &events);
        if self.bridge.send(now, events).await.is_err() {
            tracing::error!("the UI bridge has stopped; UI events are lost");
        }
    }

    /// One effect, in the order the core gave it (LLD §2.1).
    fn apply(&mut self, now: SessionTime, effect: Effect) {
        match effect {
            Effect::Duck { gain } => self.speech.duck(gain),
            Effect::StopVoice => {
                if let Err(error) = self.speech.stop() {
                    tracing::warn!(%error, "the voice did not take StopVoice; answering it here");
                    self.own_answers
                        .push_back(Input::VoiceStopped { last: None });
                }
            }
            Effect::Synthesize { clause, text } => {
                if let Err(error) = self.speech.synthesize(clause, text) {
                    tracing::warn!(%error, clause = clause.0, "the voice refused a clause");
                    self.own_answers
                        .push_back(Input::SynthesisFailed { clause });
                }
            }
            Effect::Talk { request } => self.talker.talk(request, &self.inbox),
            Effect::CancelTalk => self.talker.cancel(),
            Effect::RunTool { id, call } => {
                let answer = self.work.run_tool(id, call, &self.store, &self.inbox);
                self.own_answers.extend(answer);
            }
            Effect::RunHarness { task, spec } => {
                let answer = self.work.run_harness(task, spec, &self.inbox);
                self.own_answers.extend(answer);
            }
            Effect::CancelHarness { task } => self.work.cancel_harness(task),
            Effect::Ui(event) => {
                self.store.ui_event(now, &event);
                self.ui_events.push(event);
            }
            Effect::Log(record) => {
                self.trace(now, &record);
                self.store.log(now, &record);
            }
            Effect::Remember(memory) => self.store.remember(now, &memory),
            Effect::WakeAt { at } => self.deadlines.insert(at),
            Effect::Forget { scope } => self.store.forget(now, scope),
            Effect::AcceptProposal { capture } => self.store.accept_proposal(now, capture),
        }
    }

    /// Latency marks and drops as tracing events, stamped with session time (LLD §8).
    fn trace(&mut self, now: SessionTime, record: &LogRecord) {
        match record {
            LogRecord::Milestone { milestone } => {
                if *milestone == Milestone::VoiceStart {
                    self.turn += 1;
                }
                tracing::info!(
                    target: "boardai::latency",
                    at_ms = now.as_ms(),
                    turn = self.turn,
                    milestone = ?milestone,
                    "mark"
                );
            }
            LogRecord::Dropped { what, count } => {
                tracing::debug!(at_ms = now.as_ms(), what = ?what, count, "dropped at a limit");
            }
        }
    }

    /// Stops the talker, tasks, timers and voice, sends the last UI events, ends the session
    /// (its digest within `SESSION_QUIT_DIGEST_MS`), flushes the store.
    async fn close(mut self) -> SessionStats {
        self.quiet_down().await;
        self.end_for_good().await;
        let bridge = self.bridge.close().await;
        let flush = Duration::from_millis(SESSION_FLUSH_TIMEOUT_MS);
        if tokio::time::timeout(flush, self.store.flush())
            .await
            .is_err()
        {
            tracing::warn!("the store did not flush within {SESSION_FLUSH_TIMEOUT_MS} ms");
        }
        tracing::info!(
            steps = self.stats.steps,
            sessions = self.stats.sessions,
            resting_inputs = self.stats.resting_inputs,
            stale_talker_inputs = self.stats.stale_talker_inputs,
            ui_batches = bridge.batches,
            ui_batches_lost = bridge.lost_batches,
            ui_events_coalesced = bridge.frame.coalesced,
            "session closed"
        );
        self.stats
    }
}

/// The session's life: activity, the idle timeout, resting, waking, and its end.
impl Orchestrator {
    /// Every input but a timer tick, a playback report or a bare voice onset is activity; a final
    /// transcript with words means the user spoke, so the session is worth a digest.
    fn note_activity(&mut self, now: SessionTime, input: &Input) {
        let Phase::Active {
            spoke,
            last_activity,
            ..
        } = &mut self.phase
        else {
            return;
        };
        // A cough or a click is not someone using BoardAI; their speech (`VoiceStart`) is.
        if matches!(
            input,
            Input::Tick | Input::PlaybackAt { .. } | Input::VoiceOnset
        ) {
            return;
        }
        *last_activity = now;
        if let Input::Transcript {
            text,
            is_final: true,
        } = input
            && !text.trim().is_empty()
        {
            *spoke = true;
        }
    }

    fn idle_at(&self) -> Option<tokio::time::Instant> {
        let Phase::Active { last_activity, .. } = &self.phase else {
            return None;
        };
        let at = last_activity.plus_ms(SESSION_IDLE_TIMEOUT_MS);
        Some(self.clock.instant_at(at))
    }

    /// No input for `SESSION_IDLE_TIMEOUT_MS`: the session ends unless BoardAI is still busy.
    async fn on_idle(&mut self) {
        let now = self.clock.now();
        let state = self.core.state().clone();
        let Phase::Active {
            spoke,
            last_activity,
            store,
        } = &mut self.phase
        else {
            return;
        };
        let muted = match state {
            ConversationState::Idle => false,
            ConversationState::Muted => true,
            ConversationState::Listening(_)
            | ConversationState::Thinking
            | ConversationState::Speaking
            | ConversationState::Ducked(_) => {
                *last_activity = now;
                return;
            }
        };
        let (spoke, store) = (*spoke, *store);
        self.quiet_down().await;
        tracing::info!(muted, "the session ended after its idle timeout");
        let finishing = match store {
            StoreSession::Opened => {
                let cancel = CancelToken::new();
                let deadline = Duration::from_millis(SCRIBE_DIGEST_DEADLINE_MS);
                let close = self.close_for(spoke, now, deadline, cancel.clone());
                let task = tokio::spawn(self.store.finish_session(close));
                Some(Finishing { cancel, task })
            }
            StoreSession::Unanswered => None,
        };
        self.phase = Phase::Resting { muted, finishing };
    }

    /// The first words after resting: the last digest gets a moment (its scribe is cancelled,
    /// so the minimal digest is written), then a new session opens. A close meanwhile ends the
    /// wait at once, and no session opens.
    async fn wake(&mut self) {
        // Taken out, so a handle that has finished is never awaited again.
        let finishing = match &mut self.phase {
            Phase::Resting { finishing, .. } => finishing.take(),
            Phase::Active { .. } => None,
        };
        if let Some(mut finishing) = finishing {
            finishing.cancel.cancel();
            let wait = Duration::from_millis(SESSION_FINISH_CANCEL_WAIT_MS);
            let closing = self.closing.clone();
            tokio::select! {
                biased;
                () = closing.cancelled() => return,
                waited = tokio::time::timeout(wait, &mut finishing.task) => {
                    if waited.is_err() {
                        tracing::warn!("the last session's digest is still being written");
                    }
                }
            }
        }
        self.open_session().await;
    }

    /// The app is closing: the open session ends within `SESSION_QUIT_DIGEST_MS` plus the time
    /// to write its files; a digest still being written after an idle timeout gets as long.
    async fn end_for_good(&mut self) {
        let resting = Phase::Resting {
            muted: false,
            finishing: None,
        };
        let deadline = Duration::from_millis(SESSION_QUIT_DIGEST_MS);
        let bound = deadline + Duration::from_millis(SESSION_FINISH_WRITE_MS);
        match std::mem::replace(&mut self.phase, resting) {
            Phase::Active {
                store: StoreSession::Unanswered,
                ..
            } => tracing::warn!("the store never opened this session; it is not ended there"),
            Phase::Active {
                spoke,
                store: StoreSession::Opened,
                ..
            } => {
                let close = self.close_for(spoke, self.clock.now(), deadline, CancelToken::new());
                if tokio::time::timeout(bound, self.store.finish_session(close))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "the session did not finish in time; it gets the minimal digest"
                    );
                }
            }
            Phase::Resting {
                finishing: Some(finishing),
                ..
            } => {
                let cancel = finishing.cancel.clone();
                let quit = tokio::spawn(async move {
                    tokio::time::sleep(deadline).await;
                    cancel.cancel();
                });
                if tokio::time::timeout(bound, finishing.task).await.is_err() {
                    tracing::warn!("the last session's digest did not finish in time");
                }
                quit.abort();
            }
            Phase::Resting {
                finishing: None, ..
            } => {}
        }
    }

    /// How the session ends: nothing kept when nobody spoke, else its digest and board.
    fn close_for(
        &self,
        spoke: bool,
        t_end: SessionTime,
        deadline: Duration,
        cancel: CancelToken,
    ) -> SessionClose {
        if !spoke {
            return SessionClose::Discard;
        }
        let drew = self
            .core
            .board()
            .layers()
            .iter()
            .any(|layer| !layer.elements().is_empty());
        SessionClose::Digest {
            t_end,
            board: drew.then(|| self.core.export_session_excalidraw()),
            deadline,
            cancel,
        }
    }

    /// Stops what the session started: the talker, tools and harness tasks, timers, the voice.
    async fn quiet_down(&mut self) {
        self.talker.cancel();
        self.work.shutdown().await;
        self.deadlines.clear();
        self.speech.reset();
    }
}

/// Whether an input that arrives while resting opens a new session: the user's own voice or
/// typed words, or the push-to-talk key. While muted, only an unmute does, so noise never
/// starts a session nobody asked for.
fn opens_a_session(input: &Input, muted: bool) -> bool {
    match input {
        Input::VoiceStart | Input::Transcript { .. } => !muted,
        Input::Command(UiCommand::Mic { action }) => match action {
            MicAction::TalkDown => !muted,
            MicAction::Unmute => muted,
            MicAction::Mute | MicAction::TalkUp => false,
        },
        Input::Command(
            UiCommand::Pointer { .. }
            | UiCommand::CloseView
            | UiCommand::Stop
            | UiCommand::OpenSource { .. },
        )
        | Input::VoiceEnd { .. }
        | Input::TalkerDelta { .. }
        | Input::TalkerToolCall { .. }
        | Input::TalkerDone { .. }
        | Input::ClauseReady { .. }
        | Input::SynthesisFailed { .. }
        | Input::PlaybackAt { .. }
        | Input::PlaybackDrained
        | Input::VoiceStopped { .. }
        | Input::ToolResult { .. }
        | Input::Harness { .. }
        | Input::Tick
        | Input::MemoryFailed { .. }
        // An onset alone may be a cough; the session opens on the `VoiceStart` that confirms it.
        | Input::VoiceOnset => false,
    }
}

/// What the orchestrator drives, besides the core.
pub(crate) struct OrchestratorParts {
    pub(crate) inbox: Inbox,
    pub(crate) talker: Arc<dyn LlmProvider>,
    pub(crate) work: Work,
    pub(crate) speech: Box<dyn SpeechOut>,
    pub(crate) store: Arc<dyn StoreSink>,
    pub(crate) bridge: BridgeHandle,
    pub(crate) listening: Arc<watch::Sender<Listening>>,
    pub(crate) closing: CancelToken,
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}
