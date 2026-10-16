//! The user's side of the conversation (LLD §2.2): voice activity, transcripts, end of turn,
//! barge-in, push-to-talk, mute, the user's message, and the change gate (I9).

use crate::board::FrozenOp;
use crate::conversation::ConversationState;
use crate::conversation::barge_in::{BargeIn, Decision};
use crate::conversation::cues;
use crate::conversation::utterance::Utterance;
use crate::effect::{Effect, Gain, Milestone};
use crate::engine::{Core, MicMode, TurnMarks, milestone};
use crate::harness::{HarnessKind, HarnessTask};
use crate::history::{Message, UserPart};
use crate::limits::{
    BACKCHANNEL_LATE_FINAL_MS, DETOUR_QUESTION_MAX_BYTES, VOICE_ONSET_DUCK_HOLD_MS,
};
use crate::narration::tags::bounded;
use crate::protocol::{MicAction, PointerTarget, Speaker, UiEvent};
use crate::time::SessionTime;

/// The user's message for the turn being answered. It enters the history right before the first
/// assistant content of the turn, so words added before anything was heard can still join it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingUser {
    pub text: String,
    pub context: UserContext,
    pub message: Message,
}

/// What the next user message reports besides the words: the interruption note (case b of LLD
/// §2.7), the frozen stroke, and how many drawing steps were dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct UserContext {
    pub note: Option<String>,
    pub frozen: Vec<FrozenOp>,
    pub dropped_ops: usize,
    /// The words cut BoardAI off, so a detour opened in reply answers them.
    pub interrupted: bool,
}

impl UserContext {
    fn merge(&mut self, later: Self) {
        if later.note.is_some() {
            self.note = later.note;
        }
        self.frozen.extend(later.frozen);
        self.dropped_ops += later.dropped_ops;
        self.interrupted |= later.interrupted;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct UserSide {
    pub pending: Option<PendingUser>,
    pub context: UserContext,
}

/// The confirmation gate for code changes (LLD §3.5, I9). Only the user's next utterance can
/// open it; tool results and repo text never reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChangeGate {
    Closed,
    Awaiting {
        instruction: String,
        branch: String,
        /// When it was proposed: only words begun after it can answer it.
        opened_at: SessionTime,
    },
}

impl Core {
    /// The first sign of the user's voice. While BoardAI speaks, the voice ducks at once, so the
    /// duck lands within its budget although `VoiceStart` needs ~160 ms more to confirm speech.
    /// The duck is tentative: `VoiceStart` takes it over as a barge-in; without one (a cough, a
    /// click) the gain comes back when the hold ends and nothing else changes. Anywhere else an
    /// onset means nothing until `VoiceStart`.
    pub(super) fn on_voice_onset(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let ignored = self.config.mic == MicMode::PushToTalk && !self.push_to_talk_held;
        if ignored || self.state != ConversationState::Speaking {
            return;
        }
        if self.onset_duck_until.is_none() {
            self.set_gain(Gain::DUCKED, effects);
            milestone(effects, Milestone::OnsetDuck);
        }
        let until = now.plus_ms(VOICE_ONSET_DUCK_HOLD_MS);
        self.onset_duck_until = Some(until);
        self.wake(until, effects);
    }

    /// The onset's hold has ended with no barge-in owning the duck: the gain comes back.
    pub(super) fn release_onset_duck(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if self.onset_duck_until.is_none_or(|until| now < until) {
            return;
        }
        self.onset_duck_until = None;
        if !matches!(self.state, ConversationState::Ducked(_)) {
            self.set_gain(Gain::FULL, effects);
        }
    }

    pub(super) fn on_voice_start(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if self.config.mic == MicMode::PushToTalk && !self.push_to_talk_held {
            return;
        }
        self.take_floor(now, effects);
    }

    /// The user started talking while BoardAI was idle, thinking or listening.
    fn take_floor(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if self.state != ConversationState::Muted {
            self.clear_suggestion(effects);
        }
        match &mut self.state {
            ConversationState::Muted => {}
            ConversationState::Idle => self.listen(Utterance::start(now, true), effects),
            ConversationState::Listening(utterance) => utterance.voice_start(now),
            ConversationState::Ducked(barge_in) => barge_in.utterance_mut().voice_start(now),
            ConversationState::Thinking => self.rejoin(now, effects),
            ConversationState::Speaking => self.duck(now, effects),
        }
    }

    /// The user takes a free floor: a new turn starts.
    fn listen(&mut self, utterance: Utterance, effects: &mut Vec<Effect>) {
        milestone(effects, Milestone::VoiceStart);
        self.hand_floor(utterance, effects);
    }

