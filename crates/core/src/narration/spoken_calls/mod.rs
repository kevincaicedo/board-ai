//! Tool calls written as words (D-179, D-233). Asked to walk through a repo, gemma4 wrote the
//! call as speech, `list_dir path "."`, and BoardAI read it aloud while nothing ran. A call
//! written as a tag (`<list_dir path="."/>`) is the parser's (D-143); this is the plain-text form:
//! - a talker tool's name followed by a bracket: `read_file("src/main.rs")`, `recall({…})`, also
//!   right after gemma4's `tool_code:` or `call:` (`tool_code:list_dir{path:<|"|>.<|"|>}`);
//! - a tool name with an underscore followed by its arguments: `list_dir path "."`,
//!   `search_code query="TODO"`, where an argument is a quoted value, `key=…`, `key:` or a bare
//!   parameter name of that tool;
//! - a tool-code block (gemma4, T-026): `tool_code` or `[tool_code]`, then calls (bare or inside
//!   `print(…)`), across lines, up to its closing word (`end_tool_code`, `[/tool_code]`). The
//!   opening word counts only when a call or its closing word comes next, so an empty block
//!   (Ollama's adapter took its call out, D-191) goes too.
//!
//! Such a call never reaches speech or captions. Plain words that name a tool ("research",
//! "review", "remember", "recall") count only with a bracket, and a tool name alone ("list_dir
//! shows folders") stays speech. Decisions are made a word at a time as words end (whitespace, a
//! tag, the end of the stream), so the result doesn't depend on how the text was split. A call
//! ends at the first word that isn't an argument (in a block: nor another call or the closing
//! word), a closing bracket, a line end outside a block, a tag, or `SPOKEN_CALL_MAX_BYTES`.
//!
//! gemma4's channel tokens left unpaired in its text (`<channel|>`) are taken out of the word
//! they are written in, and the spoken words feed [`announced`], the reads speech announced
//! (D-232).

mod announced;
#[cfg(test)]
mod tests;

use crate::limits::{SPOKEN_CALL_MAX_BYTES, TOOL_CALLS_PER_RESPONSE_MAX};
use announced::Announced;
pub use announced::{
    FILLER_WORDS_MAX, OBJECT_REACH_WORDS, READ_FILLERS, READ_OBJECTS, READ_OPENERS, READ_VERBS,
};

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

/// What gemma4 writes right before a call's name (T-026, D-191).
pub const CALL_PREFIXES: &[&str] = &["tool_code:", "call:"];

/// A tool-code block's opening word and its closing words, as gemma4 writes them (T-026). The
/// opening word may be in backticks or end in a colon.
pub const TOOL_CODE_BLOCKS: &[(&str, &[&str])] = &[
    ("tool_code", &["end_tool_code", "```"]),
    ("[tool_code]", &["[/tool_code]"]),
];

/// gemma4's channel tokens: its Ollama parser leaves one in the text when it comes unpaired
/// ("…the repository structure.<channel|>", T-026).
pub const STRAY_TOKENS: &[&str] = &["<|channel>", "<channel|>"];

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
    /// A tool-code block's opening word (`block`, an index into `TOOL_CODE_BLOCKS`) was read: it
    /// and the whitespace after it, line ends included, wait for the next word.
    BlockOpen {
        block: usize,
        held: String,
    },
    /// Inside a call: its words are dropped. `space` is whitespace seen outside its quotes and
    /// brackets since its last word.
    Call {
        tool: usize,
        open: Open,
        space: bool,
    },
}

/// What a call being written has open, across words, and how far it has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    quote: Option<char>,
    depth: u32,
    /// What the call (in a block: the block) has taken so far.
    bytes: usize,
    /// The tool-code block the call is in.
    block: Option<usize>,
}

impl Open {
    fn new(bytes: usize, block: Option<usize>) -> Self {
        Self {
            quote: None,
            depth: 0,
            bytes,
            block,
        }
    }

