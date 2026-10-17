//! Invariants checked on every simulator step (LLD §2.9). A violation is recorded with the time
//! it happened; scenarios and the fuzzer assert there are none.

use std::collections::{BTreeMap, BTreeSet};

use crate::conversation::{ConversationState, cues};
use crate::effect::{Effect, ForgetScope, Gain};
use crate::engine::Core;
use crate::engine::text::change_question;
use crate::geometry::Point;
use crate::geometry::Rect;
use crate::harness::HarnessKind;
use crate::history::{AssistantPart, Message, UserPart};
use crate::ids::{CaptureId, ClauseId, ElementId, LayerId, OpId};
use crate::input::Input;
use crate::limits::{BARGE_IN_DECISION_MS, DETOUR_DEPTH_MAX, VOICE_ONSET_DUCK_HOLD_MS};
use crate::memory::{MemoryAction, MemoryWrite};
use crate::narration::speakable::Speakable;
use crate::protocol::{DrawOp, EdgePath, NodeShape, UiEvent};
use crate::sim::speech::FakeSpeech;
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Checker {
    violations: Vec<String>,
    talk_in_flight: bool,
    history: Vec<Message>,
    boxes: BTreeMap<(LayerId, ElementId), (Rect, NodeShape)>,
    paths: BTreeMap<(LayerId, ElementId), EdgePath>,
    /// Each participant's head and where its lifeline ends, as drawn so far.
    lifelines: BTreeMap<(LayerId, ElementId), (Rect, i32)>,
    /// Each message's arrow, and each cell's box.
    arrows: BTreeMap<(LayerId, ElementId), Vec<Point>>,
    cells: BTreeMap<(LayerId, ElementId), Rect>,
    started_ops: BTreeSet<OpId>,
    layer_pushes: usize,
    layer_pops: usize,
    /// The text sent to speech for every clause, so a change can be traced to the question
    /// BoardAI asked about it (I9).
    synthesized: BTreeMap<ClauseId, String>,
    /// Memories proposed and not yet kept, by capture, with when they were proposed.
    proposals: BTreeMap<CaptureId, SessionTime>,
    /// The user said "don't remember this session".
    off_record: bool,
    /// The user's final words since the last talker request, with when each utterance began:
    /// a gate is answered only by words begun after it opened (I9).
    user_finals: Vec<(SessionTime, String)>,
    /// Where `user_finals` stood when the voice ducked: a backchannel decided after that was
    /// never an utterance (LLD §2.2), so its words are taken back out.
    ducked_at: Option<usize>,
    /// Windows in which a backchannel was said over BoardAI's voice: no `StopVoice` allowed.
    backchannels: Vec<(SessionTime, SessionTime)>,
    /// The last camera frame sent for each layer.
    frames: BTreeMap<LayerId, Rect>,
    /// The latest voice onset: the one thing that may keep the voice ducked while it speaks.
    last_onset: Option<SessionTime>,
}

impl Checker {
    #[must_use]
    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    fn violate(&mut self, now: SessionTime, what: String) {
        self.violations.push(format!("{} ms: {what}", now.as_ms()));
    }

    /// A backchannel was scripted at `start`; I7 says no `StopVoice` may follow it while it is
    /// decided (by its final, or the window's end). Past that, a stop belongs to whatever the
    /// user says next.
    pub fn expect_backchannel(&mut self, start: SessionTime, core: &Core) {
        if *core.state() == ConversationState::Speaking {
            let end = start.plus_ms(BARGE_IN_DECISION_MS);
            self.backchannels.push((start, end));
        }
    }

