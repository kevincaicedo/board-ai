//! What the store needs from a whole log in one streaming pass: who it belongs to, how long it
//! ran, and the transcript (final captions only, so partials never repeat).

use std::io;
use std::path::Path;

use boardai_core::protocol::{Speaker, UiEvent};
use boardai_core::time::SessionTime;

use super::{EventLogReader, LogLine, SkipCounts};
use crate::limits::INDEX_TEXT_MAX_BYTES;
use crate::store::digest::TranscriptLine;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LogSummary {
    pub id: Option<String>,
    pub started: Option<String>,
    /// The `end` line's time, or the last time seen when the session didn't end cleanly.
    pub last_t: SessionTime,
    pub ended: bool,
    /// Final captions, up to `INDEX_TEXT_MAX_BYTES` of text; later ones are counted.
    pub transcript: Vec<TranscriptLine>,
    pub transcript_lines_left_out: u64,
    /// Whether anything was drawn.
    pub has_ops: bool,
    pub skipped: SkipCounts,
}

impl LogSummary {
    pub(crate) fn first_user_line(&self) -> Option<&str> {
        self.transcript
            .iter()
            .find(|line| line.who == Speaker::User)
            .map(|line| line.text.as_str())
    }

    /// The transcript as searchable text.
    pub(crate) fn transcript_text(&self) -> String {
        let mut out = String::new();
        for line in &self.transcript {
            out.push_str(match line.who {
                Speaker::User => "You: ",
                Speaker::Assistant => "BoardAI: ",
            });
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }
}

pub(crate) fn summarize_log(path: &Path) -> io::Result<LogSummary> {
    let mut reader = EventLogReader::open(path)?;
    let mut summary = LogSummary::default();
    let mut text_bytes = 0;
    for line in reader.by_ref() {
        let line = line?;
        summary.last_t = summary.last_t.max(line.time());
        match line {
            LogLine::Session { id, started } => {
                summary.id.get_or_insert(id);
                summary.started.get_or_insert(started);
            }
            LogLine::Ui {
                t,
                event:
                    UiEvent::Caption {
                        who,
                        text,
                        is_final: true,
                    },
            } => {
                if text_bytes + text.len() > INDEX_TEXT_MAX_BYTES {
                    summary.transcript_lines_left_out += 1;
                } else if !text.trim().is_empty() {
                    text_bytes += text.len();
                    summary.transcript.push(TranscriptLine { t, who, text });
                }
            }
            LogLine::Ui {
                event: UiEvent::OpStart { .. },
                ..
            } => summary.has_ops = true,
            LogLine::End { .. } => summary.ended = true,
            LogLine::Ui { .. }
            | LogLine::Input { .. }
            | LogLine::Record { .. }
            | LogLine::Dropped { .. } => {}
        }
    }
    summary.skipped = reader.skipped();
    Ok(summary)
}
