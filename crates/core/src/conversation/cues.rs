//! Every lexicon the conversation uses, as tables (LLD §2.2). Matching works on normalized words:
//! lowercase, with surrounding punctuation stripped, so "Mm-hmm." and "mm-hmm" are the same.
//! Tests iterate the tables, so a new entry is covered the moment it is added.

use crate::limits::BARGE_IN_CUE_WORDS;

/// Last words that mean the speaker isn't done ("so the leader… and"): the end of turn waits for
/// the grace period instead of the usual silence.
pub const CONTINUATION_WORDS: &[&str] = &[
    "and", "so", "because", "but", "or", "then", "um", "uh", "like", "the", "a", "to", "of",
];

/// Transcript endings that mean the same: speech-to-text marks a trailing-off sentence with an
/// ellipsis, and a trailing comma is a clause in progress.
pub const CONTINUATION_ENDINGS: &[&str] = &["...", "…", ","];

/// Rule (a): any of these among the first [`BARGE_IN_CUE_WORDS`] words is an interruption.
pub const INTERRUPTION_CUES: &[&str] = &[
    "wait", "stop", "hold on", "hang on", "what", "why", "how", "no", "actually", "sorry", "pause",
    "slower", "repeat", "back", "skip",
];

/// A short utterance made only of these keeps BoardAI going (LLD §2.2). The LLD's list plus the
/// spellings speech-to-text produces for them.
pub const BACKCHANNELS: &[&str] = &[
    "mm-hm",
    "mm-hmm",
    "mhm",
    "mm",
    "mm hmm",
    "hmm",
    "uh-huh",
    "uh huh",
    "yeah",
    "yep",
    "yes",
    "right",
    "all right",
    "alright",
    "ok",
    "okay",
    "sure",
    "got it",
    "gotcha",
    "i see",
    "nice",
    "cool",
];

/// The change gate's yes (LLD §3.5). Only the user's next utterance can match it (I9).
pub const AFFIRMATIVES: &[&str] = &[
    "yes", "yeah", "yep", "go", "go ahead", "do it", "sure", "okay", "ok",
];

/// Words allowed around an affirmative without changing its meaning ("yes please, go ahead").
pub const AFFIRMATIVE_FILLERS: &[&str] = &["please", "then", "just"];

/// Abbreviations whose period does not end a sentence, so a clause is not cut after them.
pub const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "mr.", "mrs.", "dr.", "approx.",
];

/// Normalized words of an utterance, in order.
#[must_use]
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|ch: char| !ch.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Number of words, for barge-in rule (b).
#[must_use]
pub fn word_count(text: &str) -> usize {
    words(text).len()
}

/// The form used for cue matching: "what's" is "what".
fn cue_form(word: &str) -> &str {
    word.split(['\'', '’']).next().unwrap_or(word)
}

/// True when `phrase` (space-separated) matches `words` starting at `index`.
fn phrase_at(words: &[String], index: usize, phrase: &str, cue: bool) -> Option<usize> {
    let mut length = 0;
    for (offset, part) in phrase.split(' ').enumerate() {
        let word = words.get(index + offset)?;
        let word = if cue { cue_form(word) } else { word.as_str() };
        if word != part {
            return None;
        }
        length = offset + 1;
    }
    Some(length)
}

/// Rule (a): an interruption cue starts within the first few words.
#[must_use]
pub fn has_interruption_cue(text: &str) -> bool {
    let words = words(text);
    (0..BARGE_IN_CUE_WORDS.min(words.len())).any(|index| {
        INTERRUPTION_CUES
            .iter()
            .any(|cue| phrase_at(&words, index, cue, true).is_some())
    })
}

/// True when every word belongs to a phrase of `table` or to `fillers`, and at least one phrase
/// matched. Longer phrases are tried first so "got it" is not read as "got" + "it".
fn made_of(text: &str, table: &[&str], fillers: &[&str]) -> bool {
    let words = words(text);
    let mut index = 0;
    let mut phrases = 0;
    while index < words.len() {
        let longest = table
            .iter()
            .filter_map(|phrase| phrase_at(&words, index, phrase, false))
            .max();
        if let Some(length) = longest {
            index += length;
            phrases += 1;
        } else if fillers.contains(&words[index].as_str()) {
            index += 1;
        } else {
            return false;
        }
    }
    phrases > 0
}

