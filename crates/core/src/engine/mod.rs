//! The engine: [`Core`], the only owner of conversation state (LLD §2.1).
//!
//! The runtime calls three things: [`Core::new`], [`Core::start`] once when the session opens,
//! and [`Core::step`] for every input. `step` never waits and never fails: anything slow becomes
//! an effect now and an input later, and bad input is dropped, counted or answered. The same
//! inputs at the same times always produce the same effects, which is what the conversation
//! simulator and session replay rely on.

mod listen;
mod ops;
mod reply;
mod stop;
pub mod text;

use std::collections::BTreeSet;

use crate::board::Board;
use crate::conversation::ConversationState;
use crate::effect::{Effect, LogRecord, Milestone};
use crate::history::{Conversation, Message};
use crate::ids::{ClauseId, LayerId};
use crate::input::Input;
use crate::limits::ORB_DRAWING_LINGER_MS;
use crate::protocol::{OrbState, PointerTarget, UiCommand, UiEvent};
use crate::surfaces::Surfaces;
use crate::thread::Thread;
use crate::time::SessionTime;
use crate::voice::Voice;
use text::SYNTHESIS_FAILED_DETAIL;

use listen::{ChangeGate, UserSide};
use ops::OpQueue;
use reply::{ContextTracker, Reply};
use stop::PendingStop;

/// How the microphone is used (D-011).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicMode {
    /// Voice activity starts a turn and can barge in.
    OpenMic,
    /// Only speech while the push-to-talk key is held counts.
    PushToTalk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    /// The talker's name as the user knows it, for spoken failure lines ("I lost the connection
    /// to Claude.").
    pub talker_name: String,
    pub mic: MicMode,
    /// Context for the first request ("where we left off", relevant memory), appended as a
    /// system message when the session starts.
    pub session_context: Option<String>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            talker_name: "the model".to_owned(),
            mic: MicMode::OpenMic,
            session_context: None,
        }
    }
}

/// Where the pointer is, and when it last landed on a target (barge-in rule d).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Pointer {
    target: Option<PointerTarget>,
    set_at: Option<SessionTime>,
}

/// Which latency marks this turn has already logged (LLD §8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TurnMarks {
    first_token: bool,
    first_audio: bool,
    first_op: bool,
}

#[derive(Clone, Debug)]
pub struct Core {
    config: CoreConfig,
    state: ConversationState,
    conversation: Conversation,
    user: UserSide,
    reply: Option<Reply>,
    voice: Voice,
    queue: OpQueue,
    board: Board,
    thread: Thread,
    surfaces: Surfaces,
    gate: ChangeGate,
    pointer: Pointer,
    context: ContextTracker,
    stop: Option<PendingStop>,
    marks: TurnMarks,
    orb: Option<OrbState>,
    drawing_until: SessionTime,
    wakes: BTreeSet<SessionTime>,
    push_to_talk_held: bool,
    backchannel_at: Option<SessionTime>,
    next_task: u32,
    /// Where each op of the current turn was anchored; only the simulator reads it (I1).
    #[cfg(feature = "sim")]
    anchors: std::collections::BTreeMap<crate::ids::OpId, (crate::ids::ClauseId, usize)>,
}

impl Core {
    #[must_use]
    pub fn new(config: CoreConfig) -> Self {
        Self {
            config,
            state: ConversationState::Idle,
            conversation: Conversation::new(),
            user: UserSide::default(),
            reply: None,
            voice: Voice::default(),
            queue: OpQueue::default(),
            board: Board::new(),
            thread: Thread::default(),
            surfaces: Surfaces::default(),
            gate: ChangeGate::Closed,
            pointer: Pointer::default(),
            context: ContextTracker::default(),
            stop: None,
            marks: TurnMarks::default(),
            orb: None,
            drawing_until: SessionTime::ZERO,
            wakes: BTreeSet::new(),
            push_to_talk_held: false,
            backchannel_at: None,
            next_task: 0,
            #[cfg(feature = "sim")]
            anchors: std::collections::BTreeMap::new(),
        }
    }

    /// Opens the session: the initial orb and thread, and the session context for the talker.
    pub fn start(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if let Some(context) = self.config.session_context.clone() {
            self.conversation.push(Message::System { text: context });
        }
        effects.push(Effect::Ui(self.thread.event()));
        self.update_orb(now, effects);
    }

