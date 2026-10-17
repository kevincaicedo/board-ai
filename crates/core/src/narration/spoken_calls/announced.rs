//! Repo reads announced in speech and never made (D-232). In 16 to 18 of 20 repo tours gemma4's
//! first turn said "I will start by listing the root directory" and ended without a call
//! (T-026). The core then asks once more for the call (`engine::nudge`).
//!
//! An announcement is an opener ("I'll", "let me", "I'm going to"), at most
//! [`FILLER_WORDS_MAX`] fillers ("start by", "take a"), a reading verb ("list", "look", "search")
//! and, within [`OBJECT_REACH_WORDS`] words of the same sentence, a word that names part of a
//! repo ("files", "folder", "repo", "structure", "code", a path such as `src/main.rs`). So "I'll
//! draw it", "let me explain" and "I'll list three reasons" are not reads. BoardAI explains
//! algorithms with a repo open too, so words a data structure uses ("tree", "root", "entries",
//! "source", "search for 7") never make one: a false read costs the user a listing in the middle
//! of a hash map. Words are matched as they end, so the result doesn't depend on how the text was
//! split; the state is the last few words. Tests iterate the tables, so a new entry is covered
//! the moment it is added.

/// Who is about to act: the talker, in the words models use to announce a step.
pub const READ_OPENERS: &[&str] = &[
    "i'll",
    "i will",
    "let me",
    "let's",
    "lets",
    "we'll",
    "we will",
    "i'm going to",
    "im going to",
    "i am going to",
    "i'm gonna",
    "we're going to",
    "i'd like to",
    "i need to",
];

/// Words allowed between the opener and the verb ("I will start by listing", "let me take a
/// look", "I'll go ahead and list").
pub const READ_FILLERS: &[&str] = &[
    "start", "begin", "by", "with", "first", "quickly", "now", "just", "go", "ahead", "and",
    "take", "have", "a", "quick", "closer", "then", "also", "briefly", "try", "to", "need", "want",
];

/// The reading verbs, each with the repo tool it implies. Looking around a repo is a listing:
/// it is what a tour that has seen nothing can call first.
pub const READ_VERBS: &[(&str, &str)] = &[
    ("list", "list_dir"),
    ("listing", "list_dir"),
    ("look", "list_dir"),
    ("looking", "list_dir"),
    ("check", "list_dir"),
    ("checking", "list_dir"),
    ("explore", "list_dir"),
    ("exploring", "list_dir"),
    ("examine", "list_dir"),
    ("examining", "list_dir"),
    ("inspect", "list_dir"),
    ("inspecting", "list_dir"),
    ("browse", "list_dir"),
    ("browsing", "list_dir"),
    ("scan", "list_dir"),
    ("scanning", "list_dir"),
    ("see", "list_dir"),
    ("view", "list_dir"),
    ("read", "read_file"),
    ("reading", "read_file"),
    ("open", "read_file"),
    ("opening", "read_file"),
    ("search", "search_code"),
    ("searching", "search_code"),
    ("grep", "search_code"),
    ("find", "search_code"),
    ("locate", "search_code"),
];

/// Words that name part of a repo, and none an algorithm's explanation uses ("tree", "root",
/// "entries", "source", "layout" are left out). A word with a folder or a file extension
/// (`crates/core`, `README.md`) counts as well.
pub const READ_OBJECTS: &[&str] = &[
    "file",
    "files",
    "folder",
    "folders",
    "directory",
    "directories",
    "subdirectory",
    "subdirectories",
    "dir",
    "dirs",
    "repo",
    "repos",
    "repository",
    "codebase",
    "code",
    "structure",
    "project",
    "crate",
    "crates",
    "module",
    "modules",
    "package",
    "packages",
    "readme",
    "src",
];

/// Most fillers between an opener and its verb.
pub const FILLER_WORDS_MAX: usize = 4;

/// How many words after the verb its object may come ("listing the top level of the
/// repository" is six).
pub const OBJECT_REACH_WORDS: usize = 8;

