//! Tool calls written as words (D-179). Asked to walk through a repo, gemma4 wrote the call as
//! speech, `list_dir path "."`, and BoardAI read it aloud while nothing ran. A call written as a
//! tag (`<list_dir path="."/>`) is the parser's (D-143); this is the plain-text form:
//! - a talker tool's name followed by a bracket: `read_file("src/main.rs")`, `recall({…})`;
//! - a tool name with an underscore followed by its arguments: `list_dir path "."`,
//!   `search_code query="TODO"`, where an argument is a quoted value, `key=…`, `key:` or a bare
//!   parameter name of that tool.
//!
//! Such a call never reaches speech or captions. Plain words that name a tool ("research",
//! "review", "remember", "recall") count only with a bracket, and a tool name alone ("list_dir
//! shows folders") stays speech. Decisions are made a word at a time as words end (whitespace, a
//! tag, the end of the stream), so the result doesn't depend on how the text was split. A call
//! ends at the first word that isn't an argument, a closing bracket, a line end, a tag, or
//! `SPOKEN_CALL_MAX_BYTES`.

use crate::limits::{SPOKEN_CALL_MAX_BYTES, TOOL_CALLS_PER_RESPONSE_MAX};

/// Each talker tool and its parameters, as the tool schemas define them (a test holds them to
/// the schemas).
pub const SPOKEN_TOOLS: &[(&str, &[&str])] = &[
    ("read_file", &["path", "start_line", "end_line"]),
    ("search_code", &["query", "glob"]),
    ("list_dir", &["path"]),
    ("research", &["question", "depth"]),
    ("review", &["scope"]),
    ("propose_change", &["instruction"]),
    ("remember", &["kind", "text", "about", "links"]),
    ("add_task", &["title", "project", "links"]),
    ("recall", &["query"]),
    ("open_session", &["id"]),
    ("show_library", &[]),
    ("show_tasks", &["project"]),
    ("show_memory", &[]),
    ("play_session", &["id"]),
];

/// A call kept out of speech: which tool, and what was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpokenCall {
    pub tool: &'static str,
    pub written: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Speech,
    /// A tool's name was read: it and the whitespace after it wait for the next word.
    Name {
        tool: usize,
        held: String,
    },
    /// Inside a call: its words are dropped. An open quote or bracket carries across words;
    /// `space` is whitespace seen outside them, and `bytes` what the call has taken so far.
    Call {
        tool: usize,
        quote: Option<char>,
        depth: u32,
        space: bool,
        bytes: usize,
    },
}

/// What becomes of a word that ended: `released` is said first (a held tool name that turned
/// out to be speech), then the word itself when `speak` is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WordFate {
    pub released: String,
    pub speak: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SpokenCalls {
    state: State,
    calls: Vec<SpokenCall>,
    /// The call being written is the last one recorded (calls past the bound aren't).
    recording: bool,
}

impl Default for SpokenCalls {
    fn default() -> Self {
        Self {
            state: State::Speech,
            calls: Vec::new(),
            recording: false,
        }
    }
}

impl SpokenCalls {
    /// The calls kept out of speech so far, at most `TOOL_CALLS_PER_RESPONSE_MAX`.
    pub(super) fn calls(&self) -> &[SpokenCall] {
        &self.calls
    }

    /// A word ended.
    pub(super) fn word(&mut self, word: &str) -> WordFate {
        match std::mem::replace(&mut self.state, State::Speech) {
            State::Speech => self.start(word, String::new()),
            State::Name { tool, held } => {
                if is_argument(tool, word) {
                    let bytes = held.len();
                    self.record(tool, held);
                    self.continue_call(tool, (None, 0, bytes), word);
                    silent()
                } else {
                    self.start(word, held)
                }
            }
            State::Call {
                tool,
                quote,
                depth,
                space,
                bytes,
            } => {
                if quote.is_some() || depth > 0 || is_argument(tool, word) {
                    if space {
                        self.append(" ");
                    }
                    self.continue_call(tool, (quote, depth, bytes), word);
                    silent()
                } else {
                    self.start(word, String::new())
                }
            }
        }
    }