    /// Speaking + voice: duck first, decide second (PRODUCT §6.2).
    fn duck(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        // The barge-in owns the gain from here; the onset's hold no longer restores it.
        self.onset_duck_until = None;
        let barge_in = BargeIn::new(now);
        self.set_gain(Gain::DUCKED, effects);
        milestone(effects, Milestone::BargeInDuck);
        self.wake(barge_in.decide_by(), effects);
        self.state = ConversationState::Ducked(barge_in);
        self.evaluate_barge_in(now, effects);
    }

    /// Thinking + voice: the request is cancelled. When nothing of the answer was heard, the new
    /// words join the same turn; otherwise they start a new one.
    fn rejoin(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let heard_something = self.voice.heard().is_some()
            || self
                .reply
                .as_ref()
                .is_some_and(|reply| reply.streams.iter().any(|stream| stream.committed));
        let utterance = match (heard_something, self.user.pending.take()) {
            (false, Some(pending)) => {
                let mut context = pending.context;
                context.merge(std::mem::take(&mut self.user.context));
                self.user.context = context;
                Utterance::resume(&pending.text, now)
            }
            (_, pending) => {
                self.user.pending = pending;
                Utterance::start(now, true)
            }
        };
        self.stop_reply(now, false, effects);
        self.listen(utterance, effects);
    }

    pub(super) fn on_voice_end(
        &mut self,
        now: SessionTime,
        voiced_ms: u32,
        effects: &mut Vec<Effect>,
    ) {
        let deadline = match &mut self.state {
            ConversationState::Listening(utterance) => {
                utterance.voice_end(now, voiced_ms);
                utterance.end_of_turn_at()
            }
            ConversationState::Ducked(barge_in) => {
                barge_in.utterance_mut().voice_end(now, voiced_ms);
                None
            }
            ConversationState::Idle
            | ConversationState::Thinking
            | ConversationState::Speaking
            | ConversationState::Muted => None,
        };
        if let Some(deadline) = deadline {
            self.wake(deadline, effects);
        }
        self.evaluate_barge_in(now, effects);
    }

    pub(super) fn on_transcript(
        &mut self,
        now: SessionTime,
        text: &str,
        is_final: bool,
        effects: &mut Vec<Effect>,
    ) {
        let utterance = match &mut self.state {
            ConversationState::Listening(utterance) => utterance,
            ConversationState::Ducked(barge_in) => barge_in.utterance_mut(),
            ConversationState::Speaking => {
                self.late_cue(now, text, is_final, effects);
                return;
            }
            ConversationState::Idle | ConversationState::Thinking | ConversationState::Muted => {
                return;
            }
        };
        utterance.transcript(text, is_final);
        let caption = UiEvent::Caption {
            who: Speaker::User,
            text: utterance.text(),
            is_final,
        };
        let deadline = utterance.end_of_turn_at();
        effects.push(Effect::Ui(caption));
        if is_final {
            milestone(effects, Milestone::FinalTranscript);
        }
        if let (Some(deadline), ConversationState::Listening(_)) = (deadline, &self.state) {
            self.wake(deadline, effects);
        }
        self.evaluate_barge_in(now, effects);
    }

    /// A cue word said too fast for a partial ("wait") was taken for a backchannel at
    /// `VoiceEnd`; its final transcript, arriving just after, still interrupts.
    fn late_cue(
        &mut self,
        now: SessionTime,
        text: &str,
        is_final: bool,
        effects: &mut Vec<Effect>,
    ) {
        let recent = self
            .backchannel_at
            .is_some_and(|at| now.ms_since(at) <= BACKCHANNEL_LATE_FINAL_MS);
        if !(is_final && recent && cues::has_interruption_cue(text)) {
            return;
        }
        self.backchannel_at = None;
        let mut utterance = Utterance::start(now, false);
        utterance.transcript(text, true);
        effects.push(Effect::Ui(UiEvent::Caption {
            who: Speaker::User,
            text: utterance.text(),
            is_final: true,
        }));
        self.interrupt(now, utterance, effects);
    }

    pub(super) fn evaluate_barge_in(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let ConversationState::Ducked(barge_in) = &self.state else {
            return;
        };
        match barge_in.decide(now, self.pointer.set_at) {
            Decision::Undecided => {}
            Decision::Backchannel => {
                self.set_gain(Gain::FULL, effects);
                effects.push(Effect::Ui(UiEvent::Acknowledge));
                self.backchannel_at = Some(now);
                self.state = if self.voice.is_audible() {
                    ConversationState::Speaking
                } else {
                    ConversationState::Thinking
                };
            }
            Decision::Interruption(_) => {
                let state = std::mem::replace(&mut self.state, ConversationState::Idle);
                if let ConversationState::Ducked(barge_in) = state {
                    self.interrupt(now, barge_in.into_utterance(), effects);
                }
            }
        }
    }

