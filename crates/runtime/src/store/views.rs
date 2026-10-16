//! What the store's reads return: view models for the talker tools and the UI's views. They
//! are serde types so the protocol events for the library and the Now list can carry them
//! when the core adds those events.

use std::path::PathBuf;

use boardai_core::protocol::UiEvent;
use boardai_core::views::{CoverShape, DigestCard, MemoryRow, NoteRow};
use serde::{Deserialize, Serialize};

use super::ids::{ItemId, SessionId};
use super::log::SkipCounts;
use super::vault::MemoryFile;

/// The sessions in the library, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub sessions: Vec<LibraryEntry>,
    /// Older sessions past `LIBRARY_ENTRIES_MAX`.
    pub more_sessions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub id: SessionId,
    pub title: String,
    /// RFC 3339, UTC; empty when unknown (a hand-made folder).
    pub started: String,
    /// `YYYY-MM-DD`, UTC.
    pub date: String,
    pub duration_ms: u64,
    /// The board the cover art is rendered from (`sessions/<folder>/board.excalidraw`,
    /// relative to the vault), when the session drew one.
    pub cover: Option<String>,
    /// The cover's node boxes, read from that board for the newest `LIBRARY_COVERS_MAX`
    /// sessions; empty for the rest and when nothing was drawn.
    pub shapes: Vec<CoverShape>,
}

/// What the memory view shows (PRODUCT §7.4), newest first, each list bounded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryView {
    /// `memory/about-you.md`.
    pub about_you: Vec<MemoryRow>,
    /// `memory/facts.md`.
    pub systems: Vec<MemoryRow>,
    pub notes: Vec<NoteRow>,
    /// The newest session that has a digest.
    pub latest: Option<DigestCard>,
}

/// The Now view (PRODUCT §7.5): open tasks, a short list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowList {
    /// The project asked for, if any.
    pub project: Option<String>,
    pub tasks: Vec<TaskView>,
    /// Open tasks past `NOW_LIST_MAX`.
    pub more_tasks: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// `t27`, or `<path>#<line>` for a task written by hand without an id.
    pub id: String,
    pub title: String,
    pub project: String,
    pub done: bool,
    pub links: Vec<String>,
    pub session: Option<String>,
    /// The title of the session it was captured in, when that session is indexed.
    pub session_title: Option<String>,
    /// That session's `YYYY-MM-DD`, UTC.
    pub session_date: Option<String>,
}

/// What `open_session` returns to bring a past board back (PRODUCT §7.4 recall).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: SessionId,
    pub title: String,
    pub started: String,
    pub duration_ms: u64,
    /// The session's log, for a full replay.
    pub events_path: PathBuf,
    /// A two-sentence recap from the digest; empty when there is no digest.
    pub recap: String,
    /// The final board as UI events: per layer still open at the end, its `LayerPush` (except
    /// the main layer) and its ops in order, then the last camera. The orchestrator can send
    /// them with short budgets to redraw the board.
    pub board: Vec<UiEvent>,
    /// Board events past `SESSION_BOARD_EVENTS_MAX`, not returned.
    pub board_events_skipped: u64,
    pub log_skipped: SkipCounts,
}

/// What `remember` and `accept_proposal` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kept", rename_all = "snake_case")]
pub enum Remembered {
    /// Written to a memory file.
    Memory {
        id: ItemId,
        file: MemoryFileName,
    },
    Note {
        id: ItemId,
        path: String,
    },
    Task {
        id: ItemId,
        project: String,
        path: String,
    },
    /// Kept with the session record, listed in its digest.
    SessionRecord,
    /// Waiting for a yes; listed in the digest.
    Proposed {
        id: ItemId,
    },
}

/// The memory file, as serde sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryFileName {
    AboutYou,
    Facts,
}

impl From<MemoryFile> for MemoryFileName {
    fn from(file: MemoryFile) -> Self {
        match file {
            MemoryFile::AboutYou => Self::AboutYou,
            MemoryFile::Facts => Self::Facts,
        }
    }
}

/// What "forget that" removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forgotten {
    pub what: ForgottenKind,
    /// The text that was removed, so the talker can say what it forgot.
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgottenKind {
    Memory,
    Note,
    Task,
    SessionRecord,
    Proposal,
}

/// A session the store started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    /// `sessions/<folder>`, relative to the vault.
    pub folder: String,
}