    /// The voice is never left ducked while BoardAI speaks: only an onset's hold may keep it
    /// down outside a barge-in (LLD §2.2), and a cough must not leave it quiet. While a barge-in
    /// is decided it stays ducked.
    pub fn check_gain(&mut self, now: SessionTime, core: &Core, speech: &FakeSpeech) {
        // Inclusive: the tick that ends the hold may be stepped after other inputs of its time.
        let held = self
            .last_onset
            .is_some_and(|onset| now <= onset.plus_ms(VOICE_ONSET_DUCK_HOLD_MS));
        let gain = speech.gain();
        match core.state() {
            ConversationState::Speaking if gain < 1.0 && !held => {
                self.violate(
                    now,
                    format!("the voice was left ducked at {gain} while speaking"),
                );
            }
            ConversationState::Ducked(_) if gain >= 1.0 => {
                self.violate(
                    now,
                    "the voice came back up while a barge-in was decided".into(),
                );
            }
            ConversationState::Idle
            | ConversationState::Listening(_)
            | ConversationState::Thinking
            | ConversationState::Speaking
            | ConversationState::Ducked(_)
            | ConversationState::Muted => {}
        }
    }

    /// Called before the core steps with `input`.
    pub fn on_input(&mut self, now: SessionTime, input: &Input, core: &Core) {
        match input {
            Input::Transcript {
                text,
                is_final: true,
            } => {
                // Only words the core was listening to count; speech while muted or ignored by
                // push-to-talk was never an utterance.
                let began = match core.state() {
                    ConversationState::Listening(utterance) => Some(utterance.started_at()),
                    ConversationState::Ducked(barge_in) => Some(barge_in.utterance().started_at()),
                    ConversationState::Idle
                    | ConversationState::Thinking
                    | ConversationState::Speaking
                    | ConversationState::Muted => None,
                };
                if let Some(began) = began {
                    self.user_finals.push((began, text.clone()));
                }
            }
            Input::TalkerDone { .. } => self.talk_in_flight = false,
            Input::VoiceOnset => self.last_onset = Some(now),
            Input::TalkerToolCall { .. }
            | Input::VoiceStart
            | Input::VoiceEnd { .. }
            | Input::Transcript { .. }
            | Input::TalkerDelta { .. }
            | Input::ClauseReady { .. }
            | Input::SynthesisFailed { .. }
            | Input::PlaybackAt { .. }
            | Input::PlaybackDrained
            | Input::VoiceStopped { .. }
            | Input::ToolResult { .. }
            | Input::Harness { .. }
            | Input::Command(_)
            | Input::Tick
            | Input::MemoryFailed { .. } => {}
        }
    }

    pub fn on_effect(
        &mut self,
        now: SessionTime,
        effect: &Effect,
        core: &Core,
        speech: &FakeSpeech,
    ) {
        match effect {
            // I5: one talker stream at a time; CancelTalk precedes a new Talk.
            Effect::Talk { .. } => {
                if self.talk_in_flight {
                    self.violate(now, "I5: Talk while another stream is in flight".into());
                }
                self.talk_in_flight = true;
                self.user_finals.clear();
            }
            Effect::CancelTalk => self.talk_in_flight = false,
            // The duck ends (gain back to full) just before the backchannel's acknowledgement,
            // so only a new duck moves the mark; an interruption clears it.
            Effect::Duck { gain } => {
                if *gain == Gain::DUCKED {
                    self.ducked_at = Some(self.user_finals.len());
                }
            }
            Effect::StopVoice => {
                self.ducked_at = None;
                if self
                    .backchannels
                    .iter()
                    .any(|(start, end)| *start <= now && now <= *end)
                {
                    self.violate(now, "I7: a backchannel stopped the voice".into());
                }
            }
            Effect::Synthesize { clause, text } => {
                self.synthesized.insert(*clause, text.clone());
            }
            Effect::RunHarness { spec, .. } => {
                if let HarnessKind::Change { branch } = &spec.kind {
                    self.check_change(now, branch, &spec.brief, speech);
                }
            }
            Effect::Ui(UiEvent::Acknowledge) => {
                if let Some(length) = self.ducked_at.take() {
                    self.user_finals.truncate(length);
                }
            }
            Effect::Ui(event) => self.on_ui(now, event, core, speech),
            Effect::Remember(write) => self.on_remember(now, write),
            Effect::AcceptProposal { capture } => self.on_accept(now, *capture),
            Effect::Forget { scope } => {
                if *scope == ForgetScope::Session {
                    self.off_record = true;
                    self.proposals.clear();
                }
            }
            Effect::RunTool { .. }
            | Effect::CancelHarness { .. }
            | Effect::Log(_)
            | Effect::WakeAt { .. } => {}
        }
    }

