//! The run replayed: every recorded input stepped through a fresh core at its time. The core is
//! deterministic, so this rebuilds the session exactly, and it exposes what the record alone
//! does not: each op's anchor (I1, against the positions the real voice reported), when each
//! clause was sent to the voice, and the conversation history.

use std::collections::{BTreeMap, BTreeSet};

use boardai_core::effect::Effect;
use boardai_core::history::Message;
use boardai_core::ids::{ClauseId, OpId};
use boardai_core::input::{Input, PlaybackPosition, WordTimings};
use boardai_core::protocol::{DrawOp, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::{Core, CoreConfig};

use crate::trace::Entry;

pub type Record = [(SessionTime, Entry)];

pub fn inputs(record: &Record) -> impl Iterator<Item = (SessionTime, &Input)> {
    record.iter().filter_map(|(at, entry)| match entry {
        Entry::Input { input } => Some((*at, input)),
        Entry::Ui { .. } | Entry::Record { .. } => None,
    })
}

/// Playback as the real voice reported it, with the core's rule for "this word has played".
#[derive(Default)]
struct Heard {
    timings: BTreeMap<ClauseId, WordTimings>,
    position: Option<PlaybackPosition>,
    played: BTreeSet<ClauseId>,
    /// When each clause's audio began, from its first position.
    started: BTreeMap<ClauseId, SessionTime>,
}

impl Heard {
    fn observe(&mut self, at: SessionTime, input: &Input) {
        match input {
            Input::ClauseReady { clause, timings } => {
                self.timings.insert(*clause, timings.clone());
            }
            Input::PlaybackAt { position } => self.advance(at, *position),
            Input::VoiceStopped {
                last: Some(position),
            } => self.advance(at, *position),
            Input::PlaybackDrained => self.played.extend(self.timings.keys().copied()),
            Input::VoiceStopped { last: None }
            | Input::VoiceOnset
            | Input::VoiceStart
            | Input::VoiceEnd { .. }
            | Input::Transcript { .. }
            | Input::TalkerDelta { .. }
            | Input::TalkerToolCall { .. }
            | Input::TalkerDone { .. }
            | Input::SynthesisFailed { .. }
            | Input::ToolResult { .. }
            | Input::Harness { .. }
            | Input::Command(_)
            | Input::Tick => {}
        }
    }

    fn advance(&mut self, at: SessionTime, position: PlaybackPosition) {
        let key = (position.clause, position.offset_ms);
        if self
            .position
            .is_some_and(|heard| key < (heard.clause, heard.offset_ms))
        {
            return;
        }
        self.position = Some(position);
        let earlier: Vec<ClauseId> = self
            .timings
            .range(..position.clause)
            .map(|(id, _)| *id)
            .collect();
        self.played.extend(earlier);
        self.started.entry(position.clause).or_insert_with(|| {
            SessionTime::from_ms(at.as_ms().saturating_sub(u64::from(position.offset_ms)))
        });
    }

    fn reached(&self, clause: ClauseId, offset: usize) -> bool {
        if self.played.contains(&clause) {
            return true;
        }
        let (Some(position), Some(timings)) = (self.position, self.timings.get(&clause)) else {
            return false;
        };
        if position.clause != clause {
            return position.clause > clause;
        }
        position.offset_ms >= word_start_ms(timings, offset)
    }

    /// When the anchor's word began playing, if its clause was seen playing.
    fn word_played_at(&self, clause: ClauseId, offset: usize) -> Option<SessionTime> {
        let started = self.started.get(&clause)?;
        let timings = self.timings.get(&clause)?;
        Some(started.plus_ms(u64::from(word_start_ms(timings, offset))))
    }
}

/// When the first word at or after byte `offset` starts, sanitized as the core does (sorted,
/// never going backwards).
fn word_start_ms(timings: &WordTimings, offset: usize) -> u32 {
    let mut words = timings.words.clone();
    words.sort_by_key(|word| word.byte_offset);
    let mut latest = 0;
    for word in words {
        latest = latest.max(word.start_ms).min(timings.duration_ms);
        if usize::try_from(word.byte_offset).is_ok_and(|byte| byte >= offset) {
            return latest;
        }
    }
    timings.duration_ms
}

#[derive(Default)]
pub struct Replay {
    pub ui: Vec<UiEvent>,
    pub conversation: Vec<Message>,
    pub violations: Vec<String>,
    pub ops_checked: usize,
    pub ops_unanchored: usize,
    /// For each op checked: milliseconds from its word playing to its stroke starting.
    pub stroke_lags_ms: Vec<i64>,
    /// When each clause was sent to the voice.
    pub synthesized: BTreeMap<ClauseId, SessionTime>,
    /// Sessions the record opened (more than one when an idle rest happened).
    pub sessions: usize,
}

impl Replay {
    /// Steps every recorded input through a core. Each `SessionStart` in the record (the
    /// orchestrator opens a session at start and after every idle rest) starts a fresh core,
    /// as the orchestrator does; the e2e's store gives every session the same config.
    pub fn run(record: &Record, config: &CoreConfig) -> Self {
        let mut core = Core::new(config.clone());
        let mut effects = Vec::new();
        let mut replay = Self::default();
        let mut heard = Heard::default();
        for (at, entry) in record {
            match entry {
                Entry::Ui {
                    event: UiEvent::SessionStart { .. },
                } => {
                    core = Core::new(config.clone());
                    heard = Heard::default();
                    replay.sessions += 1;
                    core.start(*at, &mut effects);
                }
                Entry::Input { input } => {
                    heard.observe(*at, input);
                    core.step(*at, input.clone(), &mut effects);
                }
                Entry::Ui { .. } | Entry::Record { .. } => continue,
            }
            replay.collect(*at, &mut effects, &core, &heard);
        }
        // The last session's history: a run short of the idle timeout has only one.
        replay.conversation = core.conversation().messages().to_vec();
        replay
    }

    fn collect(&mut self, at: SessionTime, effects: &mut Vec<Effect>, core: &Core, heard: &Heard) {
        for effect in effects.drain(..) {
            let event = match effect {
                Effect::Ui(event) => event,
                Effect::Synthesize { clause, .. } => {
                    self.synthesized.insert(clause, at);
                    continue;
                }
                Effect::Duck { .. }
                | Effect::StopVoice
                | Effect::Talk { .. }
                | Effect::CancelTalk
                | Effect::RunTool { .. }
                | Effect::RunHarness { .. }
                | Effect::CancelHarness { .. }
                | Effect::Log(_)
                | Effect::Remember(_)
                | Effect::WakeAt { .. }
                | Effect::Forget { .. }
                | Effect::AcceptProposal { .. } => continue,
            };
            if let UiEvent::OpStart { op, .. } = &event {
                self.check_anchor(at, op_id(op), core, heard);
            }
            self.ui.push(event);
        }
    }

    /// I1 on the real voice's positions, and how long after its word each stroke began.
    fn check_anchor(&mut self, at: SessionTime, op: OpId, core: &Core, heard: &Heard) {
        let Some((clause, offset)) = core.op_anchor(op) else {
            // The reply ended in the same step; its anchors are already forgotten.
            self.ops_unanchored += 1;
            return;
        };
        if !heard.reached(clause, offset) {
            self.violations.push(format!(
                "{} ms: op {} started before clause {} offset {offset} played",
                at.as_ms(),
                op.0,
                clause.0
            ));
            return;
        }
        self.ops_checked += 1;
        if let Some(word_at) = heard.word_played_at(clause, offset) {
            self.stroke_lags_ms
                .push(at.as_ms() as i64 - word_at.as_ms() as i64);
        }
    }
}

fn op_id(op: &DrawOp) -> OpId {
    match op {
        DrawOp::Node { op, .. }
        | DrawOp::Edge { op, .. }
        | DrawOp::Annotate { op, .. }
        | DrawOp::Strike { op, .. }
        | DrawOp::Trace { op, .. } => *op,
    }
}
