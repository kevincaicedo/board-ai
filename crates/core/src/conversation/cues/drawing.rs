//! The lexicons of drawing together (artboard 13): asking to draw, and what you say while you
//! draw. Each is a table the tests iterate, matched on the same normalized words as the other
//! cues.
//!
//! While you draw, a finished utterance is one of: a control ("done", "never mind", "undo
//! that"), a question (which finishes the drawing and is asked), a label for your latest shape
//! ("…and this one is the cache" → "cache", or just "cache"), or talk about the drawing, which
//! the talker gets when you finish.

use super::{is_backchannel, made_of, phrase_at, words};
use crate::limits::{USER_BARE_LABEL_WORDS_MAX, USER_LABEL_WORDS_MAX};

/// Asking to draw, as a whole utterance ("Let me draw what I mean.").
pub const START_PHRASES: &[&str] = &[
    "let me draw",
    "let me draw it",
    "let me draw this",
    "let me draw that",
    "let me draw something",
    "let me draw what i mean",
    "let me draw it out",
    "let me draw it for you",
    "let me draw on the board",
    "let me just draw it",
    "let me sketch it",
    "let me sketch this",
    "let me sketch it out",
    "let me show you",
    "let me show you what i mean",
    "i'll draw",
    "i'll draw it",
    "i'll draw this",
    "i'll draw that",
    "i'll draw it out",
    "i'll sketch it",
    "i'll show you",
    "can i draw",
    "can i draw it",
    "can i draw something",
    "i want to draw",
    "i want to draw something",
    "i'd like to draw",
    "i'd like to draw something",
];

/// Words allowed around a drawing control, and skipped before a label ("wait, let me draw it",
/// "okay, and this is the cache").
pub const DRAW_FILLERS: &[&str] = &[
    "okay", "ok", "actually", "wait", "hold", "hang", "on", "so", "here", "just", "um", "uh",
    "hmm", "hey", "no", "oh", "please", "now", "then", "well", "yeah", "and", "like", "alright",
    "right", "let's", "see",
];

/// Finishing the drawing, as a whole utterance or at its end ("…the cache, done").
pub const DONE_PHRASES: &[&str] = &[
    "done",
    "i'm done",
    "i am done",
    "all done",
    "i'm all done",
    "done drawing",
    "i'm done drawing",
    "finished",
    "i'm finished",
    "finished drawing",
    "that's it",
    "that's all",
    "that's everything",
    "there you go",
    "here you go",
    "there it is",
    "go ahead",
    "your turn",
    "over to you",
];

/// Leaving drawing without asking anything, as a whole utterance; your shapes stay.
pub const CANCEL_PHRASES: &[&str] = &[
    "never mind",
    "nevermind",
    "forget it",
    "forget the drawing",
    "cancel",
    "cancel that",
    "cancel drawing",
    "cancel the drawing",
    "stop drawing",
    "scrap the drawing",
];

/// Undoing your last shape, as a whole utterance.
pub const UNDO_PHRASES: &[&str] = &[
    "undo",
    "undo that",
    "undo it",
    "undo the last one",
    "scratch that",
    "erase that",
    "erase it",
    "delete that",
    "delete it",
    "remove that",
    "remove it",
    "take that back",
    "take that out",
    "not that one",
    "oops",
];

/// What comes before a label ("and this one is the cache"); the longest that matches is used.
pub const LABEL_LEAD_INS: &[&str] = &[
    "this is",
    "this one is",
    "this one's",
    "that is",
    "that's",
    "that one is",
    "that one's",
    "here is",
    "here's",
    "this is called",
    "this one is called",
    "that's called",
    "it's called",
    "this would be",
    "this will be",
    "that would be",
    "this box is",
    "this arrow is",
    "this shape is",
    "it's",
    "it is",
    "call it",
    "call this",
    "call that",
    "label it",
    "label this",
    "name it",
    "these are",
    "those are",
    "this means",
    "this represents",
    "that represents",
];

/// Words skipped between a lead-in and its label ("this is the cache" labels "cache").
pub const LABEL_ARTICLES: &[&str] = &["the", "a", "an", "our", "my", "your", "its", "their"];

/// First words that make the rest talk, not a label ("this is where requests go").
pub const NOT_LABEL_STARTS: &[&str] = &[
    "where",
    "when",
    "how",
    "why",
    "what",
    "which",
    "who",
    "because",
    "so",
    "if",
    "going",
    "gonna",
    "just",
    "basically",
    "kind",
    "sort",
    "not",
];

/// First words of a question ("does this make sense?"), which finishes the drawing.
pub const QUESTION_STARTS: &[&str] = &[
    "what", "why", "how", "does", "do", "did", "is", "are", "can", "could", "should", "would",
    "will", "where", "which", "who", "when", "isn't", "doesn't", "am",
];

/// What a finished utterance means while you draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard<'a> {
    /// Nothing to keep: a backchannel ("okay"), or asking to draw again.
    Nothing,
    /// Leave drawing without asking anything.
    Cancel,
    /// Undo your last shape.
    Undo,
    /// Finish; `before` is what came before the done phrase ("…the cache" of "…the cache,
    /// done"), as written.
    Done { before: &'a str },
    /// A question finishes the drawing and is asked.
    Question,
    /// A label for your latest shape, as written.
    Label(String),
    /// Talk about the drawing, kept for the talker.
    Talk,
}

/// "Let me draw", as a whole utterance.
#[must_use]
pub fn is_start(text: &str) -> bool {
    made_of(text, START_PHRASES, DRAW_FILLERS)
}

