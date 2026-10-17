//! Words that ask BoardAI to draw (D-178): "draw", "show me", "diagram", "sketch". When the user
//! asked for a drawing and the reply drew nothing, the core asks the talker once more
//! (`engine::nudge`). A table its tests iterate, like every lexicon in `cues`.

use super::cues::words;

/// Asking for a drawing. Single words match whole words only ("drawback" is not "draw"); "show"
/// counts only with whom it is for ("show me", "show us"), so "the show" never does.
pub const DRAW_PHRASES: &[&str] = &[
    "draw",
    "redraw",
    "drawing",
    "sketch",
    "diagram",
    "diagrams",
    "illustrate",
    "visualize",
    "visualise",
    "chart",
    "whiteboard",
    "map out",
    "show me",
    "show us",
];

/// Words that turn a drawing request around when they come right before it ("don't draw it",
/// "without a diagram").
const NEGATIONS: &[&str] = &["don't", "dont", "not", "no", "without", "never", "stop"];

/// Words that make the drawing the user's own ("can I sketch it", and "let me draw"): then the
/// user draws (You draw), and BoardAI isn't asked to.
const SELF: &[&str] = &["i", "i'll", "i'm", "i'd", "let's", "lets"];

/// How many words before a drawing phrase a negation or the user's own drawing still reaches
/// ("don't bother to draw", "I want to sketch").
const NEGATION_REACH_WORDS: usize = 3;

/// Asking for a detailed map of many parts (D-187): the talker is reminded to stage it as one
/// diagram rather than sketch it tag by tag. Live, gpt-oss-120b sketched "draw the whole map in
/// detail" tag by tag with the prompt alone.
pub const MAP_PHRASES: &[&str] = &[
    "map",
    "detailed",
    "in detail",
    "whole system",
    "entire system",
    "whole thing",
    "architecture",
    "big picture",
    "every part",
    "all the parts",
    "all its parts",
];

/// Words before "map" that make it a data structure, not a drawing ("hash map").
const NOT_A_MAP: &[&str] = &[
    "hash",
    "tree",
    "bit",
    "sorted",
    "ordered",
    "concurrent",
    "heap",
];

/// The user's words ask for a detailed map, and not only a sketch.
#[must_use]
pub fn asks_for_map(text: &str) -> bool {
    let words = words(text);
    found_unnegated(&words, MAP_PHRASES, |index| {
        words[index] != "map"
            || index
                .checked_sub(1)
                .is_none_or(|before| !NOT_A_MAP.contains(&words[before].as_str()))
    })
}

/// The user's words ask for a drawing.
#[must_use]
pub fn asks_for_drawing(text: &str) -> bool {
    let words = words(text);
    found_unnegated(&words, DRAW_PHRASES, |_| true)
}

/// A phrase of `table` at a word `keep` accepts, with no negation or "let me" just before it.
fn found_unnegated(words: &[String], table: &[&str], keep: impl Fn(usize) -> bool) -> bool {
    (0..words.len()).any(|index| {
        let found = table.iter().any(|phrase| {
            phrase
                .split(' ')
                .enumerate()
                .all(|(offset, part)| words.get(index + offset).is_some_and(|word| word == part))
        });
        let before = &words[index.saturating_sub(NEGATION_REACH_WORDS)..index];
        let negated = before
            .iter()
            .any(|word| NEGATIONS.contains(&word.as_str()) || SELF.contains(&word.as_str()))
            || before
                .windows(2)
                .any(|pair| pair[0] == "let" && pair[1] == "me");
        found && !negated && keep(index)
    })
}

#[cfg(test)]
mod tests {
    use super::{DRAW_PHRASES, MAP_PHRASES, asks_for_drawing, asks_for_map};

    #[test]
    fn a_detailed_map_is_asked_for_and_a_hash_map_is_not() {
        for phrase in MAP_PHRASES {
            assert!(
                asks_for_map(&format!("Show me the {phrase} please")),
                "{phrase}"
            );
        }
        assert!(asks_for_map("Draw the whole map in detail."));
        assert!(asks_for_map(
            "Walk me through the architecture of inf-alloc"
        ));
        for other in [
            "How does a hash map grow?",
            "Walk me through a request",
            "Sketch the cache",
            "don't make it detailed",
        ] {
            assert!(!asks_for_map(other), "{other:?}");
        }
    }

    #[test]
    fn every_phrase_asks_and_a_negation_takes_it_back() {
        for phrase in DRAW_PHRASES {
            assert!(asks_for_drawing(phrase), "{phrase}");
            let asked = format!("Can you {phrase} the cache for me?");
            assert!(asks_for_drawing(&asked), "{asked}");
            let declined = format!("No need, don't {phrase} anything");
            assert!(!asks_for_drawing(&declined), "{declined}");
        }
    }

    /// The owner's words and the measured prompts, then requests that ask for no drawing.
    #[test]
    fn requests_for_a_drawing_and_others() {
        for asked in [
            "Draw a binary search tree as I insert 5, 3, 8, 1",
            "Show me how a hash map handles collisions",
            "Can you sketch the request path?",
            "Give me a diagram of Raft",
        ] {
            assert!(asks_for_drawing(asked), "{asked}");
        }
        for other in [
            "",
            "Explain how a request reaches the database",
            "Walk me through this repo",
            "what's the drawback of Raft?",
            "how is the show going",
            "explain it without drawing",
            "let me draw it",
            "can I sketch the cache?",
            "I want to draw the flow",
        ] {
            assert!(!asks_for_drawing(other), "{other:?}");
        }
    }
}