    /// The only way state changes. Same inputs at the same times → same effects.
    pub fn step(&mut self, now: SessionTime, input: Input, effects: &mut Vec<Effect>) {
        match input {
            Input::VoiceStart => self.on_voice_start(now, effects),
            Input::VoiceEnd { voiced_ms } => self.on_voice_end(now, voiced_ms, effects),
            Input::Transcript { text, is_final } => {
                self.on_transcript(now, &text, is_final, effects);
            }
            Input::TalkerDelta { text } => self.on_talker_delta(now, &text, effects),
            Input::TalkerToolCall { invocation } => self.on_tool_call(invocation, effects),
            Input::TalkerDone { stop, raw } => self.on_talker_done(now, stop, raw, effects),
            Input::ClauseReady { clause, timings } => {
                if self.voice.ready(clause, timings) && self.state == ConversationState::Thinking {
                    self.state = ConversationState::Speaking;
                }
            }
            Input::SynthesisFailed { clause } => self.on_synthesis_failed(clause, effects),
            Input::PlaybackAt { position } => {
                if self.voice.playback_at(now, position) && !self.marks.first_audio {
                    self.marks.first_audio = true;
                    milestone(effects, Milestone::FirstAudio);
                }
            }
            Input::PlaybackDrained => self.voice.drained(),
            Input::VoiceStopped { last } => {
                if self.stop.is_some() {
                    self.settle_stop(last, effects);
                }
            }
            Input::ToolResult { id, output } => self.on_tool_result(now, &id, output, effects),
            Input::Harness { task, event } => self.on_harness(task, event, effects),
            Input::Command(UiCommand::Pointer { target }) => {
                self.pointer.set_at = target.is_some().then_some(now);
                self.pointer.target = target;
                self.evaluate_barge_in(now, effects);
            }
            Input::Command(UiCommand::Mic { action }) => self.on_mic(now, action, effects),
            Input::Tick => self.on_tick(now, effects),
        }
        self.after_step(now, effects);
    }

    /// The append-only conversation (I3), for the store and the session digest.
    #[must_use]
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    #[must_use]
    pub fn state(&self) -> &ConversationState {
        &self.state
    }

    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[must_use]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// A layer as an `.excalidraw` document, for the session folder (LLD §3.6).
    #[must_use]
    pub fn export_excalidraw(&self, layer: LayerId) -> Option<String> {
        self.board
            .layer(layer)
            .map(crate::board::excalidraw::export_layer)
    }

    /// The anchor `(clause, speakable offset)` of an op of the current turn, for the simulator's
    /// I1 check.
    #[cfg(feature = "sim")]
    #[must_use]
    pub fn op_anchor(&self, op: crate::ids::OpId) -> Option<(crate::ids::ClauseId, usize)> {
        self.anchors.get(&op).copied()
    }

    /// The voice can't say this clause. Skipping it keeps the reply moving (a clause that never
    /// played would otherwise hold the turn open forever); the user reads what wasn't said.
    fn on_synthesis_failed(&mut self, clause: ClauseId, effects: &mut Vec<Effect>) {
        let Some(caption) = self.voice.failed(clause) else {
            return;
        };
        effects.push(Effect::Ui(UiEvent::Problem {
            spoken: caption.trim().to_owned(),
            detail: SYNTHESIS_FAILED_DETAIL.to_owned(),
        }));
    }

    fn on_tick(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        self.wakes.retain(|at| *at > now);
        if self
            .stop
            .as_ref()
            .is_some_and(|stop| now >= stop.deadline())
        {
            // The runtime never answered StopVoice: the last reported position is still a
            // position that played, so the heard prefix stays honest.
            self.settle_stop(None, effects);
        }
        self.check_turn_deadlines(now, effects);
        self.evaluate_barge_in(now, effects);
    }

    /// Work that any input can make due: ops whose words have played, captions, the end of the
    /// assistant's turn, a pending harness report, and the orb.
    fn after_step(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        self.advance_ops(now, effects);
        let mut captions = Vec::new();
        self.voice.captions(&mut captions);
        effects.extend(captions.into_iter().map(Effect::Ui));
        self.maybe_finish_reply(effects);
        self.maybe_report(now, effects);
        self.update_orb(now, effects);
    }

    /// Asks for a `Tick` at `at`, once per distinct deadline.
    fn wake(&mut self, at: SessionTime, effects: &mut Vec<Effect>) {
        if self.wakes.insert(at) {
            effects.push(Effect::WakeAt { at });
        }
    }

    /// The orb follows the state. While speaking it shows Drawing from the first stroke until
    /// the last stroke's budget ends plus a linger, so the nib doesn't flicker between ops.
    fn update_orb(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let linger_until = self.drawing_until.plus_ms(ORB_DRAWING_LINGER_MS);
        let base = self.state.orb();
        let state = if base == OrbState::Speaking && now < linger_until {
            OrbState::Drawing
        } else {
            base
        };
        if state == OrbState::Drawing {
            self.wake(linger_until, effects);
        }
        if self.orb != Some(state) {
            self.orb = Some(state);
            effects.push(Effect::Ui(UiEvent::Orb {
                state,
                level_percent: 0,
            }));
        }
    }
}

fn milestone(effects: &mut Vec<Effect>, milestone: Milestone) {
    effects.push(Effect::Log(LogRecord::Milestone { milestone }));
}
