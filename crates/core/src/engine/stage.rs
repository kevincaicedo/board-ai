//! The stage itself: ending the session by voice (D-144), going back to the main board from a
//! detour (D-147), and the recap leaving when the topic moves on (D-148, PRODUCT §7.2).
//!
//! - **"End the session"** is answered by the core in two words, without the talker. Once that
//!   line has played, the core sends `SessionEnd`; the orchestrator, which owns the session's
//!   life, ends it there (its digest is written) and the next words open a new one. Words over
//!   the line's last word end it too unless they are a real question ("thanks, bye" isn't). While
//!   a harness task runs, ending would stop it partway, so BoardAI says so first and ends the
//!   session (stopping the task) only when the words come again in the next utterance.
//! - **Controls joined in one breath** ("don't remember this session, we can stop here") are
//!   each carried out, ending the session last (`joined_cues`).
//! - **"Back to the main board"**, or **"go on"** once a detour's answer has ended, closes every
//!   open detour before the talker hears the words: the board comes back whether or not the
//!   talker ever writes `<resume/>` (in the owner's session two detours never closed). The
//!   talker still answers, with the bridge sentence a resume would get. "Go on" said over the
//!   detour's answer asks for more of the detour, so it closes nothing, and so does "keep going"
//!   after an answer that ended on an offer ("Want me to keep going on how it evicts?").
//! - **The recap** sums a topic up. When a new turn starts (the next question, or a harness
//!   report), the topic has moved on and the recap leaves. Code and compare cards stay: the
//!   next question is often about them, and they leave by `close` or when two newer cards come.

use crate::conversation::joined_cues::{self, JoinedCue};
use crate::conversation::stage_cues::{self, MainBoardCue};
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::text::{
    SESSION_ENDED_LINE, SESSION_SAVED_LINE, still_running_line, stopped_line,
};
use crate::ids::LayerId;
use crate::narration::ops::CloseTarget;
use crate::protocol::UiEvent;
use crate::time::SessionTime;

/// Where the session's end stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionEnding {
    Continuing,
    /// "End the session" was said while a harness task runs, and BoardAI said ending would stop
    /// it partway: the next utterance ends the session only if it says so again.
    Confirming,
    /// "End the session" was answered: the session ends once that line has played.
    AfterReply,
    /// The line played to its end while the user was talking over its tail: the session ends
    /// when their words end, unless they are a real question.
    AfterWords,
    /// `SessionEnd` went out: the orchestrator ends the session and drops this core, so nothing
    /// new starts here (a harness report waiting for the floor would be cut off at once).
    Ended,
}

impl Core {
    /// "End the session", "we can stop here": answered here with a short line (returns true).
    /// The session ends once the line has played. With a harness task running, the line says
    /// ending would stop it, and the session ends only when the words come again: then every
    /// running task is cancelled, and the line says so.
    pub(super) fn end_session_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        if !stage_cues::is_end_session(text) {
            return false;
        }
        if self.ending != SessionEnding::Confirming
            && let Some(kind) = self.context.running_task()
        {
            let line = still_running_line(kind);
            self.context.notice(format!(
                "The user said \"{text}\" while a harness task was running. BoardAI said \
                 \"{line}\" The session ends, stopping the task, only if they say so again."
            ));
            self.say(&line, effects);
            self.ending = SessionEnding::Confirming;
            return true;
        }
        let end_line = if self.memory.is_off_record() {
            SESSION_ENDED_LINE
        } else {
            SESSION_SAVED_LINE
        };
        let stopped = self.context.stop_all();
        let line = if stopped.is_empty() {
            end_line.to_owned()
        } else {
            let kinds: Vec<_> = stopped.iter().map(|(_, kind)| kind.clone()).collect();
            stopped_line(&kinds, end_line)
        };
        for (task, _) in stopped {
            effects.push(Effect::CancelHarness { task });
        }
        self.say(&line, effects);
        self.ending = SessionEnding::AfterReply;
        true
    }

    /// The words that came after BoardAI's line about ending, before anything else hears them.
    /// After a warning about a running task, anything but "end the session" keeps the session.
    /// After "Session saved." played to its end under these words, a goodbye ends it now
    /// (returns true) and a real question keeps it.
    pub(super) fn words_after_ending(&mut self, said: &str, effects: &mut Vec<Effect>) -> bool {
        match self.ending {
            SessionEnding::Confirming => {
                if !asks_to_end(said) {
                    self.ending = SessionEnding::Continuing;
                }
                false
            }
            SessionEnding::AfterWords => {
                if stage_cues::is_farewell(said) {
                    self.ending = SessionEnding::AfterReply;
                    self.maybe_end_session(effects);
                    return true;
                }
                self.ending = SessionEnding::Continuing;
                false
            }
            SessionEnding::Continuing | SessionEnding::AfterReply | SessionEnding::Ended => false,
        }
    }

    /// Controls said in one breath ("don't remember this session, we can stop here"): each is
    /// carried out, ending the session last. True when the core answered; a spoken cancel alone
    /// says nothing, so the words then go on to the talker, which confirms it.
    pub(super) fn joined_controls(&mut self, said: &str, effects: &mut Vec<Effect>) -> bool {
        let Some(controls) = joined_cues::joined(said) else {
            return false;
        };
        let mut answered = false;
        let mut end = None;
        for control in controls {
            answered |= match control.cue {
                JoinedCue::Memory(_) => self.memory_control(control.words, effects),
                JoinedCue::CloseView => self.close_view_by_voice(control.words, effects),
                JoinedCue::OpenSettings => self.open_settings_by_voice(control.words, effects),
                JoinedCue::Cancel(_) => {
                    self.cancel_by_voice(control.words, effects);
                    false
                }
                JoinedCue::EndSession => {
                    end = Some(control.words);
                    false
                }
            };
        }
        if let Some(words) = end {
            answered |= self.end_session_by_voice(words, effects);
        }
        answered
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

    /// The end-of-session line played to its end, with no stop, while the user was already
    /// talking: their words decide (`words_after_ending`).
    pub(super) fn end_after_words(&mut self) {
        if self.ending == SessionEnding::AfterReply {
            self.ending = SessionEnding::AfterWords;
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
        // "Go on" over the detour's answer asks for more of it, and so does "keep going" after
        // an answer that ended on an offer ("Want me to keep going on how it evicts?").
        match stage_cues::main_board_cue(text) {
            Some(MainBoardCue::Back) => {}
            Some(MainBoardCue::Continue) if !interrupted && !self.reply_asked => {}
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

/// "End the session", alone or joined to other controls.
pub(super) fn asks_to_end(said: &str) -> bool {
    stage_cues::is_end_session(said)
        || joined_cues::joined(said).is_some_and(|controls| {
            controls
                .iter()
                .any(|control| control.cue == JoinedCue::EndSession)
        })
}
