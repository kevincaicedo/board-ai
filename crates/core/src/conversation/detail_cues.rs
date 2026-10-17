//! Drilling down by voice (D-188). "More detail on the arena" and "zoom into the arena" open a
//! detail layer anchored to the part they name; "back out" returns. Like the page cues
//! (D-177), "back out" said alone is answered by the core, and leading or ending a request
//! ("back out and explain the pool") it acts before the talker hears the rest. A drill-down
//! always reaches the talker, which fills the detail layer.
//!
//! The words after a drill-down's lead name the part (at most [`DETAIL_TARGET_WORDS_MAX`]); the
//! engine matches them against what is on the board. "Zoom into" alone, or naming nothing,
//! is a question for the talker.

use super::cues::words;
use super::stage_cues::{
    PageCue, STAGE_FILLERS, Token, ends_request, leads_request, made_of_stage, tokens,
};
use crate::limits::DETAIL_TARGET_WORDS_MAX;

/// What leads a drill-down. The words after it name the part.
pub const DETAIL_LEADS: &[&str] = &[
    "more detail on",
    "more details on",
    "more detail about",
    "more details about",
    "more detail into",
    "a bit more detail on",
    "a little more detail on",
    "more depth on",
    "go deeper into",
    "go deeper on",
    "go deeper in",
    "dig deeper into",
    "dig into",
    "deep dive into",
    "dive into",
    "zoom into",
    "zoom in on",
    "zoom in to",
    "drill into",
    "drill down into",
    "drill down on",
    "drill in on",
    "open up",
];

/// Going back up one level of detail (D-188).
pub const BACK_OUT_PHRASES: &[&str] = &[
    "back out",
    "zoom out",
    "zoom back out",
    "go back up",
    "go up a level",
    "back up a level",
    "up a level",
    "one level up",
    "up one level",
    "step back out",
    "come back out",
    "pull back",
    "leave the detail",
    "close the detail",
    "close this detail",
    "out of the detail",
];

/// Words before a drill-down's lead that don't change it ("can you give me more detail on…").
const DETAIL_FILLERS: &[&str] = &[
    "can", "could", "would", "you", "please", "give", "me", "show", "i", "want", "need", "like",
    "to", "let's", "lets", "we", "ok", "okay", "so", "now", "and", "maybe", "some", "get",
];

/// Words that end the part's name: a joiner, then the request.
const TARGET_ENDS: &[&str] = &[
    "and", "then", "please", "so", "because", "but", "with", "for", "while", "since",
];

/// Articles and pointers before the part's name, which name nothing.
const ARTICLES: &[&str] = &[
    "the", "a", "an", "this", "that", "its", "their", "our", "your",
];

/// A drill-down: the words naming the part, lowercased, and whether a request follows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailCue {
    pub target: String,
    pub with_request: bool,
}

/// "More detail on the arena", "zoom into the slot pool and explain leasing".
#[must_use]
pub fn detail_cue(text: &str) -> Option<DetailCue> {
    let tokens = tokens(text);
    let start = tokens
        .iter()
        .position(|(word, _)| !DETAIL_FILLERS.contains(&word.as_str()))?;
    let lead = lead_at(&tokens, start)?;
    let mut at = start + lead;
    while tokens
        .get(at)
        .is_some_and(|(word, _)| ARTICLES.contains(&word.as_str()))
    {
        at += 1;
    }
    let mut target = Vec::new();
    for (word, marked) in &tokens[at..] {
        if TARGET_ENDS.contains(&word.as_str()) {
            break;
        }
        target.push(word.as_str());
        if *marked {
            break;
        }
    }
    if target.is_empty() || target.len() > DETAIL_TARGET_WORDS_MAX {
        return None;
    }
    let rest = tokens[at + target.len()..]
        .iter()
        .filter(|(word, _)| !filler(word))
        .count();
    Some(DetailCue {
        target: target.join(" "),
        with_request: rest > 0,
    })
}

/// The longest drill-down lead at `index`, as a word count.
fn lead_at(tokens: &[Token], index: usize) -> Option<usize> {
    DETAIL_LEADS
        .iter()
        .filter(|lead| {
            lead.split(' ').enumerate().all(|(offset, part)| {
                tokens
                    .get(index + offset)
                    .is_some_and(|(word, _)| word == part)
            })
        })
        .map(|lead| lead.split(' ').count())
        .max()
}