    pub(super) fn on_mic(
        &mut self,
        now: SessionTime,
        action: MicAction,
        effects: &mut Vec<Effect>,
    ) {
        match action {
            MicAction::Mute => self.mute(now, effects),
            MicAction::Unmute => {
                if self.state == ConversationState::Muted {
                    self.state = ConversationState::Idle;
                }
            }
            MicAction::TalkDown => {
                self.push_to_talk_held = true;
                // Holding the key is an explicit request for the floor: no duck, no deciding.
                match std::mem::replace(&mut self.state, ConversationState::Idle) {
                    ConversationState::Speaking => {
                        self.interrupt(now, Utterance::start(now, true), effects);
                    }
                    ConversationState::Ducked(barge_in) => {
                        self.interrupt(now, barge_in.into_utterance(), effects);
                    }
                    state @ (ConversationState::Idle
                    | ConversationState::Listening(_)
                    | ConversationState::Thinking
                    | ConversationState::Muted) => {
                        self.state = state;
                        self.take_floor(now, effects);
                    }
                }
            }
            MicAction::TalkUp => {
                self.push_to_talk_held = false;
                if let ConversationState::Listening(utterance) = &mut self.state {
                    utterance.release(now);
                    if let Some(deadline) = utterance.end_of_turn_at() {
                        self.wake(deadline, effects);
                    }
                }
            }
        }
    }

    /// Mute stops everything: the voice, the request, and the utterance in progress.
    fn mute(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let abandoned = match &self.state {
            ConversationState::Muted => return,
            ConversationState::Listening(utterance) => utterance.text(),
            ConversationState::Ducked(barge_in) => barge_in.utterance().text(),
            ConversationState::Idle | ConversationState::Thinking | ConversationState::Speaking => {
                String::new()
            }
        };
        // Words cut off by mute were the user's next words: they can cancel a proposed change,
        // never confirm one (I9).
        if !abandoned.trim().is_empty() && matches!(self.gate, ChangeGate::Awaiting { .. }) {
            self.gate = ChangeGate::Closed;
            self.context.notice(
                "The user muted the microphone mid-sentence, so the proposed change was \
                 cancelled. Nothing was changed."
                    .to_owned(),
            );
        }
        if !abandoned.trim().is_empty() {
            self.close_proposal_on_mute();
        }
        self.stop_reply(now, true, effects);
        self.state = ConversationState::Muted;
    }

    pub(super) fn check_turn_deadlines(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let ConversationState::Listening(utterance) = &self.state else {
            return;
        };
        let capped = now >= utterance.cap_at();
        // While push-to-talk is held the user keeps the floor through pauses.
        let silent = !self.push_to_talk_held
            && utterance
                .end_of_turn_at()
                .is_some_and(|deadline| now >= deadline);
        if capped || silent {
            self.end_turn(now, effects);
        }
    }

    /// The user's turn is over: resolve the change gate, build the message, and ask the talker.
    fn end_turn(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let state = std::mem::replace(&mut self.state, ConversationState::Idle);
        let ConversationState::Listening(utterance) = state else {
            self.state = state;
            return;
        };
        if self.stop.is_some() {
            self.settle_stop(None, effects);
        }
        let mut text = utterance.text();
        // When these words began. Joined earlier words began before anything was proposed.
        let mut began = utterance.started_at();
        let mut context = std::mem::take(&mut self.user.context);
        if let Some(previous) = self.user.pending.take() {
            began = SessionTime::ZERO;
            // Nothing of the answer to the previous words was heard: they are one turn.
            text = format!("{} {text}", previous.text).trim().to_owned();
            let mut merged = previous.context;
            merged.merge(context);
            context = merged;
        }
        if text.trim().is_empty() {
            self.user.context = context;
            return;
        }
        // Keeping a proposal, the memory controls, "close that" and "open settings" are answered
        // by the core itself; what the user was told so far waits for their next real question.
        if self.resolve_proposal(&text, began, effects) {
            self.user.context = context;
            return;
        }
        self.resolve_gate(&text, began, effects);
        self.cancel_by_voice(&text, effects);
        if self.memory_control(&text, effects) || self.open_settings_by_voice(&text, effects) {
            self.user.context = context;
            return;
        }
        self.turn_question = self.question_of(&text, &context);
        let message = self.user_message(&text, &context);
        self.user.pending = Some(PendingUser {
            text,
            context,
            message,
        });
        self.marks = TurnMarks::default();
        self.state = ConversationState::Thinking;
        self.request_talk(now, effects);
    }

