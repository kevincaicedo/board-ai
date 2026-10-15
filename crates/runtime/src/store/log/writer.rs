//! The buffered, append-only log writer. Entries are buffered in memory and written in one
//! `write` per flush (every `EVENT_LOG_FLUSH_MS`, earlier at `EVENT_LOG_PENDING_MAX`, and at
//! session end). Flushes are not fsynced: a `write` survives a crash of the app, and an fsync
//! every 250 ms (`F_FULLFSYNC` on macOS) would cost far more than the power-loss case is worth.
//! The end of a session is fsynced.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use boardai_core::protocol::{UiLogKind, UiLogLine};
use boardai_core::time::SessionTime;

use super::{DropReason, LOG_VERSION, LogEntry, OwnBody, OwnLine};
use crate::limits::{EVENT_LOG_LINE_MAX_BYTES, EVENT_LOG_PENDING_MAX};
use crate::store::ids::SessionId;
use crate::store::redact::redact_json_text;
use crate::store::time::WallTime;
use crate::store::vault::sync_folder;

/// What a log has written and dropped so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogStats {
    pub lines_written: u64,
    /// Records replaced by a `dropped` marker because they were too long.
    pub oversized: u64,
    /// Entries dropped before reaching the log because the store's inbox was full.
    pub inbox_dropped: u64,
    /// Lines lost to a failed write.
    pub write_failed: u64,
    /// Secrets replaced by redaction.
    pub secrets_redacted: u64,
}

pub(crate) struct EventLogWriter {
    file: File,
    pending: Vec<LogEntry>,
    last_t: SessionTime,
    /// A failed write may have left half a line; the next write starts on a new line.
    needs_newline: bool,
    stats: LogStats,
}

impl EventLogWriter {
    /// Creates the log (it must not exist) and writes its `session` line durably.
    pub(crate) fn create(path: &Path, id: &SessionId, started: WallTime) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(path)?;
        let started = started.rfc3339();
        let header = OwnLine::new(
            SessionTime::ZERO,
            OwnBody::Session {
                id: id.as_str(),
                started: &started,
            },
        );
        let line = serde_json::to_string(&header).map_err(io::Error::other)?;
        let line = redact_json_text(&line);
        file.write_all(line.as_str().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if let Some(folder) = path.parent() {
            sync_folder(folder);
        }
        Ok(Self {
            file,
            pending: Vec::new(),
            last_t: SessionTime::ZERO,
            needs_newline: false,
            stats: LogStats {
                lines_written: 1,
                ..LogStats::default()
            },
        })
    }

    /// Buffers an entry. Returns whether the buffer is full and should be flushed now.
    pub(crate) fn push(&mut self, entry: LogEntry) -> bool {
        self.last_t = self.last_t.max(entry.time());
        self.pending.push(entry);
        self.pending.len() >= EVENT_LOG_PENDING_MAX
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Writes the buffered entries, plus a marker for `inbox_dropped` entries that never
    /// arrived. Blocking.
    pub(crate) fn flush(&mut self, inbox_dropped: u64) -> io::Result<()> {
        let entries = std::mem::take(&mut self.pending);
        let mut buffer = Vec::new();
        let mut lines = 0_u64;
        if self.needs_newline {
            buffer.push(b'\n');
        }
        for entry in entries {
            lines += self.encode(entry, &mut buffer);
        }
        if inbox_dropped > 0 {
            self.stats.inbox_dropped += inbox_dropped;
            let marker = dropped_line(self.last_t, DropReason::InboxFull, inbox_dropped, None);
            lines += push_line(&mut buffer, &marker);
        }
        if lines == 0 {
            return Ok(());
        }
        match self.file.write_all(&buffer) {
            Ok(()) => {
                self.needs_newline = false;
                self.stats.lines_written += lines;
                Ok(())
            }
            Err(error) => {
                self.needs_newline = true;
                self.stats.write_failed += lines;
                Err(error)
            }
        }
    }

    /// Flushes, writes the `end` line and makes the log durable. Blocking.
    pub(crate) fn finish(mut self, t_end: SessionTime, inbox_dropped: u64) -> io::Result<LogStats> {
        self.flush(inbox_dropped)?;
        let end = OwnLine::new(t_end.max(self.last_t), OwnBody::End {});
        let mut buffer = Vec::new();
        if self.needs_newline {
            buffer.push(b'\n');
        }
        if let Ok(line) = serde_json::to_string(&end) {
            push_line(&mut buffer, &line);
        }
        self.file.write_all(&buffer)?;
        self.file.sync_data()?;
        self.stats.lines_written += 1;
        Ok(self.stats)
    }

    /// Encodes one entry into `buffer`; returns the number of lines added.
    fn encode(&mut self, entry: LogEntry, buffer: &mut Vec<u8>) -> u64 {
        let mut lines = 0;
        match entry {
            LogEntry::Ui { t, events } => {
                for event in events {
                    let line = UiLogLine {
                        v: LOG_VERSION,
                        t,
                        k: UiLogKind::Ui,
                        event,
                    };
                    lines += self.encode_line(t, serde_json::to_string(&line), buffer);
                }
            }
            LogEntry::Input { t, input } => {
                let line = OwnLine::new(t, OwnBody::Input { input: &input });
                lines += self.encode_line(t, serde_json::to_string(&line), buffer);
            }
            LogEntry::Record { t, record } => {
                let line = OwnLine::new(t, OwnBody::Record { record: &record });
                lines += self.encode_line(t, serde_json::to_string(&line), buffer);
            }
        }
        lines
    }

    /// Redacts one encoded line and appends it, or a `dropped` marker in its place.
    fn encode_line(
        &mut self,
        t: SessionTime,
        encoded: Result<String, serde_json::Error>,
        buffer: &mut Vec<u8>,
    ) -> u64 {
        let Ok(line) = encoded else {
            debug_assert!(false, "a core type failed to encode");
            let marker = dropped_line(t, DropReason::Unencodable, 1, None);
            return push_line(buffer, &marker);
        };
        let redacted = redact_json_text(&line);
        self.stats.secrets_redacted += redacted.count() as u64;
        if redacted.as_str().len() > EVENT_LOG_LINE_MAX_BYTES {
            self.stats.oversized += 1;
            let size_bytes = redacted.as_str().len() as u64;
            let marker = dropped_line(t, DropReason::Oversized, 1, Some(size_bytes));
            return push_line(buffer, &marker);
        }
        push_line(buffer, redacted.as_str())
    }
}

fn dropped_line(t: SessionTime, reason: DropReason, count: u64, size_bytes: Option<u64>) -> String {
    let line = OwnLine::new(
        t,
        OwnBody::Dropped {
            reason,
            count,
            size_bytes,
        },
    );
    // Numbers and fixed names only: this always encodes, and holds nothing to redact.
    serde_json::to_string(&line).unwrap_or_default()
}

fn push_line(buffer: &mut Vec<u8>, line: &str) -> u64 {
    if line.is_empty() {
        return 0;
    }
    buffer.extend_from_slice(line.as_bytes());
    buffer.push(b'\n');
    1
}
