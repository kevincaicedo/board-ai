//! Streams a log back, line by line, for re-simulation, the library and the index. Memory stays
//! bounded whatever the file holds: a line longer than `EVENT_LOG_LINE_MAX_BYTES` is skipped
//! without being buffered, and every skipped line is counted by reason.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{LOG_VERSION, LogLine, WireLine};
use crate::limits::EVENT_LOG_LINE_MAX_BYTES;

/// Lines the reader skipped, by reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkipCounts {
    /// Not JSON, an unknown kind, or a field of the wrong shape.
    pub malformed: u64,
    /// Longer than `EVENT_LOG_LINE_MAX_BYTES`.
    pub oversized: u64,
    /// Written by another version of the log format.
    pub unsupported_version: u64,
}

/// An iterator over a log's records. It yields `Err` once on a read failure and then ends.
pub struct EventLogReader<R> {
    reader: R,
    buffer: Vec<u8>,
    skipped: SkipCounts,
    finished: bool,
}

impl EventLogReader<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::new(BufReader::new(File::open(path)?)))
    }
}

impl<R: BufRead> EventLogReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
            skipped: SkipCounts::default(),
            finished: false,
        }
    }

    #[must_use]
    pub const fn skipped(&self) -> SkipCounts {
        self.skipped
    }

    fn parse(&mut self) -> Option<LogLine> {
        let text = std::str::from_utf8(&self.buffer).ok()?.trim();
        if text.is_empty() {
            return None;
        }
        match serde_json::from_str::<WireLine>(text) {
            Ok(line) if line.version() == LOG_VERSION => Some(line.into_line()),
            Ok(_) => {
                self.skipped.unsupported_version += 1;
                None
            }
            Err(_) => {
                self.skipped.malformed += 1;
                None
            }
        }
    }
}

impl<R: BufRead> Iterator for EventLogReader<R> {
    type Item = io::Result<LogLine>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            match read_line_capped(&mut self.reader, &mut self.buffer, EVENT_LOG_LINE_MAX_BYTES) {
                Ok(LineRead::End) => self.finished = true,
                Ok(LineRead::TooLong) => self.skipped.oversized += 1,
                Ok(LineRead::Complete) => {
                    if std::str::from_utf8(&self.buffer).is_err() {
                        self.skipped.malformed += 1;
                    } else if let Some(line) = self.parse() {
                        return Some(Ok(line));
                    }
                }
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineRead {
    Complete,
    /// The line was longer than the cap; it was consumed and not kept.
    TooLong,
    End,
}

/// Reads one line (without its newline) into `buffer`, keeping at most `max_bytes`. A longer
/// line is consumed to its end and reported as `TooLong`.
fn read_line_capped(
    reader: &mut impl BufRead,
    buffer: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<LineRead> {
    buffer.clear();
    let mut too_long = false;
    let mut consumed_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        consumed_any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if !too_long && buffer.len() + take <= max_bytes {
            buffer.extend_from_slice(&available[..take]);
        } else {
            too_long = true;
            buffer.clear();
        }
        reader.consume(newline.map_or(take, |at| at + 1));
        if newline.is_some() {
            break;
        }
    }
    Ok(match (consumed_any, too_long) {
        (false, _) => LineRead::End,
        (true, true) => LineRead::TooLong,
        (true, false) => LineRead::Complete,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{LineRead, read_line_capped};

    #[test]
    fn long_lines_are_skipped_without_being_kept() {
        let input = format!("short\n{}\nlast", "x".repeat(100));
        let mut reader = Cursor::new(input.into_bytes());
        let mut buffer = Vec::new();
        let mut read = || read_line_capped(&mut reader, &mut buffer, 10).ok();
        assert_eq!(read(), Some(LineRead::Complete));
        assert_eq!(read(), Some(LineRead::TooLong));
        assert_eq!(read(), Some(LineRead::Complete));
        assert_eq!(read(), Some(LineRead::End));
    }
}
