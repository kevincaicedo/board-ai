//! Views summoned by voice (LLD §3.7): the library, the Now list, memory, a past board, and a
//! replay. The talker asks for one with a view tool; the runtime reads it from the store; the
//! core shows it and tells the talker what is on screen.
//!
//! One view is open at a time. It closes when the user says "close that", presses Esc, or when
//! the live stage is used again (a stroke, a detour or a card): the board is the core's, and a
//! view never hides what is being explained. "Open settings" opens the settings screen, which
//! the UI and the app own.
//!
//! A replay opened over the library returns to it when it closes (D-146): by Esc, by "close
//! that", or when it has played to its end (the UI then sends `CloseView`). Opened from the live
//! stage, it returns to the live stage.

use std::fmt::Write as _;

use crate::conversation::cues;
use crate::effect::Effect;
use crate::engine::Core;
use crate::limits::{LABEL_MAX_BYTES, VIEW_TALKER_ITEMS_MAX};
use crate::narration::tags::bounded;
use crate::protocol::{DrawOp, UiEvent};
use crate::views::{NoteRow, PastLayer, SessionCard, TaskRow, View, ViewKind};

/// The library while it is on screen, and the library a replay was opened over: closing that
/// replay shows it again (D-146). A view as read, with the day it was read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct LibraryReturn {
    on_screen: Option<Box<(View, String)>>,
    under_replay: Option<Box<(View, String)>>,
}

impl Core {
    /// Shows a view the store read, replacing any open one. A replay replacing the library (or
    /// a replay that replaced it) keeps the library to go back to.
    pub(super) fn show_view(&mut self, view: View, today: String, effects: &mut Vec<Effect>) {
        let kind = view.kind();
        let back = &mut self.library_return;
        back.under_replay = if kind == ViewKind::Replay {
            match self.view {
                Some(ViewKind::Library) => back.on_screen.take(),
                Some(ViewKind::Replay) => back.under_replay.take(),
                Some(ViewKind::Tasks | ViewKind::Memory | ViewKind::PastSession) | None => None,
            }
        } else {
            None
        };
        back.on_screen =
            (kind == ViewKind::Library).then(|| Box::new((view.clone(), today.clone())));
        self.view = Some(kind);
        effects.push(Effect::Ui(UiEvent::ViewShow { view, today }));
    }

    /// Esc, or a replay that played to its end. The talker hears about it with its next request.
    pub(super) fn close_view(&mut self, effects: &mut Vec<Effect>) {
        if let Some(kind) = self.dismiss_view(effects) {
            self.context.notice(format!(
                "The user closed the {} view.{}",
                kind_name(kind),
                self.back_on_screen()
            ));
        }
    }

    /// What is on screen after a view closed, for the talker: the library a replay went back
    /// to, or nothing.
    fn back_on_screen(&self) -> &'static str {
        if self.view == Some(ViewKind::Library) {
            " The library it was opened from is on screen again."
        } else {
            ""
        }
    }

    /// "Close that" with a view open: closed with a nod, no words (returns true).
    pub(super) fn close_view_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        if self.view.is_none() || !cues::is_close_view(text) {
            return false;
        }
        if let Some(kind) = self.dismiss_view(effects) {
            effects.push(Effect::Ui(UiEvent::Acknowledge));
            self.context.notice(format!(
                "The user said \"{text}\" and the {} view closed.{}",
                kind_name(kind),
                self.back_on_screen()
            ));
        }
        true
    }

    /// "Open settings": the UI opens the settings screen, with a nod and no words (returns
    /// true). The talker hears about it with its next request.
    pub(super) fn open_settings_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        if !cues::is_open_settings(text) {
            return false;
        }
        effects.push(Effect::Ui(UiEvent::OpenSettings));
        effects.push(Effect::Ui(UiEvent::Acknowledge));
        self.context.notice(format!(
            "The user said \"{text}\" and the settings screen opened over the board."
        ));
        true
    }

    /// Closes the open view; a replay opened over the library shows the library again instead.
    fn dismiss_view(&mut self, effects: &mut Vec<Effect>) -> Option<ViewKind> {
        let kind = self.view.take()?;
        self.library_return.on_screen = None;
        let back = match kind {
            ViewKind::Replay => self.library_return.under_replay.take(),
            ViewKind::Library | ViewKind::Tasks | ViewKind::Memory | ViewKind::PastSession => None,
        };
        match back {
            Some(library) => {
                let (view, today) = *library;
                self.show_view(view, today, effects);
            }
            None => effects.push(Effect::Ui(UiEvent::ViewDismiss)),
        }
        Some(kind)
    }

    /// The live stage is used again in this step: the open view goes away just before it.
    pub(super) fn yield_view(&mut self, first: usize, effects: &mut Vec<Effect>) {
        if self.view.is_none() {
            return;
        }
        let uses_stage = |effect: &Effect| {
            matches!(
                effect,
                Effect::Ui(
                    UiEvent::OpStart { .. }
                        | UiEvent::LayerPush { .. }
                        | UiEvent::SurfaceShow { .. }
                        | UiEvent::DrawMode { drawing: true }
                        | UiEvent::UserShape { .. }
                        | UiEvent::PageTurn { .. }
                )
            )
        };
        let Some(offset) = effects
            .get(first..)
            .and_then(|new| new.iter().position(uses_stage))
        else {
            return;
        };
        self.view = None;
        self.library_return = LibraryReturn::default();
        effects.insert(first + offset, Effect::Ui(UiEvent::ViewDismiss));
    }

    /// The open view, if any.
    #[must_use]
    pub fn view(&self) -> Option<ViewKind> {
        self.view
    }
}

