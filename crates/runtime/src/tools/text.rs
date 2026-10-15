//! Bounded line reading shared by the tools: memory per line stays bounded even for a
//! minified file with one enormous line.

use std::io::{self, BufRead};

use crate::limits::TOOL_LINE_MAX_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    /// A whole line is in the buffer.
    Complete,
    /// The line was longer than the bound; the buffer holds its start.
    Cut,
    EndOfInput,
}

/// Reads one line into `buffer` (without its line ending), keeping at most
/// [`TOOL_LINE_MAX_BYTES`] bytes and consuming the rest of the line.
pub(crate) fn read_line_bounded(
    reader: &mut impl BufRead,
    buffer: &mut Vec<u8>,
) -> io::Result<LineRead> {
    buffer.clear();
    let mut cut = false;
    let mut consumed_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        consumed_any = true;
        let (take, found_newline) = match available.iter().position(|&byte| byte == b'\n') {
            Some(newline) => (newline, true),
            None => (available.len(), false),
        };
        let room = TOOL_LINE_MAX_BYTES.saturating_sub(buffer.len());
        buffer.extend_from_slice(&available[..take.min(room)]);
        cut |= take > room;
        reader.consume(if found_newline { take + 1 } else { take });
        if found_newline {
            break;
        }
    }
    if !consumed_any {
        return Ok(LineRead::EndOfInput);
    }
    if buffer.last() == Some(&b'\r') && !cut {
        buffer.pop();
    }
    Ok(if cut {
        LineRead::Cut
    } else {
        LineRead::Complete
    })
}

/// A line as text: invalid UTF-8 is replaced, and a cut line ends with `…`.
pub(crate) fn render_line(bytes: &[u8], read: LineRead) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if matches!(read, LineRead::Cut) {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{LineRead, read_line_bounded, render_line};
    use crate::limits::TOOL_LINE_MAX_BYTES;

    #[test]
    fn long_lines_are_cut_and_short_ones_kept_whole() {
        let long = "x".repeat(TOOL_LINE_MAX_BYTES * 3);
        let input = format!("short\r\n{long}\nlast");
        let mut reader = Cursor::new(input.into_bytes());
        let mut buffer = Vec::new();
        let read = read_line_bounded(&mut reader, &mut buffer);
        assert_eq!(
            (read.ok(), buffer.as_slice()),
            (Some(LineRead::Complete), &b"short"[..])
        );
        let read = read_line_bounded(&mut reader, &mut buffer);
        assert_eq!(read.ok(), Some(LineRead::Cut));
        assert_eq!(buffer.len(), TOOL_LINE_MAX_BYTES);
        assert!(render_line(&buffer, LineRead::Cut).ends_with('…'));
        let read = read_line_bounded(&mut reader, &mut buffer);
        assert_eq!(
            (read.ok(), buffer.as_slice()),
            (Some(LineRead::Complete), &b"last"[..])
        );
        let read = read_line_bounded(&mut reader, &mut buffer);
        assert_eq!(read.ok(), Some(LineRead::EndOfInput));
    }
}
