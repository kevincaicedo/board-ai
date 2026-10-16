//! How well a narration follows the drawing rules the system prompt teaches (LLD §2.3). The
//! numbers serve two readers: `just converse` prints them per answer for spikes S3 and S5, and
//! the prompt's tests hold its worked examples to them, so the examples never teach a habit the
//! rules forbid.
//!
//! The checks are heuristics on the written text, never decisions: the core still speaks and
//! draws whatever parses. They look for the habits seen in live runs: a tag standing in for the
//! word it should precede ("Next is the <edge …/> from the client"), a long spoken preamble before
//! anything is drawn, many tags piled before one word, and Markdown read aloud.

use crate::narration::parser::{Piece, TagParser};

/// Tags that put strokes on the canvas (the ones that take a drawing slot).
const DRAWING_TAGS: &[&str] = &["node", "edge", "annotate", "strike", "trace"];

/// Words after which a noun must come: a tag right after one of these, followed by a word that
/// can't start a noun phrase, stood in for the noun.
const DETERMINERS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "our", "your", "its", "their", "each",
    "every",
];

/// Words that can't directly follow a determiner.
const NOT_A_NOUN: &[&str] = &[
    "from", "to", "and", "or", "but", "with", "in", "on", "at", "of", "for", "into", "onto",
    "then", "between", "through", "via", "which", "is", "are", "was", "were",
];

/// Written forms that are formatting, not speech: Markdown emphasis, code marks and arrows.
const MARKUP: &[&str] = &["**", "__", "`", "->", "=>", "→", "←"];

/// Line openers that make a Markdown list or heading.
const LINE_MARKUP: &[&str] = &["- ", "* ", "• ", "#"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NarrationQuality {
    /// Tags of any kind, well formed or not.
    pub tags: u32,
    pub drawing_tags: u32,
    /// Spoken words before the first drawing tag; every word when nothing is drawn.
    pub preamble_words: u32,
    /// Tags that stand in for a word, so the word is missing from speech.
    pub missing_words: u32,
    /// The most drawing tags with no spoken word between them (they all fire on one word).
    pub drawing_tags_per_word_max: u32,
    /// Markdown or arrows in the speech, which text-to-speech would read or stumble on.
    pub markup: u32,
}

/// Where the scan is between two words.
#[derive(Default)]
struct Scan {
    quality: NarrationQuality,
    /// The last spoken word, lowercased, without punctuation.
    last_word: Option<String>,
    /// Tags since the last spoken word, and whether one of them follows a determiner.
    pending_tags: u32,
    pending_drawing: u32,
    after_determiner: bool,
    words: u32,
    drew: bool,
    line_start: bool,
}

/// Scores one response as the talker wrote it, tags included.
#[must_use]
pub fn assess(written: &str) -> NarrationQuality {
    let mut parser = TagParser::new();
    let mut pieces = Vec::new();
    parser.push(written, &mut pieces);
    parser.finish(&mut pieces);
    let mut scan = Scan {
        line_start: true,
        ..Scan::default()
    };
    for piece in &pieces {
        match piece {
            Piece::Text(text) => scan.text(text),
            Piece::Tag(tag) => scan.tag(&tag.name),
            Piece::Dropped(raw) => scan.tag(dropped_name(raw)),
        }
    }
    scan.end();
    if !scan.drew {
        scan.quality.preamble_words = scan.words;
    }
    scan.quality
}

impl Scan {
    fn tag(&mut self, name: &str) {
        self.quality.tags += 1;
        if self.pending_tags == 0 {
            self.after_determiner = self
                .last_word
                .as_deref()
                .is_some_and(|word| DETERMINERS.contains(&word));
        }
        self.pending_tags += 1;
        if DRAWING_TAGS.contains(&name) {
            self.quality.drawing_tags += 1;
            self.pending_drawing += 1;
            if !self.drew {
                self.drew = true;
                self.quality.preamble_words = self.words;
            }
        }
    }

    fn text(&mut self, text: &str) {
        self.quality.markup += u32::try_from(
            MARKUP
                .iter()
                .map(|mark| text.matches(mark).count())
                .sum::<usize>(),
        )
        .unwrap_or(u32::MAX);
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                self.line_start = true;
            }
            let trimmed = line.trim_start();
            if self.line_start && LINE_MARKUP.iter().any(|mark| trimmed.starts_with(mark)) {
                self.quality.markup += 1;
            }
            if !trimmed.is_empty() {
                self.line_start = false;
            }
            self.emphasis(line);
            for token in line.split_whitespace() {
                self.token(token);
            }
        }
    }

    /// `*word*` and `_word_` emphasis (a lone `*` in prose is rare enough to count).
    fn emphasis(&mut self, line: &str) {
        for token in line.split_whitespace() {
            let core = token.trim_matches(|ch: char| ch.is_ascii_punctuation() && ch != '*');
            let starred = core.len() > 2 && core.starts_with('*') && core.ends_with('*');
            let underscored = token.len() > 2
                && token.starts_with('_')
                && token
                    .trim_end_matches(|ch: char| ".,;:!?".contains(ch))
                    .ends_with('_');
            if (starred && !core.starts_with("**")) || underscored {
                self.quality.markup += 1;
            }
        }
    }

    fn token(&mut self, token: &str) {
        let word: String = token
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase();
        let punctuation_first = token.chars().next().is_some_and(|ch| ".,;:!?".contains(ch));
        if self.pending_tags > 0 {
            let missing =
                punctuation_first || (self.after_determiner && NOT_A_NOUN.contains(&word.as_str()));
            if missing {
                self.quality.missing_words += 1;
            }
            self.flush_pending();
        }
        if word.is_empty() {
            return;
        }
        self.words += 1;
        self.last_word = Some(word);
    }

    fn flush_pending(&mut self) {
        self.quality.drawing_tags_per_word_max = self
            .quality
            .drawing_tags_per_word_max
            .max(self.pending_drawing);
        self.pending_tags = 0;
        self.pending_drawing = 0;
        self.after_determiner = false;
    }

    /// Tags at the very end anchor on the end of speech; a determiner right before them is
    /// still a missing noun ("then add a <node …/>").
    fn end(&mut self) {
        if self.pending_tags > 0 && self.after_determiner {
            self.quality.missing_words += 1;
        }
        self.flush_pending();
    }
}

