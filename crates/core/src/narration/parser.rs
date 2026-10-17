//! The streaming tag parser (LLD §2.3): an explicit, bounded character state machine.
//!
//! Text outside tags is speech. Tags are self-closing (`<node id="lb"/>`; a missing `/` is
//! tolerated) and never nest. The parser works one char at a time and keeps every partial tag in
//! its own buffers, so feeding any split of the input gives the same pieces as feeding it whole.
//!
//! Tolerance rules, in order:
//! - `<` not followed by a letter or `/` is speech ("a < b").
//! - A malformed tag that doesn't look like one (an unknown name and no attribute yet, as in
//!   "if count<limit then") is given back as speech.
//! - A malformed tag that does look like one, an unknown tag, a closing tag other than a block's
//!   (`</diagram>`, `</detail>`), and any tag over [`NARRATION_TAG_MAX_BYTES`] are dropped and
//!   counted. The parser then skips to the tag's `>` (at most [`NARRATION_SKIP_MAX_BYTES`]) and
//!   speech continues.
//!
//! A block's tags (D-187) are the only ones that close: `<diagram …>` opens one and `</diagram>`
//! ends it. The parser only reports them (`self_closing`, [`Piece::Close`]); the stream reads the
//! block between them, so the parser itself never nests.

use crate::limits::{NARRATION_SKIP_MAX_BYTES, NARRATION_TAG_MAX_BYTES};
use crate::narration::tags::TAG_NAMES;
use crate::narration::tags::diagram::{BLOCK_TAG_NAMES, PART_KIND_TAG_NAMES};

/// A tag as written: its lowercase name, attributes in order, and the exact source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTag {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub written: String,
    /// Written `<… />`. A block's opening tag (`<diagram …>`) is not.
    pub self_closing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Tag(RawTag),
    /// A block's closing tag, `</diagram>`: its lowercase name.
    Close(String),
    /// A tag that was dropped; its text is the source that was discarded, for the log.
    Dropped(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    /// Saw `<`.
    Open,
    Name,
    /// After the name or a value: whitespace, an attribute, `/` or `>`.
    Between,
    AttributeName,
    AfterAttributeName,
    BeforeValue,
    Value(char),
    /// Saw `/` and expects `>`.
    SlashClose,
    /// Inside `</…>`, which is dropped.
    Closing,
    /// Discarding the rest of a dropped tag up to its `>`.
    Skip {
        quote: Option<char>,
        skipped_bytes: usize,
    },
}

/// XML entities the talker may write inside attribute values.
const ENTITIES: &[(&str, &str)] = &[
    ("&quot;", "\""),
    ("&apos;", "'"),
    ("&lt;", "<"),
    ("&gt;", ">"),
    ("&amp;", "&"),
];

#[derive(Clone, Debug)]
pub struct TagParser {
    state: State,
    raw: String,
    name: String,
    attributes: Vec<(String, String)>,
    attribute: String,
    value: String,
}

