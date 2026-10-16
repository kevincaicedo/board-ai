//! Newline-delimited JSON framing (Ollama's native chat stream): every line is one event.
//!
//! A byte-at-a-time state machine like the SSE parser, so it gives the same events however the
//! stream is split into chunks, and every line is bounded: crossing the bound is an error, never
//! a silent truncation of provider data. Events reuse [`SseEvent`] (data only), so the streaming
//! session and its decoders are shared with the SSE providers.

use std::fmt;

use super::sse::SseEvent;
use crate::limits::NDJSON_LINE_MAX_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NdjsonError {
    LineTooLong,
    InvalidUtf8,
}

impl fmt::Display for NdjsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineTooLong => write!(
                formatter,
                "a JSON line exceeds {NDJSON_LINE_MAX_BYTES} bytes"
            ),
            Self::InvalidUtf8 => formatter.write_str("a JSON line is not UTF-8"),
        }
    }
}

#[derive(Debug)]
pub struct NdjsonParser {
    line: Vec<u8>,
    line_max_bytes: usize,
}

impl Default for NdjsonParser {
    fn default() -> Self {
        Self::new()
    }
}

impl NdjsonParser {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(NDJSON_LINE_MAX_BYTES)
    }

    /// A smaller bound, so tests can cross it without megabytes of input.
    #[must_use]
    pub fn with_limit(line_max_bytes: usize) -> Self {
        Self {
            line: Vec::new(),
            line_max_bytes,
        }
    }

    /// Feeds a chunk; every line it completes is appended to `out` (blank lines are skipped, a
    /// CR before the LF is dropped).
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<SseEvent>) -> Result<(), NdjsonError> {
        for &byte in chunk {
            if byte == b'\n' {
                self.dispatch(out)?;
            } else if self.line.len() >= self.line_max_bytes {
                self.line.clear();
                return Err(NdjsonError::LineTooLong);
            } else {
                self.line.push(byte);
            }
        }
        Ok(())
    }

    /// The body ended. A last line without its newline is dropped, as a trailing SSE event is:
    /// whether the response is complete is the decoder's call. True when something was dropped.
    pub fn finish(&mut self) -> bool {
        let dropped = !self.line.is_empty();
        self.line.clear();
        dropped
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) -> Result<(), NdjsonError> {
        let line = std::mem::take(&mut self.line);
        let text = String::from_utf8(line).map_err(|_| NdjsonError::InvalidUtf8)?;
        let text = text.strip_suffix('\r').unwrap_or(&text);
        if !text.trim().is_empty() {
            out.push(SseEvent {
                event: None,
                data: text.to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{NdjsonError, NdjsonParser};

    fn lines(parser: &mut NdjsonParser, chunks: &[&[u8]]) -> Result<Vec<String>, NdjsonError> {
        let mut out = Vec::new();
        for chunk in chunks {
            parser.push(chunk, &mut out)?;
        }
        Ok(out.into_iter().map(|event| event.data).collect())
    }

    #[test]
    fn every_split_gives_the_same_lines() {
        let body = b"{\"a\":1}\r\n\n{\"b\":\"\xc3\xa9\"}\n{\"c\":3}";
        let whole = lines(&mut NdjsonParser::new(), &[body]).unwrap_or_default();
        assert_eq!(whole, vec!["{\"a\":1}", "{\"b\":\"é\"}"]);
        for split in 0..=body.len() {
            let mut parser = NdjsonParser::new();
            let parts = lines(&mut parser, &[&body[..split], &body[split..]]);
            assert_eq!(parts, Ok(whole.clone()), "split at {split}");
            assert!(parser.finish(), "the unterminated last line is dropped");
        }
    }

    #[test]
    fn a_long_or_broken_line_is_an_error() {
        let mut parser = NdjsonParser::with_limit(4);
        assert_eq!(
            lines(&mut parser, &[b"12345\n"]),
            Err(NdjsonError::LineTooLong)
        );
        assert_eq!(
            lines(&mut NdjsonParser::new(), &[b"\xff\n"]),
            Err(NdjsonError::InvalidUtf8)
        );
    }
}