    /// A session ends only once BoardAI is quiet (D-181): no talker stream in flight, nothing
    /// playing or waiting to play, and nobody holding the floor. Anything still going would be
    /// cut off when the orchestrator ends the session.
    fn check_session_end(&mut self, now: SessionTime, core: &Core, speech: &FakeSpeech) {
        let quiet =
            !self.talk_in_flight && !speech.is_active() && *core.state() == ConversationState::Idle;
        if !quiet {
            self.violate(
                now,
                format!(
                    "SessionEnd while BoardAI is not quiet (stream in flight: {}, speech \
                     active: {}, state: {})",
                    self.talk_in_flight,
                    speech.is_active(),
                    core.state().name()
                ),
            );
        }
    }

    /// I9: a change runs only on a yes begun after BoardAI's own question naming exactly that
    /// branch and instruction played to its end.
    fn check_change(&mut self, now: SessionTime, branch: &str, brief: &str, speech: &FakeSpeech) {
        let question = Speakable::from_caption(&change_question(branch, brief));
        let heard_at = self
            .synthesized
            .iter()
            .filter(|(_, text)| text.as_str() == question.text())
            .filter_map(|(clause, _)| speech.finished_at(*clause))
            .filter(|at| *at <= now)
            .max();
        let yes = heard_at.is_some_and(|at| cues::is_affirmative(&self.finals_since(at)));
        if !yes {
            let finals = self.finals_since(SessionTime::ZERO);
            self.violate(
                now,
                format!(
                    "I9: change {brief:?} on {branch} ran without a yes to BoardAI's question \
                     about it (question heard at {heard_at:?}, user said {finals:?})"
                ),
            );
        }
    }

    /// I10 (core side): after "don't remember this session", nothing goes to the store.
    fn on_remember(&mut self, now: SessionTime, write: &MemoryWrite) {
        if self.off_record {
            let text = write.text.clone();
            self.violate(
                now,
                format!("I10: remembered {text:?} after going off the record"),
            );
        }
        if write.action == MemoryAction::Propose {
            self.proposals.insert(write.capture, now);
        }
    }

    /// The proposal gate (I9's rule for memory): a proposal is kept only after the user's own
    /// words said to keep it.
    fn on_accept(&mut self, now: SessionTime, capture: CaptureId) {
        if self.off_record {
            self.violate(
                now,
                "I10: a proposal was kept after going off the record".into(),
            );
        }
        let keep = self
            .proposals
            .remove(&capture)
            .is_some_and(|at| cues::is_keep(&self.finals_since(at)));
        if !keep {
            let finals = self.finals_since(SessionTime::ZERO);
            self.violate(
                now,
                format!(
                    "I9: proposal {} kept without the user's keep (heard {finals:?})",
                    capture.0
                ),
            );
        }
    }

