//! The engine: [`Core`], the only owner of conversation state (LLD §2.1).
//!
//! The runtime calls three things: [`Core::new`], [`Core::start`] once when the session opens,
//! and [`Core::step`] for every input. `step` never waits and never fails: anything slow becomes
//! an effect now and an input later, and bad input is dropped, counted or answered. The same
//! inputs at the same times always produce the same effects, which is what the conversation
//! simulator and session replay rely on.

mod camera;
mod draw;
mod gate;
mod listen;
mod memory;
mod nudge;
mod ops;
pub mod pages;
mod reply;
mod repo_names;
mod sources;
mod stage;
mod stop;
pub mod text;
pub mod views;
mod written;

use std::collections::BTreeSet;

use crate::board::Board;
use crate::conversation::ConversationState;
use crate::effect::{Effect, Gain, LogRecord, Milestone};
use crate::history::{Conversation, Message};
use crate::ids::{ClauseId, LayerId};
use crate::input::Input;
use crate::limits::{ORB_DRAWING_LINGER_MS, SUGGESTION_MAX_BYTES};
use crate::narration::tags::bounded;
use crate::protocol::{OrbState, PointerTarget, UiCommand, UiEvent};
use crate::surfaces::Surfaces;
use crate::thread::Thread;
use crate::time::SessionTime;
use crate::views::ViewKind;
use crate::voice::Voice;
use text::SYNTHESIS_FAILED_DETAIL;