/// The name of a dropped tag (`<node id=…` → `node`), so a malformed drawing tag still counts.
fn dropped_name(raw: &str) -> &str {
    let name = raw.trim_start_matches('<').trim_start_matches('/');
    let end = name
        .find(|ch: char| !ch.is_ascii_alphanumeric())
        .unwrap_or(name.len());
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::{NarrationQuality, assess};

    #[test]
    fn a_tag_before_its_word_is_clean() {
        let quality = assess(
            r#"It starts in the <node id="client" label="Browser" kind="actor"/>browser, then hits the <node id="cdn" label="CDN"/><edge from="client" to="cdn"/>CDN."#,
        );
        assert_eq!(
            quality,
            NarrationQuality {
                tags: 3,
                drawing_tags: 3,
                preamble_words: 4,
                missing_words: 0,
                drawing_tags_per_word_max: 2,
                markup: 0,
            }
        );
    }

    /// The habits seen live with gemma4 (S3): a tag used as the noun, before a preposition or
    /// punctuation, and at the very end after a determiner.
    #[test]
    fn a_tag_that_replaces_its_word_is_counted() {
        for (text, missing) in [
            (
                r#"Next is the <edge from="client" to="cdn"/> from the client."#,
                1,
            ),
            (r#"I'll add a <node id="lb"/>."#, 1),
            (
                r#"We connect a <edge from="api" to="redis"/> to the cache."#,
                1,
            ),
            (r#"Then we draw the <node id="db"/>"#, 1),
            (r#"Then the <node id="db"/>database."#, 0),
            (r#"<node id="db"/>Postgres stores it."#, 0),
        ] {
            assert_eq!(assess(text).missing_words, missing, "{text}");
        }
    }

    #[test]
    fn preamble_bursts_and_markup_are_measured() {
        let quality = assess(
            "Sure. We can trace that **request** flow -> step by step.\n- First the client.\n\
             <node id=\"a\"/><node id=\"b\"/><node id=\"c\"/>Here *it* is.",
        );
        assert_eq!(quality.preamble_words, 13);
        assert_eq!(quality.drawing_tags_per_word_max, 3);
        assert_eq!(quality.markup, 5, "both **, ->, the bullet and *it*");
        assert_eq!(assess("Nothing drawn at all.").preamble_words, 4);
    }

    #[test]
    fn malformed_drawing_tags_still_count() {
        let quality = assess(r#"See the <node id=lb/>load balancer."#);
        assert_eq!((quality.tags, quality.drawing_tags), (1, 1));
    }
}