impl Default for TagParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TagParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Text,
            raw: String::new(),
            name: String::new(),
            attributes: Vec::new(),
            attribute: String::new(),
            value: String::new(),
        }
    }

    pub fn push(&mut self, text: &str, out: &mut Vec<Piece>) {
        for ch in text.chars() {
            // A give-back re-feeds the char once, in the text state, where it is always consumed.
            if let Some(again) = self.step(ch, out) {
                let consumed = self.step(again, out);
                debug_assert!(consumed.is_none(), "a re-fed char is consumed");
            }
        }
    }

    /// The stream ended: an unfinished tag is dropped, or given back when it was speech.
    pub fn finish(&mut self, out: &mut Vec<Piece>) {
        match self.state {
            State::Text | State::Skip { .. } => {}
            State::Open
            | State::Name
            | State::Between
            | State::AttributeName
            | State::AfterAttributeName
            | State::BeforeValue
            | State::Value(_)
            | State::SlashClose
            | State::Closing => {
                if self.looks_like_tag() || self.state == State::Closing {
                    out.push(Piece::Dropped(std::mem::take(&mut self.raw)));
                } else {
                    let raw = std::mem::take(&mut self.raw);
                    push_text(out, &raw);
                }
            }
        }
        self.state = State::Text;
    }

    /// Consumes one char; returns it when it must be fed again as speech.
    fn step(&mut self, ch: char, out: &mut Vec<Piece>) -> Option<char> {
        match self.state {
            State::Text => {
                if ch == '<' {
                    self.begin_tag();
                } else {
                    push_char(out, ch);
                }
                None
            }
            State::Skip {
                quote,
                skipped_bytes,
            } => {
                self.skip(ch, quote, skipped_bytes);
                None
            }
            State::Open
            | State::Name
            | State::Between
            | State::AttributeName
            | State::AfterAttributeName
            | State::BeforeValue
            | State::Value(_)
            | State::SlashClose
            | State::Closing => {
                self.raw.push(ch);
                if self.raw.len() > NARRATION_TAG_MAX_BYTES {
                    self.overflow(ch, out);
                    return None;
                }
                self.in_tag(ch, out)
            }
        }
    }

    fn begin_tag(&mut self) {
        self.raw.clear();
        self.raw.push('<');
        self.name.clear();
        self.attributes.clear();
        self.attribute.clear();
        self.value.clear();
        self.state = State::Open;
    }

    fn in_tag(&mut self, ch: char, out: &mut Vec<Piece>) -> Option<char> {
        let next = match self.state {
            State::Open if ch.is_ascii_alphabetic() => {
                self.name.push(ch);
                State::Name
            }
            State::Open if ch == '/' => State::Closing,
            State::Name if is_name_char(ch) => {
                self.name.push(ch);
                State::Name
            }
            State::Name | State::Between if ch.is_whitespace() => State::Between,
            State::Name | State::Between | State::SlashClose if ch == '>' => {
                let self_closing = self.state == State::SlashClose;
                self.complete(self_closing, out);
                State::Text
            }
            State::Name | State::Between if ch == '/' => State::SlashClose,
            State::Between if ch.is_ascii_alphabetic() || ch == '_' => {
                self.attribute.clear();
                self.attribute.push(ch);
                State::AttributeName
            }
            State::AttributeName if is_name_char(ch) => {
                self.attribute.push(ch);
                State::AttributeName
            }
            State::AttributeName | State::AfterAttributeName if ch == '=' => State::BeforeValue,
            State::AttributeName | State::AfterAttributeName | State::BeforeValue
                if ch.is_whitespace() =>
            {
                if self.state == State::AttributeName {
                    State::AfterAttributeName
                } else {
                    self.state
                }
            }
            State::BeforeValue if ch == '"' || ch == '\'' => {
                self.value.clear();
                State::Value(ch)
            }
            State::Value(quote) if ch == quote => {
                let name = self.attribute.to_ascii_lowercase();
                self.attributes.push((name, decode_entities(&self.value)));
                State::Between
            }
            State::Value(quote) => {
                self.value.push(ch);
                State::Value(quote)
            }
            State::Closing if ch == '>' => {
                self.close(out);
                State::Text
            }
            State::Closing => State::Closing,
            State::Open
            | State::Name
            | State::Between
            | State::AttributeName
            | State::AfterAttributeName
            | State::BeforeValue
            | State::SlashClose
            | State::Text
            | State::Skip { .. } => return self.malformed(ch, out),
        };
        self.state = next;
        None
    }

    /// `</…>` ended: a block's closing tag, or a closing tag to drop.
    fn close(&mut self, out: &mut Vec<Piece>) {
        let raw = std::mem::take(&mut self.raw);
        let name = raw
            .trim_start_matches("</")
            .trim_end_matches('>')
            .trim()
            .to_ascii_lowercase();
        out.push(if BLOCK_TAG_NAMES.contains(&name.as_str()) {
            Piece::Close(name)
        } else {
            Piece::Dropped(raw)
        });
    }

    fn looks_like_tag(&self) -> bool {
        !self.attributes.is_empty() || TAG_NAMES.contains(&self.name.to_ascii_lowercase().as_str())
    }

    /// A syntax error at `ch`, which is already in `raw`.
    fn malformed(&mut self, ch: char, out: &mut Vec<Piece>) -> Option<char> {
        if !self.looks_like_tag() {
            let mut raw = std::mem::take(&mut self.raw);
            raw.truncate(raw.len() - ch.len_utf8());
            push_text(out, &raw);
            self.state = State::Text;
            return Some(ch);
        }
        let mut raw = std::mem::take(&mut self.raw);
        raw.truncate(raw.len() - ch.len_utf8());
        out.push(Piece::Dropped(raw));
        self.state = State::Skip {
            quote: None,
            skipped_bytes: 0,
        };
        match ch {
            // A new tag starts right here; the broken one ends without swallowing it.
            '<' => self.begin_tag(),
            '>' => self.state = State::Text,
            _ => {}
        }
        None
    }

    fn overflow(&mut self, ch: char, out: &mut Vec<Piece>) {
        let quote = match self.state {
            State::Value(quote) if ch != quote => Some(quote),
            State::BeforeValue if ch == '"' || ch == '\'' => Some(ch),
            State::Text
            | State::Open
            | State::Name
            | State::Between
            | State::AttributeName
            | State::AfterAttributeName
            | State::BeforeValue
            | State::Value(_)
            | State::SlashClose
            | State::Closing
            | State::Skip { .. } => None,
        };
        out.push(Piece::Dropped(std::mem::take(&mut self.raw)));
        self.state = if quote.is_none() && ch == '>' {
            State::Text
        } else {
            State::Skip {
                quote,
                skipped_bytes: 0,
            }
        };
    }

    fn skip(&mut self, ch: char, quote: Option<char>, skipped_bytes: usize) {
        let skipped_bytes = skipped_bytes + ch.len_utf8();
        if skipped_bytes > NARRATION_SKIP_MAX_BYTES {
            self.state = State::Text;
            return;
        }
        let quote = match quote {
            Some(open) if ch == open => None,
            Some(open) => Some(open),
            None if ch == '>' => {
                self.state = State::Text;
                return;
            }
            None => None,
        };
        self.state = State::Skip {
            quote,
            skipped_bytes,
        };
    }

    fn complete(&mut self, self_closing: bool, out: &mut Vec<Piece>) {
        let name = self.name.to_ascii_lowercase();
        let written = std::mem::take(&mut self.raw);
        // A part's kind written as its tag's name is a block's part; outside a block it drops
        // when it is read, as an unknown tag does here.
        if TAG_NAMES.contains(&name.as_str()) || PART_KIND_TAG_NAMES.contains(&name.as_str()) {
            out.push(Piece::Tag(RawTag {
                name,
                attributes: std::mem::take(&mut self.attributes),
                written,
                self_closing,
            }));
        } else {
            out.push(Piece::Dropped(written));
        }
    }
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn push_char(out: &mut Vec<Piece>, ch: char) {
    if let Some(Piece::Text(text)) = out.last_mut() {
        text.push(ch);
    } else {
        out.push(Piece::Text(ch.to_string()));
    }
}