    /// The user's words since the last talker request, from utterances begun at or after `at`.
    fn finals_since(&self, at: SessionTime) -> String {
        self.user_finals
            .iter()
            .filter(|(began, _)| *began >= at)
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn on_ui(&mut self, now: SessionTime, event: &UiEvent, core: &Core, speech: &FakeSpeech) {
        match event {
            UiEvent::OpStart { layer, op, .. } => {
                self.check_anchor(now, op, core, speech);
                self.check_stable(now, *layer, op);
                self.check_framed(now, *layer, op);
            }
            UiEvent::Camera { layer, frame, .. } => {
                self.frames.insert(*layer, *frame);
            }
            UiEvent::LayerPush { .. } => self.layer_pushes += 1,
            UiEvent::LayerPop { .. } => self.layer_pops += 1,
            UiEvent::SessionEnd => self.check_session_end(now, core, speech),
            UiEvent::Thread { detours, .. } => {
                if detours.len() > DETOUR_DEPTH_MAX {
                    self.violate(now, format!("I4: {} detours open", detours.len()));
                }
            }
            UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Caption { .. }
            | UiEvent::Freeze { .. }
            | UiEvent::Resume { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. }
            | UiEvent::Suggestion { .. }
            | UiEvent::SessionStart { .. }
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings => {}
        }
    }

    /// I11: a stroke starts inside its layer's camera frame, so the user sees it drawn.
    fn check_framed(&mut self, now: SessionTime, layer: LayerId, op: &DrawOp) {
        let bounds = crate::board::layout::op_bounds(op);
        let framed = self
            .frames
            .get(&layer)
            .is_some_and(|frame| frame.contains(bounds));
        if !framed {
            let id = op_id(op).0;
            self.violate(
                now,
                format!(
                    "I11: op {id} on layer {} starts outside the camera frame",
                    layer.0
                ),
            );
        }
    }

    /// I1: an op never starts before its anchor word's audio has played.
    fn check_anchor(&mut self, now: SessionTime, op: &DrawOp, core: &Core, speech: &FakeSpeech) {
        let id = op_id(op);
        if !self.started_ops.insert(id) {
            return;
        }
        let Some((clause, offset)) = core.op_anchor(id) else {
            return;
        };
        match speech.audio_time(clause, offset) {
            None => self.violate(
                now,
                format!("I1: op {} drawn but its clause never played", id.0),
            ),
            Some(played_at) if played_at > now => self.violate(
                now,
                format!(
                    "I1: op {} started {} ms before its word",
                    id.0,
                    played_at.ms_since(now)
                ),
            ),
            Some(_) => {}
        }
    }

    /// I6: an element's geometry never changes; a group's box only grows.
    fn check_stable(&mut self, now: SessionTime, layer: LayerId, op: &DrawOp) {
        match op {
            DrawOp::Node {
                id, shape, rect, ..
            } => {
                let key = (layer, id.clone());
                if let Some((old, old_shape)) = self.boxes.get(&key).copied() {
                    let grown = *shape == NodeShape::Group && old.union(*rect) == *rect;
                    if old != *rect && !grown {
                        self.violate(
                            now,
                            format!("I6: {} moved from {old:?} to {rect:?}", id.as_str()),
                        );
                    }
                    if old_shape != *shape {
                        self.violate(now, format!("I6: {} changed shape", id.as_str()));
                    }
                }
                self.boxes.insert(key, (*rect, *shape));
            }
            DrawOp::Edge { id, path, .. } => {
                let key = (layer, id.clone());
                if self.paths.get(&key).is_some_and(|old| old != path) {
                    self.violate(now, format!("I6: edge {} moved", id.as_str()));
                }
                self.paths.insert(key, *path);
            }
            DrawOp::Annotate { .. }
            | DrawOp::Strike { .. }
            | DrawOp::Trace { .. }
            | DrawOp::Write { .. } => {}
            DrawOp::Participant { .. } | DrawOp::Message { .. } | DrawOp::Cells { .. } => {
                self.check_stable_diagram(now, layer, op);
            }
        }
    }

    /// I6 for sequence diagrams and cells: a head, an arrow and a cell never move, and a
    /// lifeline only grows, each stretch starting exactly where the drawn line ends, straight
    /// under its head.
    fn check_stable_diagram(&mut self, now: SessionTime, layer: LayerId, op: &DrawOp) {
        match op {
            DrawOp::Participant {
                id,
                rect,
                lifeline_bottom,
                ..
            } => {
                let key = (layer, id.clone());
                if let Some((head, bottom)) = self.lifelines.get(&key).copied()
                    && (head != *rect || *lifeline_bottom < bottom)
                {
                    self.violate(now, format!("I6: participant {} moved", id.as_str()));
                }
                self.lifelines.insert(key, (*rect, *lifeline_bottom));
            }
            DrawOp::Message {
                id, points, extend, ..
            } => {
                let key = (layer, id.clone());
                if self.arrows.get(&key).is_some_and(|old| old != points) {
                    self.violate(now, format!("I6: message {} moved", id.as_str()));
                }
                self.arrows.insert(key, points.clone());
                for stretch in extend {
                    let key = (layer, stretch.participant.clone());
                    let continues = self.lifelines.get(&key).is_some_and(|(head, bottom)| {
                        stretch.x == head.center().x
                            && stretch.top == *bottom
                            && stretch.bottom > stretch.top
                    });
                    if !continues {
                        self.violate(
                            now,
                            format!(
                                "I6: the lifeline of {} grew from {}, not from where it ended",
                                stretch.participant.as_str(),
                                stretch.top
                            ),
                        );
                    }
                    if let Some(line) = self.lifelines.get_mut(&key) {
                        line.1 = line.1.max(stretch.bottom);
                    }
                }
            }
            DrawOp::Cells { cells, .. } => {
                for cell in cells {
                    let key = (layer, cell.id.clone());
                    if self.cells.get(&key).is_some_and(|old| *old != cell.rect) {
                        self.violate(now, format!("I6: cell {} moved", cell.id.as_str()));
                    }
                    self.cells.insert(key, cell.rect);
                }
            }
            DrawOp::Node { .. }
            | DrawOp::Edge { .. }
            | DrawOp::Annotate { .. }
            | DrawOp::Strike { .. }
            | DrawOp::Trace { .. }
            | DrawOp::Write { .. } => {}
        }
    }

    /// Checks that hold after every step.
    pub fn after_step(&mut self, now: SessionTime, core: &Core, scripts: &[String]) {
        // I3: the conversation only grows; earlier messages never change.
        let messages = core.conversation().messages();
        let kept = messages.len() >= self.history.len()
            && messages[..self.history.len()] == self.history[..];
        if !kept {
            self.violate(now, "I3: the conversation was edited".into());
        }
        for message in &messages[self.history.len().min(messages.len())..] {
            self.check_committed(now, message, scripts);
        }
        for problem in history_shape(messages) {
            self.violate(now, problem);
        }
        self.history = messages.to_vec();
        // I4: pushes and pops balance with the open detours.
        let open = self.layer_pushes.saturating_sub(self.layer_pops);
        if open != core.thread().depth() || core.thread().depth() > DETOUR_DEPTH_MAX {
            let depth = core.thread().depth();
            self.violate(
                now,
                format!("I4: {open} layers open but the thread has {depth} detours"),
            );
        }
    }

    /// I2 (simulator form): text committed without the provider's content is a prefix of what
    /// the talker really said, tags removed.
    fn check_committed(&mut self, now: SessionTime, message: &Message, scripts: &[String]) {
        let Message::Assistant { parts, raw: None } = message else {
            return;
        };
        for part in parts {
            let AssistantPart::Text { text } = part else {
                continue;
            };
            let heard = squash(&strip_tags(text));
            let said = scripts
                .iter()
                .any(|script| squash(&strip_tags(script)).starts_with(&heard));
            if !said {
                self.violate(now, format!("I2: committed text was never said: {text:?}"));
            }
        }
    }
}

/// What the providers require of the history's shape, which an append-only history can never
/// repair (D-036, review findings 1 and 11): it starts with the user, every `tool_use` is answered
/// in the very next message, and every tool result answers a call of the message right before.
#[must_use]
pub fn history_shape(messages: &[Message]) -> Vec<String> {
    let mut problems = Vec::new();
    let first = messages
        .iter()
        .find(|message| !matches!(message, Message::System { .. }));
    if matches!(first, Some(Message::Assistant { .. })) {
        problems.push("history: the first turn is the assistant's".to_owned());
    }
    for (index, message) in messages.iter().enumerate() {
        let calls = tool_uses(message);
        let answered: Vec<&str> = messages
            .get(index + 1)
            .map(tool_results)
            .unwrap_or_default();
        for id in &calls {
            if !answered.contains(id) {
                problems.push(format!(
                    "history: tool_use {id} has no tool_result after it"
                ));
            }
        }
        let asked: Vec<&str> = index
            .checked_sub(1)
            .and_then(|before| messages.get(before))
            .map(tool_uses)
            .unwrap_or_default();
        for id in tool_results(message) {
            if !asked.contains(&id) {
                problems.push(format!(
                    "history: tool_result {id} answers no call before it"
                ));
            }
        }
    }
    problems
}

fn tool_uses(message: &Message) -> Vec<&str> {
    let Message::Assistant { parts, .. } = message else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolUse { id, .. } => Some(id.as_str()),
            AssistantPart::Text { .. } => None,
        })
        .collect()
}

