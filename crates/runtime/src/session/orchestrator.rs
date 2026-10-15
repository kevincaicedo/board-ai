//! The orchestrator task (LLD §3.1): the one owner of [`Core`] (law 3). It steps the core with
//! each input at the clock's session time, then applies the effects in order.
//!
//! The loop serves, in this order: closing, the inbox, the latest playback position, the
//! earliest deadline, and reaping finished tasks. Answers it gives itself (a clause speech out
//! refused, a tool that can't run) are stepped before it waits again.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::effect::{Effect, LogRecord, Milestone};
use boardai_core::input::{Input, PlaybackPosition};
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::{Core, CoreConfig};
use tokio::sync::{mpsc, watch};

use super::deadlines::Deadlines;
use super::inbox::{Envelope, Inbox};
use super::ports::{SpeechOut, StoreSink};
use super::talker::Talker;
use super::work::Work;
use crate::bridge::BridgeHandle;
use crate::cancel::CancelToken;
use crate::clock::{Clock, TokioClock};
use crate::limits::SESSION_FLUSH_TIMEOUT_MS;
use crate::llm::LlmProvider;

/// Counters the session reports when it closes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub steps: u64,
    /// Inputs from cancelled talker streams, dropped before the core (D-034).
    pub stale_talker_inputs: u64,
}

pub(crate) struct Orchestrator {
    pub(crate) core: Core,
    pub(crate) clock: TokioClock,
    pub(crate) inbox: Inbox,
    pub(crate) talker: Talker,
    pub(crate) work: Work,
    pub(crate) speech: Box<dyn SpeechOut>,
    pub(crate) store: Arc<dyn StoreSink>,
    pub(crate) bridge: BridgeHandle,
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
            core: Core::new(config),
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
        self.start().await;
        let mut playing = true;
        loop {
            while let Some(input) = self.own_answers.pop_front() {
                self.step(input).await;
            }
            let deadline = self
                .deadlines
                .earliest()
                .map(|at| self.clock.instant_at(at));
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
                        if let Some(position) = latest {
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
                Some(()) = self.work.reap() => {}
            }
        }
        self.close().await
    }

    async fn start(&mut self) {
        let now = self.clock.now();
        let mut effects = std::mem::take(&mut self.effects);
        self.core.start(now, &mut effects);
        self.apply_all(now, &mut effects).await;
        self.effects = effects;
    }

    async fn receive(&mut self, envelope: Envelope) {
        match envelope {
            Envelope::Input(input) => self.step(input).await,
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
        }
    }

    /// Steps the core with one input at the current session time and applies its effects.
    pub(crate) async fn step(&mut self, input: Input) {
        let now = self.clock.now();
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

    /// Stops the talker, tasks, timers and voice, sends the last UI events, flushes the store.
    async fn close(mut self) -> SessionStats {
        self.talker.cancel();
        self.work.shutdown().await;
        self.deadlines.clear();
        self.speech.reset();
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
            stale_talker_inputs = self.stats.stale_talker_inputs,
            ui_batches = bridge.batches,
            ui_batches_lost = bridge.lost_batches,
            ui_events_coalesced = bridge.frame.coalesced,
            "session closed"
        );
        self.stats
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
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}
