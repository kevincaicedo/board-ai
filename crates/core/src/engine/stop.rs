//! Interruption and commit-what-was-heard (LLD §2.7, invariants I2 and I3).
//!
//! Stopping happens in two steps. At once: the stream is cancelled, the stroke in flight
//! freezes, ops not yet started are dropped, and the voice is told to stop. Then, when the
//! runtime answers with where the voice stopped (or the answer times out), what was heard is
//! committed to the history, which is never edited:
//! - (a) a reply not yet in the history gets an assistant message holding only the heard prefix;
//! - (b) a reply already stored stays untouched, and the next user message opens with a note
//!   saying where the user stopped hearing it.

use crate::conversation::utterance::Utterance;
use crate::conversation::{ConversationState, cues};
use crate::effect::{Effect, Gain, Milestone};
use crate::engine::gate::GateClosed;
use crate::engine::{Core, milestone};
use crate::heard::HeardPrefix;
use crate::history::{AssistantPart, Message};
use crate::ids::OpId;
use crate::input::PlaybackPosition;
use crate::limits::VOICE_STOP_TIMEOUT_MS;
use crate::protocol::UiEvent;
use crate::time::SessionTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PendingStop {
    deadline: SessionTime,
    stopped_voice: bool,
    /// Mute: a question nothing was heard of is dropped, not carried into the next turn.
    abandon_question: bool,
}

impl PendingStop {
    pub(super) fn deadline(&self) -> SessionTime {
        self.deadline
    }
}

impl Core {
    /// A confirmed interruption: BoardAI stops and the user keeps the floor.
    pub(super) fn interrupt(
        &mut self,
        now: SessionTime,
        utterance: Utterance,
        effects: &mut Vec<Effect>,
    ) {
        milestone(effects, Milestone::BargeInStop);
        self.user.context.interrupted = true;
        self.stop_reply(now, false, effects);
        self.hand_floor(utterance, effects);
    }

    /// `Esc` (PRODUCT §8.9): BoardAI stops at once, like a confirmed interruption with no words.
    /// The stroke in flight freezes, what was heard is committed, and the floor goes idle. A
    /// question nothing was heard of is dropped, as with mute: a stop means "not this", so a
    /// change waiting for a yes is cancelled in every state (I9).
    pub(super) fn on_stop(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        self.close_change_gate(GateClosed::Stop);
        match std::mem::replace(&mut self.state, ConversationState::Idle) {
            ConversationState::Speaking | ConversationState::Thinking => {
                self.stop_reply(now, true, effects);
            }
            // The user is already talking over BoardAI: the stop confirms the interruption.
            ConversationState::Ducked(barge_in) => {
                self.interrupt(now, barge_in.into_utterance(), effects);
            }
            // Quiet, Esc backs out of a detail layer (D-188), as "back out" does.
            ConversationState::Idle => {
                let detail = self
                    .thread
                    .detours()
                    .top()
                    .is_some_and(|detour| detour.anchor.is_some());
                if detail && !self.is_drawing() {
                    self.back_out(now, effects);
                }
            }
            state @ (ConversationState::Listening(_) | ConversationState::Muted) => {
                self.state = state;
            }
        }
    }

    /// The user has the floor with this utterance; its deadlines are armed.
    pub(super) fn hand_floor(&mut self, utterance: Utterance, effects: &mut Vec<Effect>) {
        let cap = utterance.cap_at();
        let end = utterance.end_of_turn_at();
        self.state = ConversationState::Listening(utterance);
        self.wake(cap, effects);
        if let Some(end) = end {
            self.wake(end, effects);
        }
    }

    /// Ends the assistant's side of the turn now; what was heard is committed when the voice
    /// confirms where it stopped.
    pub(super) fn stop_reply(
        &mut self,
        now: SessionTime,
        abandon_question: bool,
        effects: &mut Vec<Effect>,
    ) {
        // A stop, an interruption or mute cut the reply off: if it was the end-of-session line,
        // the user is still here, so the session goes on; and it asked nothing to its end.
        self.keep_session();
        self.reply_asked = false;
        if let Some(reply) = &mut self.reply {
            if reply.in_flight {
                effects.push(Effect::CancelTalk);
                reply.in_flight = false;
            }
            // Results of calls from a cancelled stream are ignored when they arrive.
            reply.tools.clear();
        }
        // So are views read for its tags: one must not open over the words that stopped it.
        self.drop_written_reads();
        self.freeze(now, effects);
        if let Some(stop) = &mut self.stop {
            stop.abandon_question |= abandon_question;
            return;
        }
        let stopped_voice = !self.voice.is_silent();
        let deadline = now.plus_ms(VOICE_STOP_TIMEOUT_MS);
        self.stop = Some(PendingStop {
            deadline,
            stopped_voice,
            abandon_question,
        });
        if stopped_voice {
            effects.push(Effect::StopVoice);
            self.wake(deadline, effects);
        } else {
            self.settle_stop(None, effects);
        }
    }