fn tool_results(message: &Message) -> Vec<&str> {
    let Message::User { parts } = message else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            UserPart::ToolResult { id, .. } => Some(id.as_str()),
            UserPart::Text { .. } => None,
        })
        .collect()
}

fn op_id(op: &DrawOp) -> OpId {
    match op {
        DrawOp::Node { op, .. }
        | DrawOp::Edge { op, .. }
        | DrawOp::Annotate { op, .. }
        | DrawOp::Strike { op, .. }
        | DrawOp::Trace { op, .. }
        | DrawOp::Participant { op, .. }
        | DrawOp::Message { op, .. }
        | DrawOp::Cells { op, .. }
        | DrawOp::Write { op, .. } => *op,
    }
}

/// Removes `<…>` spans the way the parser does for well-formed tags: a `>` inside a quoted
/// value (`path="api>redis"`) does not end the tag.
#[must_use]
pub fn strip_tags(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut in_tag = false;
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        match (in_tag, quote, ch) {
            (false, _, '<') => in_tag = true,
            (false, _, _) => plain.push(ch),
            (true, Some(open), _) if ch == open => quote = None,
            (true, None, '"' | '\'') => quote = Some(ch),
            (true, None, '>') => in_tag = false,
            (true, _, _) => {}
        }
    }
    plain
}

