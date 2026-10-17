//! Tool calls gemma4 writes in its own call syntax as text (D-191).
//!
//! gemma4 calls a tool as `<|tool_call>call:list_dir{path:<|"|>.<|"|>}<tool_call|>`, and
//! Ollama's gemma4 parser turns only that, between the two call tokens, into `tool_calls`.
//! When the model leaves out the opening token, the rest arrives as text: `list_dir{path:<|"|>.
//! <|"|>}`, a token at a time (`list`, `_`, `dir`, `{`, `path`, `:`, `<|"|>`, …). In a repo tour
//! that was the read the tour stood on, and nothing ran (D-179 kept it out of speech).
//!
//! This reads exactly that syntax, and only it, back into calls:
//! - an offered tool's name, optionally after `call:`, followed by `{`, at the start of the
//!   response, after whitespace, after a tag's `>` or right after another call: never inside a
//!   word or a tag (`<call:list_dir{…}/>` stays text, D-143);
//! - arguments in gemma's notation: bare keys, strings between `<|"|>` marks, numbers, `true`,
//!   `false`, `null`, objects and arrays. A JSON or single quote is not gemma's;
//! - at least one mark of gemma's syntax: `call:` or a `<|"|>` string. Without one
//!   (`show_library{}`, `list_dir{path:"."}`) it is words, left to the core (D-179);
//! - a `<tool_call|>` right after a call belongs to it.
//!
//! A call read is taken out of the text. One that shows gemma's syntax but can't be read
//! (unclosed at a line end or at the end of the response, longer than
//! `OLLAMA_NATIVE_CALL_MAX_BYTES`, arguments that don't parse) is taken out too, never run,
//! and counted. Everything else is given back unchanged. Decisions are made a character at a
//! time, so how the text was split into chunks never changes the result.

use std::ops::Range;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::limits::{OLLAMA_NATIVE_CALL_MAX_BYTES, OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX};

/// gemma4's string mark, a token of its own.
const STRING_MARK: &str = "<|\"|>";
const CALL_PREFIX: &str = "call:";
const CALL_CLOSE: &str = "<tool_call|>";

/// A call read from the text: an offered tool's name and its arguments.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NativeCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Text. `boundary`: a call may start at the next character.
    Text { boundary: bool },
    /// `held` is a prefix of `call:`, or of an offered tool's name and `{` (after `call:` when
    /// `prefixed`).
    Head { prefixed: bool },
    /// Inside a call's arguments.
    Body(Open),
    /// A call was read; `held` is a prefix of `<tool_call|>` that may close it.
    Close,
}

/// A call whose arguments are open: its name at `held[name]`, its arguments from
/// `held[arguments..]`, `depth` brackets open, inside a `<|"|>` string or not, and whether
/// gemma's syntax (`call:` or a string mark) was seen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    name: Range<usize>,
    arguments: usize,
    depth: u32,
    quoted: bool,
    marked: bool,
}

/// What one character does after it is handled: done, or handle it again as text.
enum Step {
    Done,
    AsText,
}

#[derive(Clone, Debug)]
pub(super) struct NativeCalls {
    names: Arc<[String]>,
    state: State,
    held: String,
    read: usize,
    dropped: u32,
}

impl NativeCalls {
    /// Reads calls to the tools named in `names`, the request's tools.
    pub(super) fn new(names: Arc<[String]>) -> Self {
        Self {
            names,
            state: State::Text { boundary: true },
            held: String::new(),
            read: 0,
            dropped: 0,
        }
    }

    /// Forgets a failed attempt before a retry.
    pub(super) fn reset(&mut self) {
        *self = Self::new(Arc::clone(&self.names));
    }

    /// Calls that showed gemma's syntax and were dropped instead of delivered.
    pub(super) fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Calls read so far.
    pub(super) fn read(&self) -> usize {
        self.read
    }

    /// Handles a piece of streamed text: what is safe to speak goes to `text`, calls read to
    /// `calls`. A possible call is held until it is decided.
    pub(super) fn push(&mut self, piece: &str, text: &mut String, calls: &mut Vec<NativeCall>) {
        for character in piece.chars() {
            if matches!(self.step(character, text, calls), Step::AsText) {
                let step = self.step(character, text, calls);
                debug_assert!(
                    matches!(step, Step::Done),
                    "text never hands a character back"
                );
            }
        }
    }

    /// The response ended: what is held is decided now.
    pub(super) fn finish(&mut self, text: &mut String) {
        match std::mem::replace(&mut self.state, State::Text { boundary: true }) {
            State::Text { .. } => {}
            State::Head { .. } | State::Close => text.push_str(&self.held),
            State::Body(open) => self.give_up(open.marked, text),
        }
        self.held.clear();
    }

