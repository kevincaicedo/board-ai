//! Coding harness tasks and their events, provider-neutral (LLD §3.5).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessTask {
    pub kind: HarnessKind,
    /// The precise brief handed to the harness.
    pub brief: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessKind {
    /// Read-only.
    Research,
    /// Read-only.
    Review,
    /// Edits files, only after a spoken yes (I9), and only on this new branch.
    Change { branch: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HarnessEvent {
    Progress {
        text: String,
    },
    FileTouched {
        path: String,
    },
    /// `citations` are `path:line` strings the harness gave as sources.
    Finished {
        summary: String,
        citations: Vec<String>,
    },
    Failed {
        reason: HarnessFailure,
        detail: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessFailure {
    NotInstalled,
    TimedOut,
    Cancelled,
    Crashed,
    BadOutput,
    /// Concurrency and queue limits are full; refused out loud (LLD §3.5).
    QueueFull,
}