/// Words joined by single spaces, so clause cuts don't matter.
#[must_use]
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::history_shape;
    use crate::history::{AssistantPart, Message, UserPart};
    use crate::ids::ToolCallId;

    fn id(text: &str) -> ToolCallId {
        ToolCallId::parse(text).unwrap_or_else(|| unreachable!("a valid id"))
    }

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![UserPart::Text { text: text.into() }],
        }
    }

    fn calls(ids: &[&str]) -> Message {
        Message::Assistant {
            parts: ids
                .iter()
                .map(|call| AssistantPart::ToolUse {
                    id: id(call),
                    name: "read_file".into(),
                    input_json: "{}".into(),
                })
                .collect(),
            raw: None,
        }
    }

    fn results(ids: &[&str]) -> Message {
        Message::User {
            parts: ids
                .iter()
                .map(|call| UserPart::ToolResult {
                    id: id(call),
                    content: "ok".into(),
                    is_error: false,
                })
                .collect(),
        }
    }

    #[test]
    fn a_well_formed_history_passes() {
        let history = [
            Message::System {
                text: "Board now: empty".into(),
            },
            user("show me the service"),
            calls(&["a", "b"]),
            results(&["a", "b"]),
            Message::Assistant {
                parts: vec![AssistantPart::Text {
                    text: "Here.".into(),
                }],
                raw: None,
            },
        ];
        assert!(history_shape(&history).is_empty());
    }

    /// The planted violations: each shape the providers reject turns the check red.
    #[test]
    fn orphans_strays_and_an_assistant_first_are_caught() {
        let orphan = [user("hi"), calls(&["a"]), user("what does it do")];
        assert_eq!(
            history_shape(&orphan).len(),
            1,
            "{:?}",
            history_shape(&orphan)
        );
        let late = [
            user("hi"),
            calls(&["a"]),
            Message::System {
                text: "note".into(),
            },
            results(&["a"]),
        ];
        assert_eq!(history_shape(&late).len(), 2, "{:?}", history_shape(&late));
        let partial = [user("hi"), calls(&["a", "b"]), results(&["a"])];
        assert_eq!(history_shape(&partial).len(), 1);
        let assistant_first = [
            Message::System {
                text: "context".into(),
            },
            Message::Assistant {
                parts: vec![AssistantPart::Text {
                    text: "Sure".into(),
                }],
                raw: None,
            },
            user("and the database"),
        ];
        assert_eq!(history_shape(&assistant_first).len(), 1);
    }
}