    /// The board stops with the voice: the stroke in flight freezes where it is, and ops that
    /// have not started are dropped. Elements laid out but never started go away unseen.
    fn freeze(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let layers: Vec<_> = self
            .board
            .layers()
            .iter()
            .filter(|layer| layer.is_open())
            .map(crate::board::Layer::id)
            .collect();
        for layer in layers {
            if let Some(frozen) = self.board.freeze(layer, now) {
                effects.push(Effect::Ui(UiEvent::Freeze { layer }));
                self.user.context.frozen.push(frozen);
            }
        }
        self.user.context.dropped_ops += self.drop_queued_ops();
    }

    /// The voice stopped at `last` (or the answer timed out): commit what was heard.
    pub(super) fn settle_stop(
        &mut self,
        last: Option<PlaybackPosition>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(stop) = self.stop.take() else {
            return;
        };
        let heard = self.voice.heard();
        let position = match (last, heard) {
            (Some(last), Some(heard)) => Some(
                if (last.clause, last.offset_ms) >= (heard.clause, heard.offset_ms) {
                    last
                } else {
                    heard
                },
            ),
            (last, heard) => last.or(heard),
        };
        self.commit_heard(position, true);
        if stop.abandon_question {
            self.user.pending = None;
        }
        self.end_reply();
        // A barge-in decided while nothing was playing stopped no audio, but its duck still has
        // to be undone, or the next answer is heard at the ducked level.
        if stop.stopped_voice || self.gain != Gain::FULL {
            self.set_gain(Gain::FULL, effects);
            self.onset_duck_until = None;
        }
    }

    /// Forgets the finished reply: its clauses, its ops and their anchors. A change it proposed
    /// whose question never played to the end goes with it.
    pub(super) fn end_reply(&mut self) {
        self.drop_unasked_change();
        self.reply = None;
        self.voice.clear();
        self.queue.fired.clear();
        #[cfg(feature = "sim")]
        self.anchors.clear();
    }

    /// Appends what the user heard of each stream not yet in the history (case a). When the
    /// voice stopped inside a stored stream, the next user message gets the note (case b).
    pub(super) fn commit_heard(&mut self, position: Option<PlaybackPosition>, interrupted: bool) {
        let Some(reply) = self.reply.take() else {
            return;
        };
        let mut note = None;
        for (index, stream) in reply.streams.iter().enumerate() {
            let fired = &self.queue.fired;
            let drawn = |op: u32| fired.contains(&OpId(stream.op_base.saturating_add(op)));
            let heard =
                HeardPrefix::from_playback(position, &self.voice, index, &stream.narration, drawn);
            if stream.committed {
                if interrupted && !heard.is_complete() && note.is_none() {
                    note = Some(heard.note());
                }
            } else if !heard.is_empty() {
                self.flush_user();
                self.conversation.push(Message::Assistant {
                    parts: vec![AssistantPart::Text {
                        text: heard.written().to_owned(),
                    }],
                    raw: None,
                });
            }
        }
        if note.is_some() {
            self.user.context.note = note;
        }
        self.reply = Some(reply);
    }

    /// Commits the user's message of this turn, right before the assistant's first content.
    pub(super) fn flush_user(&mut self) {
        if let Some(pending) = self.user.pending.take() {
            self.conversation.push(pending.message);
        }
    }

    /// The talker is done and everything it said has played: commit it and go idle. The parts
    /// its diagrams still hold are revealed first (D-237, D-250).
    pub(super) fn maybe_finish_reply(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let finished = self.stop.is_none()
            && self
                .reply
                .as_ref()
                .is_some_and(|reply| !reply.in_flight && reply.done)
            && self.voice.is_silent()
            && self.queue.is_empty();
        if !finished {
            return;
        }
        if self.queue_leftovers() {
            self.advance_ops(now, effects);
            // Waiting for its drawing slot: `advance_ops` wakes the core then.
            if !self.queue.is_empty() {
                return;
            }
        }
        self.reply_asked = self
            .voice
            .last_played_caption()
            .is_some_and(cues::ends_on_question);
        let heard = self.voice.heard();
        self.commit_heard(heard, false);
        self.show_overview(effects);
        self.end_reply();
        match std::mem::replace(&mut self.state, ConversationState::Idle) {
            ConversationState::Speaking | ConversationState::Thinking | ConversationState::Idle => {
                self.maybe_end_session(effects);
            }
            // The user started talking just as BoardAI finished: their words are a new turn. An
            // end-of-session line that drained under them still played: their words decide
            // whether the session ends (a goodbye) or goes on (a real question).
            ConversationState::Ducked(barge_in) => {
                self.end_after_words();
                self.set_gain(Gain::FULL, effects);
                self.hand_floor(barge_in.into_utterance(), effects);
            }
            state @ ConversationState::Listening(_) => {
                self.end_after_words();
                self.state = state;
            }
            ConversationState::Muted => {
                self.keep_session();
                self.state = ConversationState::Muted;
            }
        }
    }
}
