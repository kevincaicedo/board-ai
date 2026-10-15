//! `read_file`: a line range of a text file, bounded (LLD §5).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

use boardai_core::tools::{ToolFailure, ToolOutput};

use super::repo_path::{RepoRoots, ToolError};
use super::text::{LineRead, read_line_bounded, render_line};
use crate::limits::{
    BINARY_SNIFF_BYTES, READ_FILE_SCAN_MAX_BYTES, TOOL_RESULT_MAX_BYTES, TOOL_RESULT_MAX_LINES,
};

/// Lines `[start_line, end_line]` (1-based, inclusive, as the talker asks for them). Lines cut by
/// the result limit are counted in `more_lines`; with no `end_line` that is every line after
/// the last one returned.
pub(crate) fn read_file(
    roots: &RepoRoots,
    path: &str,
    start_line: Option<u32>,
    end_line: Option<u32>,
) -> Result<ToolOutput, ToolError> {
    let repo_path = roots.resolve(path)?;
    let file = File::open(repo_path.absolute()).map_err(|error| io_error(path, &error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, &error))?;
    if metadata.is_dir() {
        return Err(ToolError::new(
            ToolFailure::InvalidInput,
            format!("{path} is a folder; use list_dir"),
        ));
    }
    let mut reader = BufReader::new(file.take(READ_FILE_SCAN_MAX_BYTES));
    if is_binary(&mut reader).map_err(|error| io_error(path, &error))? {
        return Err(ToolError::new(
            ToolFailure::InvalidInput,
            format!("{path} is a binary file"),
        ));
    }
    let first_line = start_line.unwrap_or(1).max(1);
    let last_line = end_line.unwrap_or(u32::MAX);
    let window = collect_window(&mut reader, first_line, last_line)
        .map_err(|error| io_error(path, &error))?;
    if first_line > window.total_lines && !(first_line == 1 && window.total_lines == 0) {
        return Err(ToolError::new(
            ToolFailure::InvalidInput,
            format!(
                "start_line {first_line} is past the end of {path} ({} lines)",
                window.total_lines
            ),
        ));
    }
    Ok(ToolOutput::File {
        path: repo_path.display().to_owned(),
        first_line,
        lines: window.lines,
        more_lines: window.more_lines,
    })
}

struct Window {
    lines: Vec<String>,
    more_lines: u32,
    total_lines: u32,
}

/// Reads to the end of the requested range, keeping lines until either result bound is reached
/// and counting the rest. Kept lines stay contiguous: once one line doesn't fit, none after it
/// is kept.
fn collect_window(
    reader: &mut impl BufRead,
    first_line: u32,
    last_line: u32,
) -> io::Result<Window> {
    let mut lines = Vec::new();
    let mut kept_bytes = 0_usize;
    let mut full = false;
    let mut more_lines = 0_u32;
    let mut line_number = 0_u32;
    let mut buffer = Vec::new();
    while line_number < last_line {
        let read = read_line_bounded(reader, &mut buffer)?;
        if matches!(read, LineRead::EndOfInput) {
            break;
        }
        line_number = line_number.saturating_add(1);
        if line_number < first_line {
            continue;
        }
        let text = render_line(&buffer, read);
        full = full
            || lines.len() >= TOOL_RESULT_MAX_LINES
            || kept_bytes + text.len() > TOOL_RESULT_MAX_BYTES;
        if full {
            more_lines = more_lines.saturating_add(1);
        } else {
            kept_bytes += text.len();
            lines.push(text);
        }
    }
    Ok(Window {
        lines,
        more_lines,
        total_lines: line_number,
    })
}

/// A NUL in the first bytes marks a binary file, as git does.
pub(crate) fn is_binary(reader: &mut impl BufRead) -> io::Result<bool> {
    let head = reader.fill_buf()?;
    let sniffed = &head[..head.len().min(BINARY_SNIFF_BYTES)];
    Ok(sniffed.contains(&0))
}

pub(crate) fn io_error(path: &str, error: &io::Error) -> ToolError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        ToolFailure::NotFound
    } else {
        ToolFailure::Io
    };
    ToolError::new(kind, format!("{path} could not be read: {error}"))
}
