//! The session event log, `sessions/<folder>/events.jsonl` (LLD §3.6): one JSON record per
//! line, each with a version, a session time and a kind, append-only (law 4).
//!
//! | `k` | Carries | For |
//! |---|---|---|
//! | `session` | `id`, `started` | The first line: who this log belongs to |
//! | `ui` | `event` ([`UiLogLine`]) | Replay in the UI, unchanged as the core evolves |
//! | `input` | `input` (a core [`Input`]) | Re-simulating the core with the same build |
//! | `record` | `record` (a core [`LogRecord`]) | Milestones and drop counts from the core |
//! | `dropped` | `reason`, `count` | What the store itself had to drop, so gaps are visible |
//! | `end` | | The last line of a session that ended cleanly |
//!
//! The UI replays `ui` lines and skips the others. Your voice is never in it: inputs carry
//! transcripts, never audio.

mod reader;
mod summary;
mod writer;

use boardai_core::effect::LogRecord;
use boardai_core::input::Input;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use serde::{Deserialize, Serialize};

pub use reader::{EventLogReader, SkipCounts};
pub(crate) use summary::{LogSummary, summarize_log};
pub(crate) use writer::EventLogWriter;

/// The version every line carries; the reader skips other versions and counts them.
pub const LOG_VERSION: u8 = 1;

/// What the orchestrator logs, stamped with the session time of the step.
#[derive(Clone, Debug, PartialEq)]
pub enum LogEntry {
    Input {
        t: SessionTime,
        input: Input,
    },
    /// The UI events of one step, in order; each becomes its own line.
    Ui {
        t: SessionTime,
        events: Vec<UiEvent>,
    },
    Record {
        t: SessionTime,
        record: LogRecord,
    },
}

impl LogEntry {
    #[must_use]
    pub const fn time(&self) -> SessionTime {
        match self {
            Self::Input { t, .. } | Self::Ui { t, .. } | Self::Record { t, .. } => *t,
        }
    }
}

/// Why the store dropped log lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The line was longer than `EVENT_LOG_LINE_MAX_BYTES`.
    Oversized,
    /// The store's inbox was full, so the entry never reached the log.
    InboxFull,
    /// Writing to the file failed (a full disk); the buffered lines are lost.
    WriteFailed,
    /// The entry could not be encoded as JSON. The core's types always encode, so this marks
    /// a broken invariant rather than bad input.
    Unencodable,
}

/// One line read back from a log.
#[derive(Clone, Debug, PartialEq)]
pub enum LogLine {
    Session {
        id: String,
        started: String,
    },
    Ui {
        t: SessionTime,
        event: UiEvent,
    },
    Input {
        t: SessionTime,
        input: Input,
    },
    Record {
        t: SessionTime,
        record: LogRecord,
    },
    Dropped {
        t: SessionTime,
        reason: DropReason,
        count: u64,
    },
    End {
        t: SessionTime,
    },
}

impl LogLine {
    #[must_use]
    pub const fn time(&self) -> SessionTime {
        match self {
            Self::Session { .. } => SessionTime::ZERO,
            Self::Ui { t, .. }
            | Self::Input { t, .. }
            | Self::Record { t, .. }
            | Self::Dropped { t, .. }
            | Self::End { t } => *t,
        }
    }
}

/// A line this module writes, other than `ui` (the core's [`UiLogLine`]): `v`, `t` and `k`
/// first, in `UiLogLine`'s order, then the kind's own fields.
#[derive(Serialize)]
struct OwnLine<'a> {
    v: u8,
    t: SessionTime,
    k: &'static str,
    #[serde(flatten)]
    body: OwnBody<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OwnBody<'a> {
    Session {
        id: &'a str,
        started: &'a str,
    },
    Input {
        input: &'a Input,
    },
    Record {
        record: &'a LogRecord,
    },
    Dropped {
        reason: DropReason,
        count: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
    },
    End {},
}

impl<'a> OwnLine<'a> {
    const fn new(t: SessionTime, body: OwnBody<'a>) -> Self {
        let k = match body {
            OwnBody::Session { .. } => "session",
            OwnBody::Input { .. } => "input",
            OwnBody::Record { .. } => "record",
            OwnBody::Dropped { .. } => "dropped",
            OwnBody::End {} => "end",
        };
        Self {
            v: LOG_VERSION,
            t,
            k,
            body,
        }
    }
}

/// Every line as it is read. Unknown fields are ignored, so a newer build may add fields
/// without breaking this reader; a line of an unknown kind is malformed.
#[derive(Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum WireLine {
    Session {
        v: u8,
        id: String,
        started: String,
    },
    Ui {
        v: u8,
        t: SessionTime,
        event: UiEvent,
    },
    Input {
        v: u8,
        t: SessionTime,
        input: Input,
    },
    Record {
        v: u8,
        t: SessionTime,
        record: LogRecord,
    },
    Dropped {
        v: u8,
        t: SessionTime,
        reason: DropReason,
        count: u64,
    },
    End {
        v: u8,
        t: SessionTime,
    },
}

impl WireLine {
    const fn version(&self) -> u8 {
        match self {
            Self::Session { v, .. }
            | Self::Ui { v, .. }
            | Self::Input { v, .. }
            | Self::Record { v, .. }
            | Self::Dropped { v, .. }
            | Self::End { v, .. } => *v,
        }
    }

    fn into_line(self) -> LogLine {
        match self {
            Self::Session { id, started, .. } => LogLine::Session { id, started },
            Self::Ui { t, event, .. } => LogLine::Ui { t, event },
            Self::Input { t, input, .. } => LogLine::Input { t, input },
            Self::Record { t, record, .. } => LogLine::Record { t, record },
            Self::Dropped {
                t, reason, count, ..
            } => LogLine::Dropped { t, reason, count },
            Self::End { t, .. } => LogLine::End { t },
        }
    }
}