/// Words in the longest opener, which is how many recent words are kept.
const OPENER_WORDS_MAX: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// An opener was heard, and `fillers` fillers since.
    Opened { fillers: usize },
    /// A reading verb was heard `words` words ago.
    Verb { tool: &'static str, words: usize },
}

/// The reads a stream announced, in the order first announced (each tool once, so at most
/// three).
#[derive(Clone, Debug, Default)]
pub(super) struct Announced {
    phase: Phase,
    /// The last words since a sentence ended, normalized; at most `OPENER_WORDS_MAX`.
    recent: Vec<String>,
    reads: Vec<&'static str>,
}

impl Announced {
    pub(super) fn reads(&self) -> &[&'static str] {
        &self.reads
    }

    /// Speech that was released as a whole (a held tool name said after all): its words.
    pub(super) fn text(&mut self, text: &str) {
        for word in text.split_whitespace() {
            self.word(word);
        }
    }

    /// A spoken word ended.
    pub(super) fn word(&mut self, written: &str) {
        let word = normalize(written);
        let ends = ends_sentence(written);
        match std::mem::take(&mut self.phase) {
            Phase::Opened { fillers } => {
                if let Some(tool) = verb(&word) {
                    self.phase = Phase::Verb { tool, words: 0 };
                } else if READ_FILLERS.contains(&word.as_str()) && fillers < FILLER_WORDS_MAX {
                    self.phase = Phase::Opened {
                        fillers: fillers + 1,
                    };
                } else {
                    self.listen(word);
                }
            }
            Phase::Verb { tool, words } => {
                if is_object(&word) {
                    if !self.reads.contains(&tool) {
                        self.reads.push(tool);
                    }
                    self.recent.clear();
                } else {
                    if words + 1 < OBJECT_REACH_WORDS {
                        self.phase = Phase::Verb {
                            tool,
                            words: words + 1,
                        };
                    }
                    // A new announcement may start inside this one's reach.
                    self.listen(word);
                }
            }
            Phase::Idle => self.listen(word),
        }
        if ends {
            self.phase = Phase::Idle;
            self.recent.clear();
        }
    }

    /// `word` joins the recent words; an opener they end with opens an announcement.
    fn listen(&mut self, word: String) {
        if self.recent.len() == OPENER_WORDS_MAX {
            self.recent.remove(0);
        }
        self.recent.push(word);
        let opened = READ_OPENERS.iter().any(|opener| {
            opener.split(' ').count() <= self.recent.len()
                && opener
                    .rsplit(' ')
                    .zip(self.recent.iter().rev())
                    .all(|(part, heard)| part == heard)
        });
        if opened {
            self.phase = Phase::Opened { fillers: 0 };
            self.recent.clear();
        }
    }
}

/// Lowercase, surrounding punctuation stripped, a typographic apostrophe made plain.
fn normalize(word: &str) -> String {
    word.trim_matches(|ch: char| !ch.is_alphanumeric())
        .replace('’', "'")
        .to_lowercase()
}

/// The word ends a sentence: its last mark, closing quotes and brackets aside.
fn ends_sentence(word: &str) -> bool {
    word.trim_end_matches(['"', '\'', '”', '’', ')', ']', '*', '_', '`'])
        .ends_with(['.', '!', '?', ';', ':', '…'])
}

fn verb(word: &str) -> Option<&'static str> {
    READ_VERBS
        .iter()
        .find(|(verb, _)| *verb == word)
        .map(|(_, tool)| *tool)
}

/// A repo noun, or a path: a folder (`crates/core`) or a file with an extension (`main.rs`).
fn is_object(word: &str) -> bool {
    if READ_OBJECTS.contains(&word) || word.contains('/') {
        return true;
    }
    word.rsplit_once('.').is_some_and(|(stem, extension)| {
        stem.len() >= 2
            && (1..=5).contains(&extension.len())
            && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
}
