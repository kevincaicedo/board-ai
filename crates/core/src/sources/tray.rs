//! The Sources tray (artboard 14) and the drop overlay (14b), as the UI receives them. The core
//! sends the whole tray each time it changes (it is small and bounded), so the UI keeps no
//! state of its own about sources: only which tab it shows.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{DropHalf, SourceId, SourceKind, SourceState};

/// What BoardAI can read: this session's sources, and the current project's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourcesTray {
    /// The tray shows: opened by the button or "show sources", and on its own while something
    /// is being read.
    pub open: bool,
    /// Shared for this session only.
    pub session: Vec<SourceRow>,
    /// The current project, or the one proposed from a shared folder's name.
    pub project: Option<ProjectTab>,
    /// Where a source's text goes (the footer): the talker's name, and the harness's.
    pub talker: String,
    pub harness: Option<String>,
    /// Files the harness is reading right now, newest last: the ghost names at the edge
    /// (PRODUCT §8.6). Empty when nothing is being read.
    pub reading: Vec<String>,
}

/// The project tab: kept sources are pinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectTab {
    pub name: String,
    /// Not chosen yet: proposed from a shared code folder's name. Keeping a source makes it the
    /// current project.
    pub proposed: bool,
    pub sources: Vec<SourceRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceRow {
    pub id: SourceId,
    pub kind: SourceKind,
    pub title: String,
    /// `~/…` only, never an absolute path.
    pub path: String,
    pub state: SourceState,
}

/// While a file is dragged over the window (14b): what dropping it would share, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DropOverlay {
    /// "hekaton-sigmod13.pdf", or "3 items".
    pub title: String,
    /// Why it can't be shared, when nothing dragged can be.
    pub refused: Option<String>,
    /// The half under the pointer: the left shares for this session, the right keeps it in
    /// the project.
    pub half: DropHalf,
    /// The right half's project: the current one, the proposed one, or the name a new one would
    /// get.
    pub project: String,
}

/// What the tray's controls ask (UI → core).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum SourceAction {
    /// The tray button.
    Open,
    Close,
    /// The pin: keep this session source in the project (the current one, the proposed one,
    /// or a new one named after the source).
    Keep {
        source: SourceId,
    },
    /// The pin again: the project stops keeping it; it stays readable for this session.
    Unkeep {
        source: SourceId,
    },
    /// No longer readable at all, in this session or the project.
    Remove {
        source: SourceId,
    },
}

/// What the tray's chooser picks (the app shows the macOS chooser itself, D-173).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ChooseWhat {
    Folder,
    File,
}