use camera::Cameras;
use draw::Drawing;
use gate::ChangeGate;
use listen::UserSide;
use memory::Memory;
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
    /// The idle line under the orb ("Pick up Raft log compaction where we left off?"), from the
    /// last session's recap. Shown at start, cleared when the user first speaks.
    pub suggestion: Option<String>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            talker_name: "the model".to_owned(),
            mic: MicMode::OpenMic,
            session_context: None,
            suggestion: None,
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
    /// When the last stroke's budget ends; `None` until anything is drawn.
    drawing_until: Option<SessionTime>,
    cameras: Cameras,
    /// What the user asked in the turn being answered, for a detour card's subtitle; `None` when
    /// the reply answers no question of theirs.
    turn_question: Option<String>,
    /// BoardAI's last reply played to its end on a question (an offer: "Want me to keep going on
    /// how it evicts?"), so "keep going" answers it and closes no detour (D-147).
    reply_asked: bool,
    /// The idle suggestion is on screen.
    suggesting: bool,
    wakes: BTreeSet<SessionTime>,
    push_to_talk_held: bool,
    /// A voice onset ducked the voice tentatively; the gain comes back at this time unless a
    /// barge-in takes the duck over first.
    onset_duck_until: Option<SessionTime>,
    /// The gain the voice was last told, so a stop that had nothing to fade still undoes a duck.
    gain: Gain,
    next_task: u32,
    /// What this session captured, the memory controls' state, and a proposal awaiting a yes.
    memory: Memory,
    /// The view on screen, if any (LLD §3.7).
    view: Option<ViewKind>,
    /// The library on screen, and the one a replay opened over: closing that replay shows it
    /// again (D-146).
    library_return: views::LibraryReturn,
    /// Whether "end the session" was said, and whether the session has ended (D-144).
    ending: stage::SessionEnding,
    /// Views read for tool calls the talker wrote as tags (D-143).
    written_reads: written::WrittenReads,
    /// Whether the user is drawing, and what BoardAI keeps while it watches (artboard 13).
    drawing: Drawing,
    /// What the repo tools returned, for checking a module map's labels (D-180).
    repo_names: repo_names::RepoNames,
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
            drawing_until: None,
            cameras: Cameras::default(),
            turn_question: None,
            reply_asked: false,
            suggesting: false,
            wakes: BTreeSet::new(),
            push_to_talk_held: false,
            onset_duck_until: None,
            gain: Gain::FULL,
            next_task: 0,
            memory: Memory::default(),
            view: None,
            library_return: views::LibraryReturn::default(),
            ending: stage::SessionEnding::Continuing,
            written_reads: written::WrittenReads::default(),
            drawing: Drawing::Off,
            repo_names: repo_names::RepoNames::default(),
            #[cfg(feature = "sim")]
            anchors: std::collections::BTreeMap::new(),
        }
    }

    /// Opens the session: the initial orb and thread, the idle suggestion, and the session
    /// context for the talker.
    pub fn start(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if let Some(context) = self.config.session_context.clone() {
            self.conversation.push(Message::System { text: context });
        }
        effects.push(Effect::Ui(self.thread.event()));
        let suggestion = self
            .config
            .suggestion
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| bounded(text, SUGGESTION_MAX_BYTES));
        if let Some(text) = suggestion {
            self.suggesting = true;
            effects.push(Effect::Ui(UiEvent::Suggestion { text: Some(text) }));
        }
        self.update_orb(now, effects);
    }

    /// The only way state changes. Same inputs at the same times → same effects.
    pub fn step(&mut self, now: SessionTime, input: Input, effects: &mut Vec<Effect>) {
        let first = effects.len();
        match input {
            Input::VoiceOnset => self.on_voice_onset(now, effects),
            Input::VoiceStart => self.on_voice_start(now, effects),
            Input::VoiceEnd {
                voiced_ms,
                silence_ms,
            } => self.on_voice_end(now, voiced_ms, silence_ms, effects),
            Input::Transcript { text, is_final } => {
                self.on_transcript(now, &text, is_final, effects);
            }
            Input::TalkerDelta { text } => self.on_talker_delta(now, &text, effects),
            Input::TalkerToolCall { invocation } => self.on_tool_call(now, invocation, effects),
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
            Input::Command(UiCommand::Stop) => self.on_stop(now, effects),
            Input::Command(UiCommand::CloseView) => self.close_view(effects),
            Input::Command(UiCommand::OpenSource { path, line }) => {
                self.open_source(&path, line, effects);
            }
            Input::Command(UiCommand::Draw { action }) => self.on_draw(now, action, effects),
            Input::Tick => self.on_tick(now, effects),
            Input::MemoryFailed { change, detail } => {
                self.on_memory_failed(change, &detail, effects);
            }
        }
        self.after_step(now, effects);
        self.yield_view(first, effects);
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

    /// Whether the user said "don't remember this session" (I10).
    #[must_use]
    pub fn is_off_record(&self) -> bool {
        self.memory.is_off_record()
    }

    /// A layer as an `.excalidraw` document.
    #[must_use]
    pub fn export_excalidraw(&self, layer: LayerId) -> Option<String> {
        self.board
            .layer(layer)
            .map(crate::board::excalidraw::export_layer)
    }

    /// The whole session as one `.excalidraw` document with a frame per layer (the main board,
    /// then each detour in the order it opened), for the session folder (LLD §3.6).
    #[must_use]
    pub fn export_session_excalidraw(&self) -> String {
        crate::board::excalidraw::export_board(&self.board)
    }

    /// The user spoke for the first time: the idle suggestion has done its job.
    fn clear_suggestion(&mut self, effects: &mut Vec<Effect>) {
        if std::mem::take(&mut self.suggesting) {
            effects.push(Effect::Ui(UiEvent::Suggestion { text: None }));
        }
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
        self.release_onset_duck(now, effects);
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
        // Before the reply can finish: its last clause may be the change question that just
        // played.
        self.arm_change_gate(now);
        self.maybe_finish_reply(effects);
        self.maybe_report(now, effects);
        self.settle_hint(effects);
        self.update_orb(now, effects);
    }

    /// Sets the voice's gain, remembering it.
    fn set_gain(&mut self, gain: Gain, effects: &mut Vec<Effect>) {
        self.gain = gain;
        effects.push(Effect::Duck { gain });
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
        let linger_until = self
            .drawing_until
            .map(|until| until.plus_ms(ORB_DRAWING_LINGER_MS));
        // While the user draws, a quiet BoardAI is listening and watching (artboard 13).
        let base = match self.state.orb() {
            OrbState::Idle if self.is_drawing() => OrbState::Listening,
            state @ (OrbState::Idle
            | OrbState::Listening
            | OrbState::Thinking
            | OrbState::Speaking
            | OrbState::Drawing
            | OrbState::Muted) => state,
        };
        let state = match linger_until {
            Some(until) if base == OrbState::Speaking && now < until => OrbState::Drawing,
            Some(_) | None => base,
        };
        if let (OrbState::Drawing, Some(until)) = (state, linger_until) {
            self.wake(until, effects);
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