    fn inside(&self) -> bool {
        self.quote.is_some() || self.depth > 0
    }
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
    announced: Announced,
}

impl Default for SpokenCalls {
    fn default() -> Self {
        Self {
            state: State::Speech,
            calls: Vec::new(),
            recording: false,
            announced: Announced::default(),
        }
    }
}

impl SpokenCalls {
    /// The calls kept out of speech so far, at most `TOOL_CALLS_PER_RESPONSE_MAX`.
    pub(super) fn calls(&self) -> &[SpokenCall] {
        &self.calls
    }

    /// The repo reads the speech so far announced (D-232), each tool once, first first.
    pub(super) fn announced(&self) -> &[&'static str] {
        self.announced.reads()
    }

    /// A word ended.
    pub(super) fn word(&mut self, word: &str) -> WordFate {
        let fate = self.decide(word);
        self.announced.text(&fate.released);
        if fate.speak {
            self.announced.word(word);
        }
        fate
    }

    /// Whitespace after a word: what to say now. Inside a call it is dropped; after a held name
    /// it waits with it. A line end ends both, except in a tool-code block.
    pub(super) fn whitespace(&mut self, ch: char) -> String {
        let inside = match &mut self.state {
            State::Speech => return ch.to_string(),
            State::Name { held, .. } if ch != '\n' && held.len() < SPOKEN_CALL_MAX_BYTES => {
                held.push(ch);
                return String::new();
            }
            State::BlockOpen { held, .. } if held.len() < SPOKEN_CALL_MAX_BYTES => {
                held.push(ch);
                return String::new();
            }
            State::Name { .. } | State::BlockOpen { .. } => {
                let mut released = self.boundary();
                released.push(ch);
                return released;
            }
            State::Call { open, .. } if ch == '\n' && open.block.is_none() => {
                self.state = State::Speech;
                return ch.to_string();
            }
            State::Call { open, space, .. } => {
                *space |= !open.inside();
                if open.inside() {
                    open.bytes += ch.len_utf8();
                }
                open.inside()
            }
        };
        if inside {
            self.append(ch.encode_utf8(&mut [0; 4]));
            self.check_bound();
        }
        String::new()
    }

    /// A tag, a word too long to read, or the end of the stream: a held name is said after all,
    /// and a call ends. Returns what to say.
    pub(super) fn boundary(&mut self) -> String {
        let released = match std::mem::replace(&mut self.state, State::Speech) {
            State::Name { held, .. } | State::BlockOpen { held, .. } => held,
            State::Speech | State::Call { .. } => String::new(),
        };
        self.announced.text(&released);
        released
    }

    fn decide(&mut self, word: &str) -> WordFate {
        match std::mem::replace(&mut self.state, State::Speech) {
            State::Speech => self.start(word, String::new()),
            State::Name { tool, held } => {
                if !is_argument(tool, word) {
                    return self.start(word, held);
                }
                let bytes = held.len();
                self.record(tool, held);
                self.continue_call(tool, Open::new(bytes, None), word);
                silent()
            }
            State::BlockOpen { block, held } => {
                if is_block_close(block, word) {
                    return silent();
                }
                let Some(tool) = block_call(word) else {
                    return self.start(word, held);
                };
                self.record(tool, String::new());
                self.continue_call(tool, Open::new(held.len(), Some(block)), word);
                silent()
            }
            State::Call { tool, open, space } => self.in_call(tool, open, space, word),
        }
    }

    /// A word after a call's word: more of it, in a block the next call or the closing word,
    /// or speech again.
    fn in_call(&mut self, tool: usize, open: Open, space: bool, word: &str) -> WordFate {
        // `[/tool_code]` would read as an argument (a bracket): the closing word goes first.
        let closes = open
            .block
            .is_some_and(|block| !open.inside() && is_block_close(block, word));
        if closes {
            return silent();
        }
        if open.inside() || is_argument(tool, word) {
            if space {
                self.append(" ");
            }
            self.continue_call(tool, open, word);
            return silent();
        }
        if open.block.is_none() {
            return self.start(word, String::new());
        }
        let Some(next) = block_call(word) else {
            return self.start(word, String::new());
        };
        self.record(next, String::new());
        self.continue_call(next, Open::new(open.bytes, open.block), word);
        silent()
    }

