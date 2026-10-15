//! Text from a harness, bounded before it reaches the core: progress notes, the summary, and
//! the `path:line` citations in it.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::limits::{HARNESS_CITATIONS_MAX, HARNESS_PROGRESS_MAX_CHARS, HARNESS_SUMMARY_MAX_BYTES};

/// A progress note: trimmed, one line, cut at a word to the bound. Empty notes are dropped.
pub(crate) fn progress_note(text: &str) -> Option<String> {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() <= HARNESS_PROGRESS_MAX_CHARS {
        return Some(flat);
    }
    let cut: String = flat.chars().take(HARNESS_PROGRESS_MAX_CHARS).collect();
    let at_word = cut.rfind(' ').map_or(cut.as_str(), |space| &cut[..space]);
    Some(format!("{at_word}…"))
}

/// The final summary, cut to the bound at a character boundary.
pub(crate) fn bounded_summary(text: &str) -> String {
    let text = text.trim();
    if text.len() <= HARNESS_SUMMARY_MAX_BYTES {
        return text.to_owned();
    }
    let mut end = HARNESS_SUMMARY_MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[allow(
    clippy::expect_used,
    reason = "the pattern is a constant, compiled by the tests below"
)]
static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s(\[`'])(/?(?:[\w.\-]+/)*[\w.\-]+\.[A-Za-z0-9]+):(\d+)")
        .expect("valid citation pattern")
});

/// `path:line` citations in a summary, relative to the repo. A citation counts only if the
/// file exists inside the repo: never claim what wasn't read (law 6).
pub(crate) fn citations(summary: &str, repo: &Path) -> Vec<String> {
    let Ok(repo) = std::fs::canonicalize(repo) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for capture in CITATION.captures_iter(summary) {
        let (Some(path), Some(line)) = (capture.get(1), capture.get(2)) else {
            continue;
        };
        let relative = relative_to(&repo, path.as_str());
        let inside = std::fs::canonicalize(repo.join(&relative))
            .is_ok_and(|file| file.starts_with(&repo) && file.is_file());
        let citation = format!("{relative}:{}", line.as_str());
        if inside && seen.insert(citation.clone()) {
            found.push(citation);
            if found.len() == HARNESS_CITATIONS_MAX {
                break;
            }
        }
    }
    found
}

/// A path the harness reported, relative to the repo when it is inside it.
pub(crate) fn relative_to(repo: &Path, path: &str) -> String {
    let path = path.strip_prefix("./").unwrap_or(path);
    let as_path = Path::new(path);
    as_path.strip_prefix(repo).map_or_else(
        |_| path.to_owned(),
        |rest| rest.to_string_lossy().into_owned(),
    )
}

/// A random v4 UUID for `--session-id`. The randomness is std's per-process hash keys (seeded
/// by the OS) mixed with time and a counter: unique enough for session ids, no RNG crate.
pub(crate) fn new_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        });
    let state = RandomState::new();
    let mut bytes = [0_u8; 16];
    for (half, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = state.build_hasher();
        hasher.write_u64(sequence);
        hasher.write_u64(nanos);
        hasher.write_u32(std::process::id());
        hasher.write_usize(half);
        chunk.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::{bounded_summary, new_session_id, progress_note};
    use crate::limits::{HARNESS_PROGRESS_MAX_CHARS, HARNESS_SUMMARY_MAX_BYTES};

    #[test]
    fn notes_are_one_bounded_line() {
        assert_eq!(progress_note("  \n "), None);
        assert_eq!(
            progress_note("Reading\n the  router."),
            Some("Reading the router.".into())
        );
        let long = "word ".repeat(200);
        let note = progress_note(&long).unwrap_or_default();
        assert!(note.chars().count() <= HARNESS_PROGRESS_MAX_CHARS + 1);
        assert!(note.ends_with("word…"));
        let summary = bounded_summary(&"é".repeat(HARNESS_SUMMARY_MAX_BYTES));
        assert!(summary.len() <= HARNESS_SUMMARY_MAX_BYTES + '…'.len_utf8());
    }

    #[test]
    fn session_ids_are_distinct_v4_uuids() {
        let first = new_session_id();
        let second = new_session_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }
}