    fn step(&mut self, character: char, text: &mut String, calls: &mut Vec<NativeCall>) -> Step {
        match self.state.clone() {
            State::Text { boundary } => {
                if boundary && self.may_start(character) {
                    self.held.push(character);
                    self.state = State::Head { prefixed: false };
                    self.after_head(text);
                } else {
                    text.push(character);
                    self.state = State::Text {
                        boundary: character.is_whitespace() || character == '>',
                    };
                }
                Step::Done
            }
            State::Head { .. } => {
                self.held.push(character);
                self.after_head(text);
                Step::Done
            }
            State::Body(open) => {
                let line_end = character == '\n' && !open.quoted;
                if line_end || self.held.len() + character.len_utf8() > OLLAMA_NATIVE_CALL_MAX_BYTES
                {
                    self.give_up(open.marked, text);
                    self.held.clear();
                    self.state = State::Text { boundary: false };
                    return Step::AsText;
                }
                self.held.push(character);
                self.in_body(open, calls, text);
                Step::Done
            }
            State::Close => {
                let matched = self.held.len();
                if CALL_CLOSE[matched..].starts_with(character) {
                    self.held.push(character);
                    if self.held.len() == CALL_CLOSE.len() {
                        self.held.clear();
                        self.state = State::Text { boundary: true };
                    }
                    return Step::Done;
                }
                text.push_str(&self.held);
                self.held.clear();
                self.state = State::Text {
                    boundary: matched == 0,
                };
                Step::AsText
            }
        }
    }

    fn may_start(&self, character: char) -> bool {
        CALL_PREFIX.starts_with(character)
            || self.names.iter().any(|name| name.starts_with(character))
    }

    /// Decides a head after a character was added to it: still possible, a call's opening
    /// brace, or text after all.
    fn after_head(&mut self, text: &mut String) {
        let State::Head { prefixed } = self.state else {
            return;
        };
        let start = if prefixed { CALL_PREFIX.len() } else { 0 };
        let head = &self.held[start..];
        if !prefixed && head == CALL_PREFIX {
            self.state = State::Head { prefixed: true };
            return;
        }
        if let Some(name) = head.strip_suffix('{')
            && self.names.iter().any(|offered| offered == name)
        {
            self.state = State::Body(Open {
                name: start..start + name.len(),
                arguments: self.held.len() - 1,
                depth: 1,
                quoted: false,
                marked: prefixed,
            });
            return;
        }
        let possible = (!prefixed && CALL_PREFIX.starts_with(head))
            || self.names.iter().any(|offered| {
                head.len() <= offered.len() + 1
                    && offered
                        .as_bytes()
                        .iter()
                        .chain(b"{")
                        .zip(head.as_bytes())
                        .all(|(a, b)| a == b)
            });
        if possible {
            return;
        }
        let last = head.chars().next_back();
        text.push_str(&self.held);
        self.held.clear();
        self.state = State::Text {
            boundary: last.is_some_and(|last| last.is_whitespace() || last == '>'),
        };
    }

    /// Follows the arguments after a character was added to them.
    fn in_body(&mut self, mut open: Open, calls: &mut Vec<NativeCall>, text: &mut String) {
        if self.held.ends_with(STRING_MARK) {
            open.quoted = !open.quoted;
            open.marked = true;
        } else if !open.quoted {
            match self.held.chars().next_back() {
                Some('{' | '[') => open.depth = open.depth.saturating_add(1),
                Some('}' | ']') => open.depth = open.depth.saturating_sub(1),
                Some(_) | None => {}
            }
        }
        if open.depth > 0 {
            self.state = State::Body(open);
            return;
        }
        let call = arguments(&self.held[open.arguments..]).map(|arguments| NativeCall {
            name: self.held[open.name].to_owned(),
            arguments,
        });
        match (open.marked, call) {
            (false, _) => {
                text.push_str(&self.held);
                self.held.clear();
                self.state = State::Text { boundary: false };
                return;
            }
            (true, Some(call)) if self.read < OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX => {
                self.read += 1;
                calls.push(call);
            }
            (true, Some(_) | None) => self.dropped = self.dropped.saturating_add(1),
        }
        self.held.clear();
        self.state = State::Close;
    }

    /// A call that couldn't be read: dropped and counted when it showed gemma's syntax, given
    /// back as text otherwise.
    fn give_up(&mut self, marked: bool, text: &mut String) {
        if marked {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            text.push_str(&self.held);
        }
    }
}

/// gemma's arguments (`{path:<|"|>src<|"|>,start_line:18}`) as a JSON object: each `<|"|>`
/// string becomes a JSON string and each bare key a quoted one, and the result must parse as
/// an object. `None` for anything else, a JSON or single quote included. serde_json bounds the
/// nesting; the input is at most `OLLAMA_NATIVE_CALL_MAX_BYTES`.
fn arguments(body: &str) -> Option<Map<String, Value>> {
    let mut json = String::with_capacity(body.len() + 16);
    let mut rest = body;
    while let Some(character) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix(STRING_MARK) {
            let end = after.find(STRING_MARK)?;
            json.push_str(&serde_json::to_string(&after[..end]).ok()?);
            rest = &after[end + STRING_MARK.len()..];
            continue;
        }
        rest = &rest[character.len_utf8()..];
        match character {
            '"' | '\'' => return None,
            '{' | ',' => {
                json.push(character);
                let key_end = rest
                    .find(|key: char| !(key.is_ascii_alphanumeric() || key == '_'))
                    .unwrap_or(rest.len());
                if key_end > 0 && rest[key_end..].starts_with(':') {
                    json.push('"');
                    json.push_str(&rest[..key_end]);
                    json.push_str("\":");
                    rest = &rest[key_end + 1..];
                }
            }
            other => json.push(other),
        }
    }
    match serde_json::from_str(&json) {
        Ok(Value::Object(object)) => Some(object),
        Ok(_) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests;
