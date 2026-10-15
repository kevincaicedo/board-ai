//! Invariants checked on every simulator step (LLD §2.9). A violation is recorded with the time
//! it happened; scenarios and the fuzzer assert there are none.

use std::collections::{BTreeMap, BTreeSet};

use crate::conversation::{ConversationState, cues};
use crate::effect::Effect;
use crate::engine::Core;
use crate::geometry::Rect;
use crate::harness::HarnessKind;
use crate::history::{AssistantPart, Message};
use crate::ids::{ElementId, LayerId, OpId};
use crate::input::Input;
use crate::limits::{BARGE_IN_DECISION_MS, DETOUR_DEPTH_MAX};
use crate::protocol::{DrawOp, EdgePath, NodeShape, UiEvent};
use crate::sim::speech::FakeSpeech;
use crate::time::SessionTime;
use crate::tools::ToolCall;

#[derive(Clone, Debug, Default)]
pub struct Checker {
    violations: Vec<String>,
    talk_in_flight: bool,
    history: Vec<Message>,
    boxes: BTreeMap<(LayerId, ElementId), (Rect, NodeShape)>,
    paths: BTreeMap<(LayerId, ElementId), EdgePath>,
    started_ops: BTreeSet<OpId>,
    layer_pushes: usize,
    layer_pops: usize,
    change_proposed: bool,
    user_finals: String,
    /// Windows in which a backchannel was said over BoardAI's voice: no `StopVoice` allowed.
    backchannels: Vec<(SessionTime, SessionTime)>,
}

impl Checker {
    #[must_use]
    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    fn violate(&mut self, now: SessionTime, what: String) {
        self.violations.push(format!("{} ms: {what}", now.as_ms()));
    }

    /// A backchannel was scripted at `start`; I7 says no `StopVoice` may follow it.
    pub fn expect_backchannel(&mut self, start: SessionTime, core: &Core) {
        if *core.state() == ConversationState::Speaking {
            let end = start.plus_ms(BARGE_IN_DECISION_MS * 2);
            self.backchannels.push((start, end));
        }
    }

    /// Called before the core steps with `input`.
    pub fn on_input(&mut self, input: &Input, core: &Core) {
        match input {
            Input::Transcript {
                text,
                is_final: true,
            } => {
                // Only words the core was listening to count; speech while muted or ignored by
                // push-to-talk was never an utterance.
                let listening = matches!(
                    core.state(),
                    ConversationState::Listening(_) | ConversationState::Ducked(_)
                );
                if listening {
                    self.user_finals.push(' ');
                    self.user_finals.push_str(text);
                }
            }
            Input::TalkerDone { .. } => self.talk_in_flight = false,
            Input::TalkerToolCall { invocation } => {
                if matches!(invocation.call, Ok(ToolCall::ProposeChange { .. })) {
                    self.change_proposed = true;
                }
            }
            Input::VoiceStart
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
            | Input::Tick => {}
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
            Effect::StopVoice => {
                if self
                    .backchannels
                    .iter()
                    .any(|(start, end)| *start <= now && now <= *end)
                {
                    self.violate(now, "I7: a backchannel stopped the voice".into());
                }
            }
            // I9: a change runs only after a proposal and a spoken yes.
            Effect::RunHarness { spec, .. } => {
                if let HarnessKind::Change { .. } = spec.kind {
                    let yes = cues::is_affirmative(&self.user_finals);
                    if !self.change_proposed || !yes {
                        let finals = self.user_finals.clone();
                        self.violate(
                            now,
                            format!("I9: change ran without a yes (heard {finals:?})"),
                        );
                    }
                    self.change_proposed = false;
                }
            }
            Effect::Ui(event) => self.on_ui(now, event, core, speech),
            Effect::Duck { .. }
            | Effect::Synthesize { .. }
            | Effect::RunTool { .. }
            | Effect::CancelHarness { .. }
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::WakeAt { .. } => {}
        }
    }

    fn on_ui(&mut self, now: SessionTime, event: &UiEvent, core: &Core, speech: &FakeSpeech) {
        match event {
            UiEvent::OpStart { layer, op, .. } => {
                self.check_anchor(now, op, core, speech);
                self.check_stable(now, *layer, op);
            }
            UiEvent::LayerPush { .. } => self.layer_pushes += 1,
            UiEvent::LayerPop { .. } => self.layer_pops += 1,
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
            | UiEvent::Camera { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. } => {}
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
            DrawOp::Annotate { .. } | DrawOp::Strike { .. } | DrawOp::Trace { .. } => {}
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

fn op_id(op: &DrawOp) -> OpId {
    match op {
        DrawOp::Node { op, .. }
        | DrawOp::Edge { op, .. }
        | DrawOp::Annotate { op, .. }
        | DrawOp::Strike { op, .. }
        | DrawOp::Trace { op, .. } => *op,
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