/// What a finished utterance means while you draw.
#[must_use]
pub fn heard(text: &str) -> Heard<'_> {
    if words(text).is_empty() || is_backchannel(text) || is_start(text) {
        return Heard::Nothing;
    }
    if made_of(text, CANCEL_PHRASES, DRAW_FILLERS) {
        return Heard::Cancel;
    }
    if made_of(text, UNDO_PHRASES, DRAW_FILLERS) {
        return Heard::Undo;
    }
    if made_of(text, DONE_PHRASES, DRAW_FILLERS) {
        return Heard::Done { before: "" };
    }
    if let Some(before) = before_done(text) {
        return Heard::Done { before };
    }
    // "…and here is the cache" is a label, though "is" can open a question.
    if asks(text) || (lead_in(&words(text)).is_none() && opens_a_question(text)) {
        return Heard::Question;
    }
    label_or_talk(text)
}

/// A label, or talk: what words that are neither a control nor a question mean.
#[must_use]
pub fn label_or_talk(text: &str) -> Heard<'_> {
    let spans = word_spans(text);
    let normalized = words(text);
    let (begin, words_max) = match lead_in(&normalized) {
        Some(end) => (skip(&normalized, end, LABEL_ARTICLES), USER_LABEL_WORDS_MAX),
        None => (
            skip(
                &normalized,
                skip(&normalized, 0, DRAW_FILLERS),
                LABEL_ARTICLES,
            ),
            USER_BARE_LABEL_WORDS_MAX,
        ),
    };
    let count = normalized.len().saturating_sub(begin);
    let talk = normalized
        .get(begin)
        .is_some_and(|first| NOT_LABEL_STARTS.contains(&first.as_str()));
    let Some(&(from, _)) = spans.get(begin) else {
        return Heard::Talk;
    };
    if count == 0 || count > words_max || talk {
        return Heard::Talk;
    }
    let label = text[from..].trim_end_matches(|ch: char| !ch.is_alphanumeric() && ch != ')');
    Heard::Label(label.to_owned())
}

/// Whether the words heard so far could still become a label, for the hint under the latest
/// shape while you speak (artboard 13's "→ will label this shape").
#[must_use]
pub fn could_label(partial: &str) -> bool {
    let normalized = words(partial);
    let start = skip(&normalized, 0, DRAW_FILLERS);
    if start == normalized.len() {
        return false;
    }
    let rest = &normalized[start..];
    // A lead-in still being said ("and this one…").
    let leading = LABEL_LEAD_INS.iter().any(|lead_in| {
        let parts: Vec<&str> = lead_in.split(' ').collect();
        parts.len() > rest.len() && rest.iter().zip(&parts).all(|(word, part)| word == part)
    });
    if leading {
        return true;
    }
    match lead_in(&normalized) {
        // "…this one is the" is a label on its way.
        Some(end) => {
            let begin = skip(&normalized, end, LABEL_ARTICLES);
            normalized.len().saturating_sub(begin) <= USER_LABEL_WORDS_MAX
        }
        None => matches!(heard(partial), Heard::Label(_)),
    }
}

/// Where the longest label lead-in ends, if one opens the words: at the start, or after fillers
/// ("and here is…": "here" can be a filler or a lead-in's first word).
fn lead_in(normalized: &[String]) -> Option<usize> {
    let fillers_end = skip(normalized, 0, DRAW_FILLERS);
    (0..=fillers_end).find_map(|at| {
        LABEL_LEAD_INS
            .iter()
            .filter_map(|lead_in| phrase_at(normalized, at, lead_in, false))
            .max()
            .map(|length| at + length)
    })
}

/// It ends with a question mark.
fn asks(text: &str) -> bool {
    let closed = text.trim_end().trim_end_matches(['"', '\'', ')', '”', '’']);
    closed.ends_with('?')
}

/// It opens with a question word ("does this make sense").
fn opens_a_question(text: &str) -> bool {
    let normalized = words(text);
    let start = skip(&normalized, 0, DRAW_FILLERS);
    normalized
        .get(start)
        .is_some_and(|first| QUESTION_STARTS.contains(&first.as_str()))
}

/// What comes before a done phrase that ends the utterance ("…the cache, and done" → "…the
/// cache"), as written; `None` when it doesn't end with one or nothing comes before it.
fn before_done(text: &str) -> Option<&str> {
    let normalized = words(text);
    let spans = word_spans(text);
    let length = DONE_PHRASES
        .iter()
        .filter_map(|phrase| {
            let count = phrase.split(' ').count();
            let at = normalized.len().checked_sub(count)?;
            phrase_at(&normalized, at, phrase, false)
        })
        .max()?;
    // Fillers just before it ("and", "okay") belong to the done.
    let mut end = normalized.len() - length;
    while end > 0 && DRAW_FILLERS.contains(&normalized[end - 1].as_str()) {
        end -= 1;
    }
    let &(cut, _) = spans.get(end)?;
    let before = text[..cut].trim_end_matches(|ch: char| !ch.is_alphanumeric());
    (end > 0 && !before.is_empty()).then_some(before)
}

/// The first index at or after `from` whose word is not in `table`.
fn skip(normalized: &[String], from: usize, table: &[&str]) -> usize {
    let mut index = from;
    while normalized
        .get(index)
        .is_some_and(|word| table.contains(&word.as_str()))
    {
        index += 1;
    }
    index
}

/// Byte ranges `[begin, end)` of the written words that [`words`] keeps, in the same order, so a
/// match on normalized words cuts the text as it was written.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for token in text.split_inclusive(char::is_whitespace) {
        let word = token.trim_end();
        let begin = offset
            + (word.len()
                - word
                    .trim_start_matches(|ch: char| !ch.is_alphanumeric())
                    .len());
        if word.chars().any(char::is_alphanumeric) {
            spans.push((begin, offset + word.len()));
        }
        offset += token.len();
    }
    spans
}

#[cfg(test)]
mod tests;
