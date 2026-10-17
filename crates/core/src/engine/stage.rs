//! The stage itself: ending the session by voice (D-144), going back to the main board from a
//! detour (D-147), and the recap leaving when the topic moves on (D-148, PRODUCT §7.2).
//!
//! - **"End the session"** is answered by the core in two words, without the talker. Once that
//!   line has played, the core sends `SessionEnd`; the orchestrator, which owns the session's
//!   life, ends it there (its digest is written) and the next words open a new one.
//! - **"Back to the main board"**, or **"go on"** once a detour's answer has ended, closes every
//!   open detour before the talker hears the words: the board comes back whether or not the
//!   talker ever writes `<resume/>` (in the owner's session two detours never closed). The
//!   talker still answers, with the bridge sentence a resume would get. "Go on" said over the
//!   detour's answer asks for more of the detour, so it closes nothing.
//! - **The recap** sums a topic up. When a new turn starts (the next question, or a harness
//!   report), the topic has moved on and the recap leaves. Code and compare cards stay: the
//!   next question is often about them, and they leave by `close` or when two newer cards come.

use crate::conversation::stage_cues::{self, MainBoardCue};
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::text::{SESSION_ENDED_LINE, SESSION_SAVED_LINE};
use crate::ids::LayerId;
use crate::narration::ops::CloseTarget;
use crate::protocol::UiEvent;
use crate::time::SessionTime;

/// Where the session's end stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionEnding {
    Continuing,
    /// "End the session" was answered: the session ends once that line has played.
    AfterReply,
    /// `SessionEnd` went out: the orchestrator ends the session and drops this core, so nothing
    /// new starts here (a harness report waiting for the floor would be cut off at once).
    Ended,
}

impl Core {
    /// "End the session", "we can stop here": answered here with a short line (returns true).
    /// The session ends once the line has played.
    pub(super) fn end_session_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        if !stage_cues::is_end_session(text) {
            return false;
        }
        let line = if self.memory.is_off_record() {
            SESSION_ENDED_LINE
        } else {
            SESSION_SAVED_LINE
        };
        self.say(line, effects);
        self.ending = SessionEnding::AfterReply;
        true
    }

    /// The reply that answered "end the session" finished playing: the session ends now. A
    /// reply that was cut off (the user spoke over it, or muted) ends nothing.
    pub(super) fn maybe_end_session(&mut self, effects: &mut Vec<Effect>) {
        if self.ending == SessionEnding::AfterReply {
            self.ending = SessionEnding::Ended;
            effects.push(Effect::Ui(UiEvent::SessionEnd));
        }
    }

    /// The user cut the end-of-session line off: they are still talking, so nothing ends.
    pub(super) fn keep_session(&mut self) {
        if self.ending == SessionEnding::AfterReply {
            self.ending = SessionEnding::Continuing;
        }
    }

    /// The session has ended: this core starts nothing new.
    pub(super) fn has_ended(&self) -> bool {
        self.ending == SessionEnding::Ended
    }

    /// "Back to the main board", or "go on" after a detour's answer: every open detour closes
    /// before the talker hears the words, which then go on to it with a note (returns true).
    /// `interrupted`: the words cut BoardAI off, so "go on" asks for more of the detour.
    pub(super) fn back_to_main_board(
        &mut self,
        now: SessionTime,
        text: &str,
        interrupted: bool,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self.thread.depth() == 0 {
            return false;
        }
        match stage_cues::main_board_cue(text) {
            Some(MainBoardCue::Back) => {}
            Some(MainBoardCue::Continue) if !interrupted => {}
            Some(MainBoardCue::Continue) | None => return false,
        }
        let titles: Vec<String> = self
            .thread
            .detours()
            .items()
            .iter()
            .map(|detour| format!("\"{}\"", detour.title))
            .collect();
        self.close_all_detours(now, effects);
        self.context.notice(format!(
            "The user said \"{text}\", so every open detour closed ({}) and the main board is \
             back; a stroke that froze on it finishes first. Pick the main explanation up where \
             it stopped, with a short bridge sentence (\"So, back to …\"); if it had already \
             finished, say so in a few words and ask what's next. Don't write <resume/>: no \
             detour is open.",
            titles.join(", ")
        ));
        true
    }

    /// Pops every open detour, innermost first, then the stroke frozen on the main board
    /// continues (LLD §2.7 resume). The stack holds at most `DETOUR_DEPTH_MAX` detours.
    fn close_all_detours(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let mut events = Vec::new();
        for _ in 0..self.thread.depth() {
            self.close_detour(&mut events);
        }
        effects.extend(events.into_iter().map(Effect::Ui));
        self.resume_frozen(now, LayerId::MAIN, effects);
    }

    /// A new turn starts: the recap has done its job and leaves.
    pub(super) fn recap_leaves(&mut self, effects: &mut Vec<Effect>) {
        let mut events = Vec::new();
        if self.surfaces.close(CloseTarget::Recap, &mut events).is_ok() {
            effects.extend(events.into_iter().map(Effect::Ui));
        }
    }
}