    /// Whitespace after a word: what to say now. Inside a call it is dropped; after a held name
    /// it waits with it. A line end ends both.
    pub(super) fn whitespace(&mut self, ch: char) -> String {
        let inside = match &mut self.state {
            State::Speech => return ch.to_string(),
            State::Name { held, .. } => {
                if ch != '\n' && held.len() < SPOKEN_CALL_MAX_BYTES {
                    held.push(ch);
                    return String::new();
                }
                let mut released = self.boundary();
                released.push(ch);
                return released;
            }
            State::Call { .. } if ch == '\n' => {
                self.state = State::Speech;
                return ch.to_string();
            }
            State::Call {
                quote,
                depth,
                space,
                ..
            } => {
                let inside = quote.is_some() || *depth > 0;
                *space |= !inside;
                inside
            }
        };
        if inside {
            self.append(ch.encode_utf8(&mut [0; 4]));
        }
        String::new()
    }

    /// A tag, a word too long to read, or the end of the stream: a held name is said after all,
    /// and a call ends. Returns what to say.
    pub(super) fn boundary(&mut self) -> String {
        match std::mem::replace(&mut self.state, State::Speech) {
            State::Name { held, .. } => held,
            State::Speech | State::Call { .. } => String::new(),
        }
    }

    /// `word` in speech, after `released`: it may start a call or hold a tool's name.
    fn start(&mut self, word: &str, released: String) -> WordFate {
        let Some((tool, rest)) = tool_name(word) else {
            return WordFate {
                released,
                speak: true,
            };
        };
        let (name, _) = SPOKEN_TOOLS[tool];
        if rest.starts_with(['(', '{']) {
            self.record(tool, String::new());
            self.continue_call(tool, (None, 0, 0), word);
            return WordFate {
                released,
                speak: false,
            };
        }
        // Only a name no sentence uses ("list_dir") waits to see whether arguments follow.
        if rest.trim_matches(['`', ':']).is_empty() && name.contains('_') {
            self.state = State::Name {
                tool,
                held: word.to_owned(),
            };
            return WordFate {
                released,
                speak: false,
            };
        }
        WordFate {
            released,
            speak: true,
        }
    }

    fn record(&mut self, tool: usize, written: String) {
        self.recording = self.calls.len() < TOOL_CALLS_PER_RESPONSE_MAX;
        if self.recording {
            self.calls.push(SpokenCall {
                tool: SPOKEN_TOOLS[tool].0,
                written,
            });
        }
    }

    /// Appends to the call being written, when it was recorded.
    fn append(&mut self, text: &str) {
        if let Some(call) = self.calls.last_mut().filter(|_| self.recording) {
            call.written.push_str(text);
        }
    }

    /// `word` belongs to the call: track its quotes and brackets across words. A closed bracket
    /// doesn't end the call (the rest of its word goes with it, and so does the whitespace
    /// after), the first word that isn't an argument does; so does the bound.
    fn continue_call(&mut self, tool: usize, open: (Option<char>, u32, usize), word: &str) {
        let (mut quote, mut depth, bytes) = open;
        self.append(word);
        for ch in word.chars() {
            match (quote, ch) {
                (Some(open), _) if ch == open => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(ch),
                (None, '(' | '{' | '[') => depth += 1,
                (None, ')' | '}' | ']') => depth = depth.saturating_sub(1),
                (None, _) => {}
            }
        }
        let bytes = bytes + word.len() + 1;
        self.state = if bytes >= SPOKEN_CALL_MAX_BYTES {
            State::Speech
        } else {
            State::Call {
                tool,
                quote,
                depth,
                space: false,
                bytes,
            }
        };
    }
}

fn silent() -> WordFate {
    WordFate {
        released: String::new(),
        speak: false,
    }
}

/// The tool a word names, and what follows the name in the word (`read_file("x")` → `("x")`).
/// A leading backtick is allowed; the name must end where an identifier would.
fn tool_name(word: &str) -> Option<(usize, &str)> {
    let core = word.trim_start_matches('`');
    SPOKEN_TOOLS
        .iter()
        .enumerate()
        .find_map(|(index, (name, _))| {
            let rest = core.strip_prefix(name)?;
            let continues = rest
                .chars()
                .next()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
            (!continues).then_some((index, rest))
        })
}