fn push_text(out: &mut Vec<Piece>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Piece::Text(existing)) = out.last_mut() {
        existing.push_str(text);
    } else {
        out.push(Piece::Text(text.to_owned()));
    }
}

fn decode_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        decoded.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some((entity, plain)) = ENTITIES.iter().find(|(entity, _)| rest.starts_with(entity))
        {
            decoded.push_str(plain);
            rest = &rest[entity.len()..];
        } else {
            decoded.push('&');
            rest = &rest[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::{Piece, TagParser};
    use crate::limits::NARRATION_TAG_MAX_BYTES;

    fn parse(input: &str) -> Vec<Piece> {
        let mut parser = TagParser::new();
        let mut out = Vec::new();
        parser.push(input, &mut out);
        parser.finish(&mut out);
        out
    }

    fn texts(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text(text) => Some(text.as_str()),
                Piece::Tag(_) | Piece::Dropped(_) | Piece::Close(_) => None,
            })
            .collect()
    }

    fn dropped(pieces: &[Piece]) -> usize {
        pieces
            .iter()
            .filter(|piece| matches!(piece, Piece::Dropped(_)))
            .count()
    }

    #[test]
    fn parses_tags_between_speech() {
        let pieces = parse(r#"Then the <node id="lb" label='Load &amp; balance'/>load balancer"#);
        assert_eq!(texts(&pieces), "Then the load balancer");
        let Some(Piece::Tag(tag)) = pieces.get(1) else {
            panic!("expected a tag: {pieces:?}");
        };
        assert_eq!(tag.name, "node");
        assert_eq!(
            tag.attributes,
            vec![
                ("id".to_owned(), "lb".to_owned()),
                ("label".to_owned(), "Load & balance".to_owned())
            ]
        );
    }

    #[test]
    fn speech_that_looks_like_a_tag_is_given_back() {
        for prose in ["a < b and c <= d", "if count<limit then stop", "x <3", "a<"] {
            let pieces = parse(prose);
            assert_eq!(texts(&pieces), prose, "{prose}");
            assert_eq!(dropped(&pieces), 0, "{prose}");
        }
    }

    #[test]
    fn bad_unknown_closing_and_oversized_tags_are_dropped_and_speech_continues() {
        let long = format!(
            r#"<node id="a" label="{}"/>"#,
            "x".repeat(NARRATION_TAG_MAX_BYTES)
        );
        for (input, expected_text) in [
            (r#"one <node id=lb/> two"#.to_owned(), "one  two"),
            (r#"one <blink a="b"/> two"#.to_owned(), "one  two"),
            ("one </node> two".to_owned(), "one  two"),
            (format!("one {long} two"), "one  two"),
            (
                r#"one <node id="a" <edge from="a" to="b"/>two"#.to_owned(),
                "one two",
            ),
        ] {
            let pieces = parse(&input);
            assert_eq!(texts(&pieces), expected_text, "{input}");
            assert!(dropped(&pieces) >= 1, "{input}");
        }
    }

    #[test]
    fn an_unterminated_tag_is_dropped_at_the_end() {
        let pieces = parse(r#"so <node id="a" label="Re"#);
        assert_eq!(texts(&pieces), "so ");
        assert_eq!(dropped(&pieces), 1);
    }
}
