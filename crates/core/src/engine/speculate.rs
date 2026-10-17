//! The speculative start (D-262, revisiting D-155): on a hosted talker, a finished early final
//! sends the turn's request before the detector confirms the end of the turn, and nothing of it
//! shows until the final confirms it (LLD §2.2).
//!
//! Speech in decodes the final after 96 ms of quiet inside speech and sends it as `EarlyFinal`;
//! then either the speaker goes on (`VoiceResumed`), or `VoiceEnd` and the same text as the final
//! follow about 160 ms later. When the early final sounds finished, the turn is ended in a copy
//! of the core and only the copy's request leaves. The core itself stays in `Listening`, so the
//! orb, the captions and the voice show nothing, and the stream's inputs are held.
//!
//! When the turn really ends, the core ends it as it always does. If that asks for the very same
//! request, it is the one already running: the new `Talk` is dropped and the held inputs replay.
//! Anything else cancels the stream before any of it is seen: speech that resumed, a different
//! request, a state the turn left without asking, the deadline, too many held inputs. So the turn
//! ends exactly as it would without a speculation; only its request can leave sooner.

use crate::conversation::ConversationState;
use crate::conversation::cues::{self, Completeness};
use crate::effect::{Effect, Milestone, TalkRequest};
use crate::history::RawAssistant;
use crate::input::StopReason;
use crate::limits::{SPECULATION_HELD_INPUTS_MAX, SPECULATION_MAX_MS};
use crate::time::SessionTime;
use crate::tools::ToolInvocation;

use super::{Core, MicMode, RequestStart, milestone};

/// What the talker's stream sends the core, as `step` hands it on.
#[derive(Clone, Debug)]
pub(super) enum TalkerInput {
    Delta(String),
    ToolCall(ToolInvocation),
    Done {
        stop: StopReason,
        raw: Option<RawAssistant>,
    },
}

/// A request that left on an early final, waiting for its turn to end.
#[derive(Clone, Debug)]
pub(super) struct Speculation {
    /// What was sent: the turn's own request must be exactly this to take its stream over.
    request: TalkRequest,
    /// The stream's inputs, in order, replayed once the turn is confirmed.
    held: Vec<TalkerInput>,
    deadline: SessionTime,
}

impl Core {
    /// A finished early final starts the turn's request now, if the talker is hosted and the
    /// turn would ask for one if it ended here.
    pub(super) fn on_early_final(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) {
        // One is left only if its `VoiceResumed` never came: it no longer stands either.
        self.cancel_speculation(effects);
        // While the user draws no request may run (I14), even one their question would start.
        if self.config.request_start != RequestStart::OnEarlyFinal
            || self.config.mic == MicMode::PushToTalk
            || self.is_drawing()
        {
            return;
        }
        // Once the voice ended, the final follows at once and ends the turn without help.
        let ConversationState::Listening(utterance) = &self.state else {
            return;
        };
        if !utterance.is_voice_active() {
            return;
        }
        let Some(request) = self.request_if_ended(now, text) else {
            return;
        };
        effects.push(Effect::Talk {
            request: request.clone(),
        });
        milestone(effects, Milestone::SpeculativeRequest);
        let deadline = now.plus_ms(SPECULATION_MAX_MS);
        self.wake(deadline, effects);
        self.speculation = Some(Box::new(Speculation {
            request,
            held: Vec::new(),
            deadline,
        }));
    }

    /// The request the turn would send if it ended now on `text`, worked out on a copy: the
    /// core itself changes nothing until the final confirms it.
    fn request_if_ended(&self, now: SessionTime, text: &str) -> Option<TalkRequest> {
        let mut fork = self.clone();
        let ConversationState::Listening(utterance) = &mut fork.state else {
            return None;
        };
        utterance.transcript(text, true);
        if cues::completeness(&utterance.text()) != Completeness::Finished {
            return None;
        }
        let mut forked = Vec::new();
        fork.end_turn(now, &mut forked);
        let mut requests = forked.into_iter().filter_map(|effect| {
            if let Effect::Talk { request } = effect {
                Some(request)
            } else {
                None
            }
        });
        match (requests.next(), requests.next()) {
            (Some(request), None) => Some(request),
            (Some(_), Some(_)) | (None, _) => None,
        }
    }

    /// A talker input: held while its turn is unconfirmed, else stepped.
    pub(super) fn on_talker_input(
        &mut self,
        now: SessionTime,
        input: TalkerInput,
        effects: &mut Vec<Effect>,
    ) {
        let Some(speculation) = self.speculation.as_mut() else {
            self.talker_input(now, input, effects);
            return;
        };
        if speculation.held.len() < SPECULATION_HELD_INPUTS_MAX {
            speculation.held.push(input);
        } else {
            self.cancel_speculation(effects);
        }
    }

    fn talker_input(&mut self, now: SessionTime, input: TalkerInput, effects: &mut Vec<Effect>) {
        match input {
            TalkerInput::Delta(text) => self.on_talker_delta(now, &text, effects),
            TalkerInput::ToolCall(invocation) => self.on_tool_call(now, invocation, effects),
            TalkerInput::Done { stop, raw } => self.on_talker_done(now, stop, raw, effects),
        }
    }

    /// Drops the speculation, and its stream with it.
    pub(super) fn cancel_speculation(&mut self, effects: &mut Vec<Effect>) {
        if self.speculation.take().is_some() {
            effects.push(Effect::CancelTalk);
        }
    }

    /// After every step: once the turn has ended (or the core left `Listening` without ending
    /// it, or the user began to draw), the speculation is confirmed if the turn asked for exactly
    /// its request, and cancelled otherwise; past its deadline it is cancelled. `first` is where
    /// this step's effects begin.
    pub(super) fn settle_speculation(
        &mut self,
        now: SessionTime,
        first: usize,
        effects: &mut Vec<Effect>,
    ) {
        let Some(speculation) = self.speculation.as_ref() else {
            return;
        };
        let talks: Vec<usize> = (first..effects.len())
            .filter(|index| matches!(effects[*index], Effect::Talk { .. }))
            .collect();
        // Drawing that began meanwhile ends it too: no stream may run while the user draws (I14).
        let listening = matches!(self.state, ConversationState::Listening(_)) && !self.is_drawing();
        if listening && talks.is_empty() {
            if now >= speculation.deadline {
                self.cancel_speculation(effects);
            }
            return;
        }
        let Some(speculation) = self.speculation.take() else {
            return;
        };
        let same = match talks.as_slice() {
            [index] => matches!(
                &effects[*index],
                Effect::Talk { request } if *request == speculation.request
            ),
            _ => false,
        };
        if !same {
            // Before this step's own request, if any (I5).
            effects.insert(first, Effect::CancelTalk);
            return;
        }
        if let [index] = talks.as_slice() {
            effects.remove(*index);
        }
        for input in speculation.held {
            self.talker_input(now, input, effects);
        }
    }
}