    /// What a detour opened in reply would answer: the user's words without their lead-in, when
    /// they cut BoardAI off or follow up on what is drawn. A first question on an empty board
    /// isn't a detour's question, even if the talker opens one.
    fn question_of(&self, text: &str, context: &UserContext) -> Option<String> {
        let follows_up = self
            .board
            .layers()
            .iter()
            .any(|layer| layer.bounds().is_some());
        let question = cues::without_lead_in(text).trim();
        ((context.interrupted || follows_up) && !question.is_empty())
            .then(|| bounded(question, DETOUR_QUESTION_MAX_BYTES))
    }

    /// "Stop that research", "cancel that": the named running task is cancelled at once, and the
    /// talker hears the words with a note saying it is done, so it only has to confirm.
    fn cancel_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) {
        let Some(target) = cues::harness_cancel(text) else {
            return;
        };
        let Some((task, kind)) = self.context.cancel_task(target) else {
            return;
        };
        effects.push(Effect::CancelHarness { task });
        self.context.notice(format!(
            "{} task {} was cancelled because the user asked to stop it. Confirm in three words \
             or fewer.",
            crate::engine::text::harness_kind_name(&kind),
            task.0
        ));
    }

    /// I9: only this utterance can confirm the proposed change, and only with a plain yes. Words
    /// the user was already saying when the change was proposed answer something else: the gate
    /// waits for the next ones.
    fn resolve_gate(&mut self, text: &str, began: SessionTime, effects: &mut Vec<Effect>) {
        if let ChangeGate::Awaiting { opened_at, .. } = &self.gate
            && *opened_at > began
        {
            return;
        }
        let ChangeGate::Awaiting {
            instruction,
            branch,
            ..
        } = std::mem::replace(&mut self.gate, ChangeGate::Closed)
        else {
            return;
        };
        if !cues::is_affirmative(text) {
            self.context.notice(
                "The user did not say yes, so the proposed change was cancelled. Nothing was \
                 changed."
                    .to_owned(),
            );
            return;
        }
        let kind = HarnessKind::Change {
            branch: branch.clone(),
        };
        let task = self.start_task(kind.clone());
        effects.push(Effect::RunHarness {
            task,
            spec: HarnessTask {
                kind,
                brief: instruction,
            },
        });
        self.context.notice(format!(
            "The user said yes. Change task {} is running on branch {branch}; its result arrives \
             as a system message.",
            task.0
        ));
    }

    /// The user's message: the interruption note first, then the words, then where they were.
    fn user_message(&self, text: &str, context: &UserContext) -> Message {
        let mut lines = Vec::new();
        if let Some(note) = &context.note {
            lines.push(note.clone());
        }
        lines.push(text.to_owned());
        if let Some(pointer) = self.describe_pointer() {
            lines.push(pointer);
        }
        if self.thread.has_context() {
            lines.push(format!("[Thread: {}]", self.thread.describe()));
        }
        for frozen in &context.frozen {
            lines.push(format!(
                "[Drawing stopped at {}% of {} \"{}\". It finishes when you resume or draw on that \
                 layer again.]",
                frozen.progress_percent,
                frozen.id.as_str(),
                frozen.label
            ));
        }
        if context.dropped_ops > 0 {
            lines.push(format!(
                "[{} planned drawing steps were never drawn. Replan them if you still need them.]",
                context.dropped_ops
            ));
        }
        Message::User {
            parts: vec![UserPart::Text {
                text: lines.join("\n"),
            }],
        }
    }

    fn describe_pointer(&self) -> Option<String> {
        match self.pointer.target.as_ref()? {
            PointerTarget::Element { layer, id } => {
                let element = self.board.layer(*layer)?.element(id)?;
                let place = if layer.0 == 0 {
                    "the main board"
                } else {
                    "a detour"
                };
                Some(format!(
                    "[Pointing at: {} \"{}\" on {place}]",
                    id.as_str(),
                    element.label
                ))
            }
            PointerTarget::CodeLine { surface, line } => {
                let (path, text) = self.surfaces.code_line(*surface, *line)?;
                Some(format!("[Pointing at: {path}:{line}: {}]", text.trim()))
            }
        }
    }
}