/// A backchannel transcript: empty, or only backchannel phrases ("yeah, okay").
#[must_use]
pub fn is_backchannel(text: &str) -> bool {
    words(text).is_empty() || made_of(text, BACKCHANNELS, &[])
}

/// The change gate's test: the utterance is a yes and nothing else.
#[must_use]
pub fn is_affirmative(text: &str) -> bool {
    made_of(text, AFFIRMATIVES, AFFIRMATIVE_FILLERS)
}

/// The utterance sounds unfinished: it ends on a continuation word or a trailing-off mark.
#[must_use]
pub fn ends_unfinished(text: &str) -> bool {
    let trimmed = text.trim_end();
    if CONTINUATION_ENDINGS
        .iter()
        .any(|ending| trimmed.ends_with(ending))
    {
        return true;
    }
    words(trimmed)
        .last()
        .is_some_and(|last| CONTINUATION_WORDS.contains(&last.as_str()))
}

/// The word ending at `end` (exclusive) is an abbreviation, so its period is not a sentence end.
#[must_use]
pub fn is_abbreviation(word: &str) -> bool {
    let lower = word.to_lowercase();
    ABBREVIATIONS
        .iter()
        .any(|abbreviation| lower.ends_with(abbreviation))
}

#[cfg(test)]
mod tests {
    use super::{
        AFFIRMATIVES, BACKCHANNELS, CONTINUATION_ENDINGS, CONTINUATION_WORDS, INTERRUPTION_CUES,
        ends_unfinished, has_interruption_cue, is_affirmative, is_backchannel, word_count,
    };

    #[test]
    fn every_backchannel_is_recognized_with_transcription_noise() {
        for phrase in BACKCHANNELS {
            assert!(is_backchannel(phrase), "{phrase}");
            let noisy = format!("{}.", phrase.to_uppercase());
            assert!(is_backchannel(&noisy), "{noisy}");
            assert!(!has_interruption_cue(phrase), "{phrase} must not be a cue");
        }
        assert!(is_backchannel(""));
        assert!(is_backchannel("Yeah, okay."));
        assert!(!is_backchannel("yeah but why"));
    }

    #[test]
    fn every_cue_interrupts_in_the_first_words_only() {
        for cue in INTERRUPTION_CUES {
            assert!(has_interruption_cue(cue), "{cue}");
            assert!(has_interruption_cue(&format!("oh {cue}, hmm")), "{cue}");
            assert!(
                !has_interruption_cue(&format!("one two three {cue}")),
                "{cue} after the cue window"
            );
            assert!(!is_backchannel(cue), "{cue} must not be a backchannel");
        }
        assert!(has_interruption_cue("What's an inode?"));
        assert!(!has_interruption_cue("whatever"));
    }

    #[test]
    fn every_affirmative_confirms_and_other_utterances_do_not() {
        for phrase in AFFIRMATIVES {
            assert!(is_affirmative(phrase), "{phrase}");
            assert!(is_affirmative(&format!("{phrase}, please.")), "{phrase}");
        }
        assert!(is_affirmative("Yes, go ahead."));
        for other in ["", "no", "please", "yes but rename it", "go away", "wait"] {
            assert!(!is_affirmative(other), "{other:?}");
        }
    }

    #[test]
    fn every_continuation_word_and_ending_sounds_unfinished() {
        for word in CONTINUATION_WORDS {
            assert!(ends_unfinished(&format!("so the leader {word}")), "{word}");
            assert!(ends_unfinished(&format!("So the leader {word}.")), "{word}");
        }
        for ending in CONTINUATION_ENDINGS {
            assert!(
                ends_unfinished(&format!("so the leader{ending}")),
                "{ending}"
            );
        }
        assert!(!ends_unfinished("how does the leader get elected?"));
        assert!(!ends_unfinished(""));
    }

    #[test]
    fn counts_words_without_punctuation() {
        assert_eq!(word_count("wait, why Redis first?"), 4);
        assert_eq!(word_count(" -- "), 0);
    }
}
