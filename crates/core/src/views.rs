//! What the views summoned by voice show (LLD §3.7, PRODUCT §7.4–§7.6): the library of past
//! sessions, the Now list, memory, a past session's board, and a replay.
//!
//! The runtime reads them from the store and answers the talker's view tools with them; the core
//! shows one with `UiEvent::ViewShow` and tells the talker what is on screen. A view is a picture
//! of the vault: the live board stays the core's own (law 1), so a past board is never mixed into
//! it. These types are exported to TypeScript with the rest of the protocol.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::geometry::Rect;
use crate::protocol::{DrawOp, NodeShape};

/// One view at a time; a new one replaces the open one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum View {
    /// Past sessions as cards, newest first (PRODUCT §7.6).
    Library {
        sessions: Vec<SessionCard>,
        /// Older sessions not listed.
        more_sessions: u32,
    },
    /// The Now list (PRODUCT §7.5): open tasks for one project or all, and the latest notes.
    Tasks {
        /// The project asked for; `None` lists every project.
        project: Option<String>,
        tasks: Vec<TaskRow>,
        /// Open tasks past the short list.
        more_tasks: u32,
        notes: Vec<NoteRow>,
    },
    /// What BoardAI remembers: about you, about your systems, recent notes and the latest
    /// session digest (PRODUCT §7.4).
    Memory {
        about_you: Vec<MemoryRow>,
        systems: Vec<MemoryRow>,
        notes: Vec<NoteRow>,
        latest: Option<DigestCard>,
    },
    /// A past session's final board, drawn at once beside its recap (recall). It is shown over
    /// the live board, never merged into it.
    PastSession {
        session: SessionCard,
        /// Two sentences from the session's digest; empty when it has none.
        recap: String,
        /// The layers still open when the session ended: the main board first.
        layers: Vec<PastLayer>,
        /// Ops past the limit, not sent.
        ops_left_out: u32,
    },
    /// A past session played back like a film. The UI loads its log by id through the
    /// transport and plays it; the live session keeps running underneath.
    Replay { session: SessionCard },
}

impl View {
    #[must_use]
    pub fn kind(&self) -> ViewKind {
        match self {
            Self::Library { .. } => ViewKind::Library,
            Self::Tasks { .. } => ViewKind::Tasks,
            Self::Memory { .. } => ViewKind::Memory,
            Self::PastSession { .. } => ViewKind::PastSession,
            Self::Replay { .. } => ViewKind::Replay,
        }
    }
}

/// Which view is open, without its contents (the core's own record; not on the wire).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewKind {
    Library,
    Tasks,
    Memory,
    PastSession,
    Replay,
}

/// A session as a library card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionCard {
    /// The session's id (`<date>-<time>`), stable across renames.
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD`, UTC; empty when unknown.
    pub date: String,
    pub duration_ms: u32,
    /// The main board's nodes, for the soft cover art; empty when nothing was drawn.
    pub cover: Vec<CoverShape>,
}

/// One node of a past board, as the cover art draws it (a blurred pastel shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverShape {
    pub rect: Rect,
    pub shape: NodeShape,
}

/// Where an item came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionRef {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD`, UTC; empty when unknown.
    pub date: String,
}

/// An open task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskRow {
    /// `t27`, or `<path>#<line>` for a task written by hand without an id.
    pub id: String,
    pub title: String,
    pub project: String,
    /// The session it was captured in, when known.
    pub from: Option<SessionRef>,
}

/// A note, by its first line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NoteRow {
    pub id: String,
    pub text: String,
    /// `YYYY-MM-DD`, UTC; empty when unknown.
    pub date: String,
    pub from: Option<SessionRef>,
    /// `path:line` references and other links, as written.
    pub links: Vec<String>,
}

/// A kept memory (`memory/about-you.md` or `memory/facts.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MemoryRow {
    pub id: String,
    pub text: String,
}

/// The latest session digest, as a card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DigestCard {
    pub session: SessionCard,
    pub notes: u32,
    pub tasks: u32,
    pub open_questions: u32,
}

/// One layer of a past board, its ops in drawing order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PastLayer {
    /// The detour's title; `None` for the main board.
    pub title: Option<String>,
    pub ops: Vec<DrawOp>,
}