fn filler(word: &str) -> bool {
    TARGET_ENDS.contains(&word) || STAGE_FILLERS.contains(&word) || DETAIL_FILLERS.contains(&word)
}

/// "Back out", alone or joined to a request at its start or end.
#[must_use]
pub fn back_out_cue(text: &str) -> Option<PageCue> {
    if made_of_stage(text, BACK_OUT_PHRASES) {
        return Some(PageCue::Only);
    }
    let tokens = tokens(text);
    (leads_request(&tokens, BACK_OUT_PHRASES) || ends_request(&tokens, BACK_OUT_PHRASES))
        .then_some(PageCue::WithRequest)
}

/// "Back out" and nothing else, for a part of joined controls.
#[must_use]
pub fn is_back_out(text: &str) -> bool {
    made_of_stage(text, BACK_OUT_PHRASES)
}

/// A drill-down naming its part and nothing else, for a part of joined controls.
#[must_use]
pub fn is_detail(text: &str) -> bool {
    detail_cue(text).is_some_and(|cue| !cue.with_request)
}

/// Words of a label or an id, lowercased, without articles: how a spoken name is compared.
#[must_use]
pub fn name_words(text: &str) -> Vec<String> {
    words(&text.replace(['_', '-', '.', '·', '(', ')'], " "))
        .into_iter()
        .filter(|word| !ARTICLES.contains(&word.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BACK_OUT_PHRASES, DETAIL_LEADS, DetailCue, back_out_cue, detail_cue, is_back_out,
        is_detail, name_words,
    };
    use crate::conversation::stage_cues::PageCue;

    fn cue(target: &str, with_request: bool) -> Option<DetailCue> {
        Some(DetailCue {
            target: target.to_owned(),
            with_request,
        })
    }

    #[test]
    fn every_lead_drills_into_the_part_it_names() {
        for lead in DETAIL_LEADS {
            assert_eq!(
                detail_cue(&format!("{lead} the arena")),
                cue("arena", false),
                "{lead}"
            );
            let asked = format!("Okay, can you {}, the slot pool?", lead.to_uppercase());
            assert_eq!(detail_cue(&asked), cue("slot pool", false), "{asked}");
            let more = format!("{lead} the arena and explain the free list");
            assert_eq!(detail_cue(&more), cue("arena", true), "{more}");
            assert!(is_detail(&format!("{lead} the arena")), "{lead}");
        }
    }

    #[test]
    fn the_owners_words_drill_down_and_questions_do_not() {
        assert_eq!(detail_cue("More detail on the arena."), cue("arena", false));
        assert_eq!(
            detail_cue("Zoom into the aligned pool, please."),
            cue("aligned pool", false)
        );
        for asked in [
            "",
            "zoom into",
            "more detail",
            "why is there more detail on the arena?",
            "more detail on how the arena hands out chunks of every size class it keeps",
            "what does the arena do",
        ] {
            assert_eq!(detail_cue(asked), None, "{asked:?}");
        }
    }

    #[test]
    fn back_out_alone_or_joined_to_a_request() {
        for phrase in BACK_OUT_PHRASES {
            assert_eq!(back_out_cue(phrase), Some(PageCue::Only), "{phrase}");
            assert!(is_back_out(&format!("Okay, {phrase}, please.")), "{phrase}");
            let joined = format!("{phrase} and explain the pool");
            assert_eq!(
                back_out_cue(&joined),
                Some(PageCue::WithRequest),
                "{joined}"
            );
        }
        for asked in [
            "",
            "why back out now?",
            "how do we zoom out the camera in CSS",
        ] {
            assert_eq!(back_out_cue(asked), None, "{asked:?}");
        }
    }

    #[test]
    fn names_compare_by_their_words() {
        assert_eq!(
            name_words("The SlotState · Buffer pool"),
            ["slotstate", "buffer", "pool"]
        );
        assert_eq!(name_words("arena_s"), ["arena", "s"]);
    }
}