    /// `word` in speech, after `released`: it may start a call or a block, or hold a tool's
    /// name.
    fn start(&mut self, word: &str, released: String) -> WordFate {
        if let Some(block) = block_open(word) {
            self.state = State::BlockOpen {
                block,
                held: word.to_owned(),
            };
            return WordFate {
                released,
                speak: false,
            };
        }
        let Some((tool, rest)) = tool_name(word) else {
            return WordFate {
                released,
                speak: true,
            };
        };
        if rest.starts_with(['(', '{']) {
            self.record(tool, String::new());
            self.continue_call(tool, Open::new(0, None), word);
            return WordFate {
                released,
                speak: false,
            };
        }
        // Only a name no sentence uses ("list_dir") waits to see whether arguments follow.
        if is_bare_name(tool, rest) {
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
    fn continue_call(&mut self, tool: usize, open: Open, word: &str) {
        let Open {
            mut quote,
            mut depth,
            bytes,
            block,
        } = open;
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
        self.state = State::Call {
            tool,
            open: Open {
                quote,
                depth,
                bytes: bytes + word.len() + 1,
                block,
            },
            space: false,
        };
        self.check_bound();
    }

    /// Past `SPOKEN_CALL_MAX_BYTES` the call (or its block) is taken to have ended.
    fn check_bound(&mut self) {
        if let State::Call { open, .. } = &self.state
            && open.bytes >= SPOKEN_CALL_MAX_BYTES
        {
            self.state = State::Speech;
        }
    }
}

/// The word without gemma4's stray channel tokens; empty when it was only those.
pub(super) fn without_stray_tokens(mut word: String) -> String {
    // Each pass shortens the word, so this ends.
    while let Some(token) = STRAY_TOKENS.iter().find(|token| word.contains(**token)) {
        word = word.replace(token, "");
    }
    word
}

fn silent() -> WordFate {
    WordFate {
        released: String::new(),
        speak: false,
    }
}

/// The tool a word names, and what follows the name in the word (`read_file("x")` → `("x")`).
/// A leading backtick and one of gemma4's `CALL_PREFIXES` are allowed; the name must end where
/// an identifier would.
fn tool_name(word: &str) -> Option<(usize, &str)> {
    let core = word.trim_start_matches('`');
    let core = CALL_PREFIXES
        .iter()
        .find_map(|prefix| core.strip_prefix(prefix))
        .unwrap_or(core);
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

/// A tool's name with nothing after it, which only arguments can make a call: one no sentence
/// uses (`list_dir`, not "research").
fn is_bare_name(tool: usize, rest: &str) -> bool {
    rest.trim_matches(['`', ':']).is_empty() && SPOKEN_TOOLS[tool].0.contains('_')
}

/// The tool-code block a word opens.
fn block_open(word: &str) -> Option<usize> {
    let word = word.trim_matches('`').trim_end_matches(':');
    TOOL_CODE_BLOCKS
        .iter()
        .position(|(opening, _)| *opening == word)
}

/// The word closes `block`.
fn is_block_close(block: usize, word: &str) -> bool {
    let word = word.trim_end_matches(['.', ',', ';']);
    TOOL_CODE_BLOCKS
        .get(block)
        .is_some_and(|(_, closing)| closing.contains(&word))
}

/// The tool a word in a tool-code block calls: a call, bare or inside `print(…)`, or a bare
/// name its arguments follow.
fn block_call(word: &str) -> Option<usize> {
    let core = word.trim_start_matches('`');
    let core = core.strip_prefix("print(").unwrap_or(core);
    let (tool, rest) = tool_name(core)?;
    (rest.starts_with(['(', '{']) || is_bare_name(tool, rest)).then_some(tool)
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
