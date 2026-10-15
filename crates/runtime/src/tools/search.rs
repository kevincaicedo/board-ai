//! `search_code` and `list_dir`: walks that respect `.gitignore` (the `ignore` crate) and stay
//! bounded (LLD §5).

use std::path::Path;

use boardai_core::tools::{DirEntry, SearchHit, ToolFailure, ToolOutput};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use regex::bytes::{Regex, RegexBuilder};

use super::repo_path::{RepoRoots, ToolError};
use super::text::{LineRead, render_line};
use crate::limits::{
    BINARY_SNIFF_BYTES, LIST_ENTRIES_MAX, SEARCH_FILE_MAX_BYTES, SEARCH_FILES_MAX, SEARCH_HITS_MAX,
    SEARCH_REGEX_SIZE_MAX_BYTES, SEARCH_SCAN_MAX_BYTES, TOOL_LINE_MAX_BYTES, TOOL_RESULT_MAX_BYTES,
};

/// A walker over one folder: `.gitignore` applies even outside a git repo, hidden files are
/// included (`.github` matters), `.git` itself is skipped, symlinks are not followed, and the
/// order is by name so results are deterministic.
fn walker(folder: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(folder);
    builder
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(Ord::cmp)
        .filter_entry(|entry| entry.file_name() != ".git");
    builder
}

#[derive(Default)]
struct Hits {
    hits: Vec<SearchHit>,
    kept_bytes: usize,
    more_hits: u32,
    files_opened: usize,
    bytes_scanned: u64,
}

impl Hits {
    fn push(&mut self, hit: SearchHit) {
        let size = hit.path.len() + hit.text.len();
        if self.more_hits == 0
            && self.hits.len() < SEARCH_HITS_MAX
            && self.kept_bytes + size <= TOOL_RESULT_MAX_BYTES
        {
            self.kept_bytes += size;
            self.hits.push(hit);
        } else {
            self.more_hits = self.more_hits.saturating_add(1);
        }
    }

    /// The scan budget is spent: the walk stops and `more_hits` becomes a lower bound.
    fn exhausted(&self) -> bool {
        self.files_opened >= SEARCH_FILES_MAX || self.bytes_scanned >= SEARCH_SCAN_MAX_BYTES
    }
}

pub(crate) fn search_code(
    roots: &RepoRoots,
    query: &str,
    glob: Option<&str>,
) -> Result<ToolOutput, ToolError> {
    if roots.is_empty() {
        return Err(ToolError::new(
            ToolFailure::OutsideRepo,
            "no folder is open",
        ));
    }
    let regex = RegexBuilder::new(query)
        .size_limit(SEARCH_REGEX_SIZE_MAX_BYTES)
        .dfa_size_limit(SEARCH_REGEX_SIZE_MAX_BYTES)
        .build()
        .map_err(|error| {
            ToolError::new(ToolFailure::InvalidInput, format!("bad regex: {error}"))
        })?;
    let mut hits = Hits::default();
    for folder in roots.folders() {
        let mut builder = walker(folder);
        if let Some(glob) = glob {
            let overrides = OverrideBuilder::new(folder)
                .add(glob)
                .and_then(|builder| builder.build())
                .map_err(|error| {
                    ToolError::new(ToolFailure::InvalidInput, format!("bad glob: {error}"))
                })?;
            builder.overrides(overrides);
        }
        for entry in builder.build() {
            if hits.exhausted() {
                break;
            }
            // An unreadable entry (permissions, a racing delete) is skipped, not fatal.
            let Ok(entry) = entry else { continue };
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                search_file(roots, entry.path(), &regex, &mut hits);
            }
        }
    }
    Ok(ToolOutput::Matches {
        hits: hits.hits,
        more_hits: hits.more_hits,
    })
}

fn search_file(roots: &RepoRoots, path: &Path, regex: &Regex, hits: &mut Hits) {
    let size = path.metadata().map_or(u64::MAX, |metadata| metadata.len());
    if size > SEARCH_FILE_MAX_BYTES {
        return;
    }
    let Ok(content) = std::fs::read(path) else {
        return;
    };
    hits.files_opened += 1;
    hits.bytes_scanned = hits.bytes_scanned.saturating_add(size);
    if content[..content.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return;
    }
    let display = roots.display_for(path);
    for (index, line) in content.split(|&byte| byte == b'\n').enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if !regex.is_match(line) {
            continue;
        }
        let (kept, read) = if line.len() > TOOL_LINE_MAX_BYTES {
            (&line[..TOOL_LINE_MAX_BYTES], LineRead::Cut)
        } else {
            (line, LineRead::Complete)
        };
        hits.push(SearchHit {
            path: display.clone(),
            line: u32::try_from(index + 1).unwrap_or(u32::MAX),
            text: render_line(kept, read),
        });
    }
}

pub(crate) fn list_dir(roots: &RepoRoots, path: &str) -> Result<ToolOutput, ToolError> {
    let repo_path = roots.resolve(path)?;
    if !repo_path.absolute().is_dir() {
        return Err(ToolError::new(
            ToolFailure::InvalidInput,
            format!("{path} is not a folder; use read_file"),
        ));
    }
    let mut builder = walker(repo_path.absolute());
    builder.max_depth(Some(1));
    let mut entries = Vec::new();
    let mut more_entries = 0_u32;
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue;
        }
        if entries.len() >= LIST_ENTRIES_MAX {
            more_entries = more_entries.saturating_add(1);
            continue;
        }
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
        });
    }
    Ok(ToolOutput::Listing {
        entries,
        more_entries,
    })
}