/// A word that is one of `tool`'s arguments: a quoted value, a bracket, `key=…`, `key:`,
/// `--key` or a bare parameter name of the tool.
fn is_argument(tool: usize, word: &str) -> bool {
    let word = word.trim_matches(['`', ',', ';']);
    if word.starts_with(['"', '\'', '(', '{', '[']) {
        return true;
    }
    let key = word
        .trim_start_matches('-')
        .split(['=', ':'])
        .next()
        .unwrap_or_default();
    SPOKEN_TOOLS[tool].1.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::{SPOKEN_TOOLS, SpokenCalls, tool_name};
    use crate::limits::SPOKEN_CALL_MAX_BYTES;
    use crate::tools::TOOL_NAMES;

    /// The table follows the schemas: every tool, every parameter, in order.
    #[test]
    fn the_table_matches_the_tool_schemas() {
        let names: Vec<&str> = SPOKEN_TOOLS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, TOOL_NAMES.to_vec());
        for schema in crate::tools::schemas() {
            let mut properties: Vec<&str> = schema.input_schema["properties"]
                .as_object()
                .map(|object| object.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let mut table: Vec<&str> = SPOKEN_TOOLS
                .iter()
                .find(|(name, _)| *name == schema.name)
                .map(|(_, parameters)| parameters.to_vec())
                .unwrap_or_default();
            properties.sort_unstable();
            table.sort_unstable();
            assert_eq!(table, properties, "{}", schema.name);
        }
    }

    /// Speech left after feeding `text` word by word.
    fn speech(text: &str) -> (String, Vec<String>) {
        let mut calls = SpokenCalls::default();
        let mut out = String::new();
        let mut word = String::new();
        for ch in text.chars().chain(std::iter::once('\n')) {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    let fate = calls.word(&word);
                    out.push_str(&fate.released);
                    if fate.speak {
                        out.push_str(&word);
                    }
                    word.clear();
                }
                out.push_str(&calls.whitespace(ch));
            } else {
                word.push(ch);
            }
        }
        out.push_str(&calls.boundary());
        let written = calls
            .calls()
            .iter()
            .map(|call| call.written.clone())
            .collect();
        (out.trim_end().to_owned(), written)
    }

    #[test]
    fn calls_written_as_words_are_kept_out_of_speech() {
        for (text, spoken, call) in [
            (
                r#"Let me look. list_dir path "." Then I read."#,
                "Let me look. Then I read.",
                r#"list_dir path ".""#,
            ),
            (
                r#"Open read_file("src/a b.rs") now."#,
                "Open now.",
                r#"read_file("src/a b.rs")"#,
            ),
            (
                r#"`search_code query="TODO"` finds it."#,
                "finds it.",
                r#"`search_code query="TODO"`"#,
            ),
            (
                r#"Later research(question="why", depth="quick") helps."#,
                "Later helps.",
                r#"research(question="why", depth="quick")"#,
            ),
            (
                r#"list_dir {"path": "crates"} shows it."#,
                "shows it.",
                r#"list_dir {"path": "crates"}"#,
            ),
        ] {
            let (said, calls) = speech(text);
            assert_eq!(said, spoken, "{text}");
            assert_eq!(calls, vec![call.to_owned()], "{text}");
        }
    }

    #[test]
    fn words_that_only_name_a_tool_stay_speech() {
        for text in [
            "Research shows caches help, and I recall the review.",
            r#"Please remember kind="note" there."#,
            "list_dir shows the folders.",
            "read_files are not tools.",
            "The list_dir",
        ] {
            let (said, calls) = speech(text);
            assert_eq!(said, text, "{text}");
            assert!(calls.is_empty(), "{text}");
        }
    }

    #[test]
    fn a_call_ends_at_a_line_end_and_at_its_bound() {
        let (said, _) = speech("read_file(\"a\",\nstart_line=1) spans lines.");
        assert_eq!(said, "\nstart_line=1) spans lines.");
        let long = format!(
            "read_file(\"{}\" and more words",
            "x ".repeat(SPOKEN_CALL_MAX_BYTES)
        );
        let (said, calls) = speech(&long);
        assert!(
            said.ends_with("and more words") || said.contains("x x"),
            "{said}"
        );
        assert!(
            calls[0].len() <= SPOKEN_CALL_MAX_BYTES + 8,
            "{}",
            calls[0].len()
        );
        assert_eq!(tool_name("read_file_x"), None);
    }
}
