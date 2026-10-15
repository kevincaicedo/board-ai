//! Bounded pieces of the subprocess reader: LF-delimited lines and the output tail.
//!
//! Lines split on LF only. pi's RPC framing requires it (JSON strings may hold U+2028), and
//! Claude Code's stream-json is the same shape.

use std::collections::VecDeque;

use crate::limits::HARNESS_LINE_MAX_BYTES;

/// Splits a byte stream into lines without their LF (and without a CR before it). A line longer
/// than the bound is skipped whole and counted, and the next line is read normally.
#[derive(Debug)]
pub(crate) struct LineSplitter {
    current: Vec<u8>,
    skipping: bool,
    max_bytes: usize,
    skipped_lines: u64,
}

impl Default for LineSplitter {
    fn default() -> Self {
        Self::with_limit(HARNESS_LINE_MAX_BYTES)
    }
}

impl LineSplitter {
    pub(crate) fn with_limit(max_bytes: usize) -> Self {
        Self {
            current: Vec::new(),
            skipping: false,
            max_bytes,
            skipped_lines: 0,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8], lines: &mut Vec<Vec<u8>>) {
        let mut rest = chunk;
        while !rest.is_empty() {
            match rest.iter().position(|&byte| byte == b'\n') {
                Some(newline) => {
                    self.append(&rest[..newline]);
                    self.end_line(lines);
                    rest = &rest[newline + 1..];
                }
                None => {
                    self.append(rest);
                    rest = &[];
                }
            }
        }
    }

    /// The stream ended: a final line without an LF still counts.
    pub(crate) fn finish(&mut self, lines: &mut Vec<Vec<u8>>) {
        if !self.current.is_empty() || self.skipping {
            self.end_line(lines);
        }
    }

    pub(crate) fn skipped_lines(&self) -> u64 {
        self.skipped_lines
    }

    fn append(&mut self, bytes: &[u8]) {
        if self.skipping {
            return;
        }
        if self.current.len() + bytes.len() > self.max_bytes {
            self.skipping = true;
            self.skipped_lines += 1;
            self.current.clear();
            return;
        }
        self.current.extend_from_slice(bytes);
    }

    fn end_line(&mut self, lines: &mut Vec<Vec<u8>>) {
        if !std::mem::replace(&mut self.skipping, false) {
            let mut line = std::mem::take(&mut self.current);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() {
                lines.push(line);
            }
        }
        self.current.clear();
    }
}

/// The last bytes of a stream; older bytes are dropped as new ones arrive.
#[derive(Debug)]
pub(crate) struct TailBuffer {
    bytes: VecDeque<u8>,
    capacity: usize,
}

impl TailBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        let keep = &chunk[chunk.len().saturating_sub(self.capacity)..];
        let overflow = (self.bytes.len() + keep.len()).saturating_sub(self.capacity);
        self.bytes.drain(..overflow);
        self.bytes.extend(keep);
    }

    /// The last `max_bytes` as text (invalid UTF-8 replaced), trimmed.
    pub(crate) fn tail_text(&self, max_bytes: usize) -> String {
        let skip = self.bytes.len().saturating_sub(max_bytes);
        let tail: Vec<u8> = self.bytes.iter().skip(skip).copied().collect();
        String::from_utf8_lossy(&tail).trim().to_owned()
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{LineSplitter, TailBuffer};

    fn split(chunks: &[&[u8]], limit: usize) -> (Vec<Vec<u8>>, u64) {
        let mut splitter = LineSplitter::with_limit(limit);
        let mut lines = Vec::new();
        for chunk in chunks {
            splitter.push(chunk, &mut lines);
        }
        splitter.finish(&mut lines);
        (lines, splitter.skipped_lines())
    }

    #[test]
    fn lines_split_on_lf_only_wherever_chunks_fall() {
        let input = "{\"a\":\"x\u{2028}y\"}\r\n\n{\"b\":2}\nlast".as_bytes();
        let whole = split(&[input], 64);
        assert_eq!(whole.0.len(), 3);
        assert_eq!(whole.0[0], "{\"a\":\"x\u{2028}y\"}".as_bytes());
        for cut in 0..=input.len() {
            let (head, tail) = input.split_at(cut);
            assert_eq!(split(&[head, tail], 64), whole, "cut at {cut}");
        }
    }

    #[test]
    fn an_overlong_line_is_skipped_and_counted() {
        let (lines, skipped) = split(&[b"short\n", &[b'x'; 40], b"\nnext\n"], 16);
        assert_eq!(lines, [b"short".to_vec(), b"next".to_vec()]);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn the_tail_keeps_only_the_last_bytes() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"abcdef");
        tail.push(b"ghij");
        assert_eq!(tail.len(), 8);
        assert_eq!(tail.tail_text(8), "cdefghij");
        tail.push(b"0123456789");
        assert_eq!(tail.tail_text(4), "6789");
    }
}
