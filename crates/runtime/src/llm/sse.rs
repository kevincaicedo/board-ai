//! A small, bounded Server-Sent Events parser (the WHATWG event-stream format).
//!
//! It is a byte-at-a-time state machine with no recursion, so it gives the same events however
//! the stream is split into chunks: a CR at the end of one chunk and the LF that starts the next
//! are one line ending. Lines and events are bounded; crossing a bound is an error, never a
//! silent truncation of provider data.

use std::fmt;

use crate::limits::{SSE_EVENT_MAX_BYTES, SSE_LINE_MAX_BYTES};

/// One dispatched event. Fields other than `event` and `data` (`id`, `retry`) are not used by
/// any provider we speak to and are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SseError {
    LineTooLong,
    EventTooLarge,
    InvalidUtf8,
}

impl fmt::Display for SseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineTooLong => {
                write!(formatter, "an SSE line exceeds {SSE_LINE_MAX_BYTES} bytes")
            }
            Self::EventTooLarge => {
                write!(
                    formatter,
                    "an SSE event exceeds {SSE_EVENT_MAX_BYTES} bytes"
                )
            }
            Self::InvalidUtf8 => formatter.write_str("an SSE line is not UTF-8"),
        }
    }
}

const BYTE_ORDER_MARK: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Debug)]
pub struct SseParser {
    line: Vec<u8>,
    /// The previous byte was a CR, so an LF right after it ends nothing.
    after_carriage_return: bool,
    /// No line has ended yet; a byte order mark may lead the first one.
    first_line: bool,
    event: Option<String>,
    data: String,
    has_data: bool,
    line_max_bytes: usize,
    event_max_bytes: usize,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(SSE_LINE_MAX_BYTES, SSE_EVENT_MAX_BYTES)
    }

    /// Smaller limits, so tests can cross them without megabytes of input.
    #[must_use]
    pub fn with_limits(line_max_bytes: usize, event_max_bytes: usize) -> Self {
        Self {
            line: Vec::new(),
            after_carriage_return: false,
            first_line: true,
            event: None,
            data: String::new(),
            has_data: false,
            line_max_bytes,
            event_max_bytes,
        }
    }

    /// Feeds a chunk and appends every event it completes to `out`.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        for &byte in chunk {
            let after_carriage_return = std::mem::replace(&mut self.after_carriage_return, false);
            match byte {
                b'\n' if after_carriage_return => {}
                b'\n' => self.end_line(out)?,
                b'\r' => {
                    self.end_line(out)?;
                    self.after_carriage_return = true;
                }
                _ => {
                    if self.line.len() >= self.line_max_bytes {
                        return Err(SseError::LineTooLong);
                    }
                    self.line.push(byte);
                }
            }
        }
        Ok(())
    }

    /// Ends the stream. An event that was not terminated by a blank line is discarded, as the
    /// format requires; returns whether anything was discarded.
    pub fn finish(&mut self) -> bool {
        let discarded = !self.line.is_empty() || self.has_data || self.event.is_some();
        self.line.clear();
        self.event = None;
        self.data.clear();
        self.has_data = false;
        discarded
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        let mut line = std::mem::take(&mut self.line);
        if std::mem::replace(&mut self.first_line, false) && line.starts_with(BYTE_ORDER_MARK) {
            line.drain(..BYTE_ORDER_MARK.len());
        }
        let result = self.process_line(&line, out);
        line.clear();
        // Keep the allocation for the next line.
        self.line = line;
        result
    }

    fn process_line(&mut self, line: &[u8], out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        if line.is_empty() {
            self.dispatch(out);
            return Ok(());
        }
        if line.first() == Some(&b':') {
            return Ok(());
        }
        let (field, value) = match line.iter().position(|&byte| byte == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                (&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &[][..]),
        };
        let value = std::str::from_utf8(value).map_err(|_| SseError::InvalidUtf8)?;
        match field {
            b"event" => self.event = Some(value.to_owned()),
            b"data" => {
                let separator = usize::from(self.has_data);
                if self.data.len() + separator + value.len() > self.event_max_bytes {
                    return Err(SseError::EventTooLarge);
                }
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.event.take();
        if std::mem::replace(&mut self.has_data, false) {
            out.push(SseEvent {
                event,
                data: std::mem::take(&mut self.data),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SseError, SseEvent, SseParser};

    fn parse(input: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let mut parser = SseParser::new();
        let mut out = Vec::new();
        parser.push(input, &mut out)?;
        Ok(out)
    }

    #[test]
    fn parses_events_comments_and_multiline_data() {
        let events = parse(
            b"\xEF\xBB\xBF: keep-alive\nevent: ping\ndata: {}\n\ndata: a\ndata:b\r\n\r\nid: 7\n\n",
        );
        assert_eq!(
            events,
            Ok(vec![
                SseEvent {
                    event: Some("ping".into()),
                    data: "{}".into()
                },
                SseEvent {
                    event: None,
                    data: "a\nb".into()
                },
            ])
        );
    }

    #[test]
    fn a_line_ending_split_across_chunks_is_one_ending() {
        let mut parser = SseParser::new();
        let mut out = Vec::new();
        assert_eq!(parser.push(b"data: x\r", &mut out), Ok(()));
        assert_eq!(parser.push(b"\n\r", &mut out), Ok(()));
        assert_eq!(parser.push(b"\n", &mut out), Ok(()));
        assert_eq!(out.len(), 1);
        assert!(!parser.finish());
    }

    #[test]
    fn crossing_a_bound_is_an_error() {
        let mut parser = SseParser::with_limits(8, 12);
        let mut out = Vec::new();
        assert_eq!(
            parser.push(b"data: 123456789\n", &mut out),
            Err(SseError::LineTooLong)
        );
        let mut parser = SseParser::with_limits(64, 12);
        assert_eq!(
            parser.push(b"data: 1234567\ndata: 123456\n", &mut out),
            Err(SseError::EventTooLarge)
        );
        assert_eq!(parse(b"data: \xFF\n\n"), Err(SseError::InvalidUtf8));
    }

    #[test]
    fn an_unterminated_event_is_discarded_at_the_end() {
        let mut parser = SseParser::new();
        let mut out = Vec::new();
        assert_eq!(parser.push(b"data: half", &mut out), Ok(()));
        assert!(out.is_empty());
        assert!(parser.finish());
    }
}
