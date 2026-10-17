//! Pages (D-177): starting fresh without moving anything drawn (I6).
//!
//! The owner said "clean the current board and start fresh" and nothing happened: the board
//! only grows. A page turn retires the main board whole (`Board::turn_page`) and a fresh, empty
//! one takes its place, with the camera reset. It comes two ways:
//! - **By voice.** "Clear the board", "start fresh", "new page" (`stage_cues::page_cue`) turn the
//!   page before the talker hears anything. Said alone, the core answers in two words, like the
//!   memory controls; leading or ending a request ("clear the board and explain DNS"), the words
//!   go on to the talker with a notice saying the board is fresh.
//! - **By tag.** `<page title="DNS"/>` when the talker moves to a new topic, when its words play.
//!
//! A page turn closes every open detour first (they belong to the old page), clears the plan,
//! and dismisses the cards beside the board. A frozen stroke stays frozen on the page it was
//! drawn on: it belongs to the old topic.

use crate::board::page::PageRefused;
use crate::conversation::stage_cues::{self, PageCue};
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::listen::UserContext;
use crate::ids::LayerId;
use crate::limits::PAGES_PER_SESSION_MAX;
use crate::narration::ops::CloseTarget;
use crate::protocol::UiEvent;

/// What BoardAI says when the user only asked for a clean board.
pub const PAGE_TURNED_LINE: &str = "Fresh page.";

/// What it says when the session has used every page.
pub const PAGES_FULL_LINE: &str = "There are no pages left in this session.";

impl Core {
    /// "Clear the board", "start fresh", "new page": the board turns to a fresh page before the
    /// talker hears the words. Returns true when the words asked for nothing else, so the core
    /// answered them itself; otherwise they go on to the talker with a notice.
    pub(super) fn page_by_voice(
        &mut self,
        text: &str,
        context: &mut UserContext,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(cue) = stage_cues::page_cue(text) else {
            return false;
        };
        let outcome = self.turn_page(None, effects);
        // A frozen stroke and drawing never drawn belong to the old page's topic.
        context.frozen.clear();
        context.dropped_ops = 0;
        let what = match outcome {
            Ok(page) => format!(
                "the board turned to a fresh, empty page (page {page}); earlier pages stay in the \
                 session's record, off the board, and their ids are free again"
            ),
            Err(PageRefused::Empty) => "the board was already empty".to_owned(),
            Err(PageRefused::Full) => format!(
                "the board could not turn to a new page: this session has used all \
                 {PAGES_PER_SESSION_MAX} pages, so the board stays as it is"
            ),
        };
        match cue {
            PageCue::Only => {
                let line = match outcome {
                    Ok(_) | Err(PageRefused::Empty) => PAGE_TURNED_LINE,
                    Err(PageRefused::Full) => PAGES_FULL_LINE,
                };
                self.say(line, effects);
                self.context.notice(format!(
                    "The user said \"{text}\": {what}. BoardAI said \"{line}\"; wait for their \
                     next question."
                ));
                true
            }
            PageCue::WithRequest => {
                self.context.notice(format!(
                    "The user said \"{text}\", so {what}. Answer the rest of their words, drawing \
                     from scratch, and don't write <page/>."
                ));
                false
            }
        }
    }

    /// `<page title="…"/>`, as its words play. False (dropped and counted) when the session has
    /// no pages left.
    pub(super) fn page_by_tag(&mut self, title: Option<&str>, effects: &mut Vec<Effect>) -> bool {
        match self.turn_page(title.map(str::to_owned), effects) {
            Ok(_) | Err(PageRefused::Empty) => true,
            Err(PageRefused::Full) => {
                self.context.notice(format!(
                    "Your <page/> tag was not carried out: this session has used all \
                     {PAGES_PER_SESSION_MAX} pages, so keep drawing on this one."
                ));
                false
            }
        }
    }

    /// Closes the open detours, then turns the page when there is drawing to turn away from.
    /// The old topic's plan and cards go either way, unless the session is out of pages.
    fn turn_page(
        &mut self,
        title: Option<String>,
        effects: &mut Vec<Effect>,
    ) -> Result<u16, PageRefused> {
        let mut events = Vec::new();
        for _ in 0..self.thread.depth() {
            self.close_detour(&mut events);
        }
        let turned = self.board.turn_page(title.clone());
        if let Ok(page) = turned {
            self.cameras.reset(LayerId::MAIN);
            events.push(UiEvent::PageTurn { page, title });
        }
        if matches!(turned, Ok(_) | Err(PageRefused::Empty)) {
            if self.thread.has_context() {
                self.thread.set_plan(Vec::new());
                events.push(self.thread.event());
            }
            // Nothing to close is not a failure here: the board is fresh either way.
            let _ = self.surfaces.close(CloseTarget::All, &mut events);
        }
        effects.extend(events.into_iter().map(Effect::Ui));
        turned
    }
}