fn kind_name(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::Library => "library",
        ViewKind::Tasks => "Now list",
        ViewKind::Memory => "memory",
        ViewKind::PastSession => "past session",
        ViewKind::Replay => "replay",
    }
}

/// What the talker reads about a view now on screen: bounded lists, dates as written, and how
/// to answer. `today` lets it say "from today".
#[must_use]
pub fn describe(view: &View, today: &str) -> String {
    let mut text = String::new();
    match view {
        View::Library {
            sessions,
            more_sessions,
        } => describe_library(&mut text, sessions, *more_sessions, today),
        View::Tasks {
            project,
            tasks,
            more_tasks,
            ..
        } => describe_tasks(&mut text, project.as_deref(), tasks, *more_tasks, today),
        View::Memory {
            about_you,
            systems,
            latest,
            ..
        } => {
            let _ = writeln!(text, "On screen now: what you remember. Today is {today}.");
            text.push_str("About the user:\n");
            for row in about_you.iter().take(VIEW_TALKER_ITEMS_MAX) {
                let _ = writeln!(text, "- {}", label(&row.text));
            }
            text.push_str("About their systems:\n");
            for row in systems.iter().take(VIEW_TALKER_ITEMS_MAX) {
                let _ = writeln!(text, "- {}", label(&row.text));
            }
            if let Some(latest) = latest {
                let _ = writeln!(text, "Latest session: {}.", card(&latest.session));
            }
        }
        View::PastSession {
            session,
            recap,
            layers,
            ..
        } => describe_past(&mut text, session, recap, layers),
        View::Replay { session } => {
            let _ = writeln!(
                text,
                "On screen now: {} replaying from the start. Say one short sentence, then let \
                 it play.",
                card(session)
            );
        }
    }
    if let View::Tasks { notes, .. } | View::Memory { notes, .. } = view {
        describe_notes(&mut text, notes);
        text.push_str("Answer in a sentence or two; don't read the whole list out.\n");
    }
    text
}

fn describe_tasks(
    text: &mut String,
    project: Option<&str>,
    tasks: &[TaskRow],
    more: u32,
    today: &str,
) {
    let scope = project
        .map(|name| format!(" for \"{}\"", label(name)))
        .unwrap_or_default();
    let total = tasks.len() + usize::try_from(more).unwrap_or(usize::MAX);
    let _ = writeln!(
        text,
        "On screen now: the Now list{scope}, {total} open tasks. Today is {today}."
    );
    for task in tasks.iter().take(VIEW_TALKER_ITEMS_MAX) {
        let from = task
            .from
            .as_ref()
            .map(|from| format!(", from \"{}\" ({})", label(&from.title), from.date))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "- {} [{}]{from}",
            label(&task.title),
            label(&task.project)
        );
    }
    list_more(text, tasks.len(), "tasks");
}

fn describe_past(text: &mut String, session: &SessionCard, recap: &str, layers: &[PastLayer]) {
    let _ = writeln!(
        text,
        "On screen now: the board of {}, as it ended. It is a picture of that session: the \
         live board is under it, and it closes when you draw.",
        card(session)
    );
    let labels: Vec<String> = layers
        .iter()
        .flat_map(|layer| layer.ops.iter())
        .filter_map(|op| match op {
            DrawOp::Node { label: name, .. } | DrawOp::Participant { label: name, .. } => {
                Some(label(name))
            }
            DrawOp::Cells { label: name, .. } if !name.is_empty() => Some(label(name)),
            DrawOp::Cells { .. }
            | DrawOp::Edge { .. }
            | DrawOp::Annotate { .. }
            | DrawOp::Strike { .. }
            | DrawOp::Trace { .. }
            | DrawOp::Message { .. }
            | DrawOp::Write { .. } => None,
        })
        .take(VIEW_TALKER_ITEMS_MAX * 3)
        .collect();
    if !labels.is_empty() {
        let _ = writeln!(text, "On it: {}.", labels.join(", "));
    }
    if !recap.is_empty() {
        let _ = writeln!(text, "Recap: {}", label(recap));
    }
    text.push_str("Say in a sentence or two what it shows and where it ended.\n");
}

