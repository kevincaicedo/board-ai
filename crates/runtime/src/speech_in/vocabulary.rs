//! Repo symbols for speech-to-text's vocabulary (LLD §3.4): the names a user is likely to say
//! about the opened code ("SessionChannels", "speech_out", "Redis"), so Whisper spells them as
//! the code does. Recent board terms come first in the prompt; these follow, best first.
//!
//! The scan is cheap and bounded: source files only, respecting `.gitignore`, at most
//! [`VOCABULARY_SCAN_FILES_MAX`] files and [`VOCABULARY_SCAN_BYTES_MAX`] bytes. A name counts
//! when the code defines it (it follows `fn`, `struct`, `class`, `def`…) and it looks like a
//! symbol rather than an English word (mixed case, an underscore or a digit); names are ranked by
//! how often they appear.

use std::collections::{BTreeSet, HashMap};
use std::io::Read as _;
use std::path::PathBuf;

use crate::limits::{
    VOCABULARY_IDENTIFIERS_MAX, VOCABULARY_REPO_TERMS_MAX, VOCABULARY_SCAN_BYTES_MAX,
    VOCABULARY_SCAN_FILE_MAX_BYTES, VOCABULARY_SCAN_FILES_MAX, VOCABULARY_TERM_MAX_BYTES,
};

/// Extensions read as source code.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "go", "py", "java", "kt", "swift", "rb", "c", "h", "cc",
    "cpp", "hpp", "cs", "scala", "ex", "exs", "php", "proto", "sql",
];

/// Words that introduce a definition in the languages above.
const DEFINING_WORDS: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "trait",
    "type",
    "impl",
    "mod",
    "class",
    "interface",
    "def",
    "func",
    "function",
    "const",
    "message",
    "service",
    "module",
    "object",
    "record",
    "protocol",
];

/// Shortest name worth biasing recognition towards.
const TERM_MIN_BYTES: usize = 4;

/// The opened repos' best symbols, at most [`VOCABULARY_REPO_TERMS_MAX`], each folder's own name
/// first. Blocking: run it off the async runtime.
#[must_use]
pub fn repo_terms(roots: &[PathBuf]) -> Vec<String> {
    let mut scan = Scan::default();
    let mut terms: Vec<String> = roots
        .iter()
        .filter_map(|root| root.file_name()?.to_str().map(str::to_owned))
        .collect();
    for root in roots {
        scan.walk(root);
    }
    terms.extend(scan.ranked());
    let mut seen = BTreeSet::new();
    terms.retain(|term| seen.insert(term.to_ascii_lowercase()));
    terms.truncate(VOCABULARY_REPO_TERMS_MAX);
    terms
}

#[derive(Default)]
struct Scan {
    files: usize,
    bytes: u64,
    counts: HashMap<String, u32>,
    defined: BTreeSet<String>,
}

impl Scan {
    fn walk(&mut self, root: &PathBuf) {
        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            if self.files >= VOCABULARY_SCAN_FILES_MAX || self.bytes >= VOCABULARY_SCAN_BYTES_MAX {
                return;
            }
            let path = entry.path();
            let source = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension));
            if !source || !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut text = String::new();
            if file
                .take(VOCABULARY_SCAN_FILE_MAX_BYTES)
                .read_to_string(&mut text)
                .is_err()
            {
                continue;
            }
            self.files += 1;
            self.bytes += text.len() as u64;
            self.read(&text);
        }
    }

    /// Counts identifiers and notes the defined ones; no allocation for names already known.
    fn read(&mut self, text: &str) {
        let mut previous = "";
        for word in identifiers(text) {
            if DEFINING_WORDS.contains(&previous) && is_symbol(word) {
                self.defined.insert(word.to_owned());
            }
            if let Some(count) = self.counts.get_mut(word) {
                *count += 1;
            } else if self.counts.len() < VOCABULARY_IDENTIFIERS_MAX {
                self.counts.insert(word.to_owned(), 1);
            }
            previous = word;
        }
    }

    /// Defined symbols, most used first; ties in name order so the result is stable.
    fn ranked(&self) -> Vec<String> {
        let mut ranked: Vec<(u32, &String)> = self
            .defined
            .iter()
            .map(|name| (self.counts.get(name).copied().unwrap_or(0), name))
            .collect();
        ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
        ranked.into_iter().map(|(_, name)| name.clone()).collect()
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*` runs of `text`.
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|word| word.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_'))
}

/// Looks like a symbol rather than a word: long enough, and with an inner capital, an
/// underscore or a digit ("getUserById", "speech_out", "S3Client", "Redis" does not qualify by
/// shape but "RedisCache" does).
fn is_symbol(word: &str) -> bool {
    let inner_capital = word.chars().skip(1).any(|ch| ch.is_ascii_uppercase());
    let marked = word.contains('_') || word.chars().any(|ch| ch.is_ascii_digit());
    let lower_after_upper = word.chars().any(|ch| ch.is_ascii_lowercase());
    (TERM_MIN_BYTES..=VOCABULARY_TERM_MAX_BYTES).contains(&word.len())
        && (inner_capital || marked)
        && lower_after_upper
        && !word.trim_matches('_').is_empty()
}

#[cfg(test)]
mod tests {
    use super::{Scan, is_symbol, repo_terms};

    #[test]
    fn defined_symbols_rank_by_use() {
        let mut scan = Scan::default();
        scan.read(
            "pub struct SessionChannels { inbox: Inbox }\n\
             fn speech_in(channels: &SessionChannels) -> SpeechInLink { channels.speech_in() }\n\
             fn run() {}\n\
             const MAX_LINES: usize = 3;\n\
             let x = SpeechInLink::new(); let y = SpeechInLink::new();",
        );
        // `run` is a word and `MAX_LINES` has no lowercase; `SpeechInLink` is used, not defined.
        assert_eq!(scan.ranked(), vec!["SessionChannels", "speech_in"]);
    }

    #[test]
    fn symbols_have_a_shape_words_do_not() {
        for symbol in ["getUserById", "speech_out", "S3Client", "RedisCache"] {
            assert!(is_symbol(symbol), "{symbol}");
        }
        for word in ["run", "Redis", "request", "MAX_LINES", "____", "a_b"] {
            assert!(!is_symbol(word), "{word}");
        }
    }

    #[test]
    fn a_folder_scan_is_bounded_and_names_the_folder_first() {
        let folder =
            std::env::temp_dir().join(format!("boardai-vocabulary-{}", std::process::id()));
        let source = folder.join("src");
        std::fs::create_dir_all(&source).expect("folder");
        std::fs::write(
            source.join("lib.rs"),
            "pub struct VoiceGain; fn duck_voice() {}",
        )
        .expect("write");
        std::fs::write(source.join("notes.txt"), "struct NotCode;").expect("write");
        let terms = repo_terms(std::slice::from_ref(&folder));
        let name = folder
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        assert_eq!(terms, vec![name, "VoiceGain", "duck_voice"]);
        std::fs::remove_dir_all(&folder).expect("cleanup");
    }
}