fn describe_library(text: &mut String, sessions: &[SessionCard], more: u32, today: &str) {
    if sessions.is_empty() {
        text.push_str("On screen now: the library. There are no past sessions yet.\n");
        return;
    }
    let total = sessions.len() + usize::try_from(more).unwrap_or(usize::MAX);
    let _ = writeln!(
        text,
        "On screen now: the library, {total} past sessions, newest first. Today is {today}."
    );
    for (number, session) in sessions.iter().take(VIEW_TALKER_ITEMS_MAX).enumerate() {
        let _ = writeln!(text, "{}. {} id {}", number + 1, card(session), session.id);
    }
    list_more(text, sessions.len(), "sessions");
    text.push_str(
        "Answer in one short sentence; the user can ask to open one (open_session) or play it \
         (play_session).\n",
    );
}

fn describe_notes(text: &mut String, notes: &[NoteRow]) {
    if notes.is_empty() {
        return;
    }
    text.push_str("Recent notes:\n");
    for note in notes.iter().take(VIEW_TALKER_ITEMS_MAX) {
        let _ = writeln!(text, "- {} ({})", label(&note.text), note.date);
    }
}

fn list_more(text: &mut String, shown: usize, what: &str) {
    if shown > VIEW_TALKER_ITEMS_MAX {
        let _ = writeln!(
            text,
            "({} more {what} on screen)",
            shown - VIEW_TALKER_ITEMS_MAX
        );
    }
}

/// `"Raft · log compaction" (2026-10-12, 31 min)`.
fn card(session: &SessionCard) -> String {
    let minutes = u64::from(session.duration_ms).div_ceil(60_000);
    format!(
        "\"{}\" ({}, {minutes} min)",
        label(&session.title),
        session.date
    )
}

fn label(text: &str) -> String {
    bounded(text, LABEL_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::describe;
    use crate::geometry::Rect;
    use crate::ids::{ElementId, OpId};
    use crate::protocol::{DrawOp, NodeShape};
    use crate::views::{PastLayer, SessionCard, TaskRow, View};

    fn session(title: &str) -> SessionCard {
        SessionCard {
            id: "2026-10-12-093000".to_owned(),
            title: title.to_owned(),
            date: "2026-10-12".to_owned(),
            duration_ms: 31 * 60_000,
            cover: Vec::new(),
        }
    }

    #[test]
    fn the_talker_reads_what_is_on_screen_bounded() {
        let sessions = (0..20).map(|n| session(&format!("Session {n}"))).collect();
        let library = View::Library {
            sessions,
            more_sessions: 3,
        };
        let text = describe(&library, "2026-10-15");
        assert!(text.contains("23 past sessions") && text.contains("Today is 2026-10-15"));
        assert!(text.contains("1. \"Session 0\" (2026-10-12, 31 min) id 2026-10-12-093000"));
        assert!(!text.contains("Session 9\"") && text.contains("(12 more sessions on screen)"));
        let tasks = View::Tasks {
            project: Some("Harness".into()),
            tasks: vec![TaskRow {
                id: "t1".into(),
                title: "Check Redis TTLs".into(),
                project: "Harness".into(),
                from: None,
            }],
            more_tasks: 0,
            notes: Vec::new(),
        };
        assert!(describe(&tasks, "2026-10-15").contains("- Check Redis TTLs [Harness]"));
        let empty = View::Library {
            sessions: Vec::new(),
            more_sessions: 0,
        };
        assert!(describe(&empty, "2026-10-15").contains("no past sessions yet"));
    }

    #[test]
    fn a_past_board_is_described_by_its_labels() {
        let node = DrawOp::Node {
            id: ElementId::parse("leader").unwrap_or_else(|| unreachable!()),
            op: OpId(0),
            shape: NodeShape::Service,
            label: "leader".into(),
            rect: Rect::default(),
            stack: 1,
            fill: None,
            link: None,
        };
        let past = View::PastSession {
            session: session("Raft · log compaction"),
            recap: "A snapshot replaces the log.".into(),
            layers: vec![PastLayer {
                title: None,
                ops: vec![node],
            }],
            ops_left_out: 0,
        };
        let text = describe(&past, "2026-10-15");
        assert!(text.contains("On it: leader.") && text.contains("Recap: A snapshot"));
    }
}
