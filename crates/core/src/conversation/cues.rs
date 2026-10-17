//! Every lexicon the conversation uses, as tables (LLD §2.2). Matching works on normalized words:
//! lowercase, with surrounding punctuation stripped, so "Mm-hmm." and "mm-hmm" are the same.
//! Tests iterate the tables, so a new entry is covered the moment it is added.

pub mod drawing;

use crate::limits::BARGE_IN_CUE_WORDS;

/// Last words that mean the speaker isn't done ("so the leader… and"): the end of turn waits for
/// the grace period instead of the usual silence. Whisper closes a sentence cut by a pause with
/// a period or a question mark ("I think the problem is.", "What happens when?", `spike_turns`),
/// so words that rarely end one count whatever the punctuation.
pub const CONTINUATION_WORDS: &[&str] = &[
    "and", "so", "because", "but", "or", "then", "um", "uh", "like", "the", "a", "an", "to", "of",
    "is", "are", "was", "were", "when", "if", "with", "for", "my", "your", "our", "their", "into",
    "than", "which", "whether",
];

/// Transcript endings that mean the same: speech-to-text marks a trailing-off sentence with an
/// ellipsis, and a trailing comma is a clause in progress.
pub const CONTINUATION_ENDINGS: &[&str] = &["...", "…", ","];

/// Rule (a): any of these among the first [`BARGE_IN_CUE_WORDS`] words is an interruption. The
/// LLD's cues, then what Whisper's earliest partials (256 ms after the onset) make of them:
/// "way" and "we" for "wait", "hold" and "hang" before their "on", "actual" and "action" for
/// "actually" (`spike_turns`: 37 of 50 cue clips heard at 256 ms instead of 29, and none of 450
/// backchannel clips misread, D-159). "hold up" is a cue in its own right. After an early
/// partial the final decides nothing these add: a short remark that isn't a backchannel
/// interrupts anyway (rule e).
pub const INTERRUPTION_CUES: &[&str] = &[
    "wait", "stop", "hold on", "hang on", "what", "why", "how", "no", "actually", "sorry", "pause",
    "slower", "repeat", "back", "skip", "hold up", "hold", "hang", "way", "we", "actual", "action",
];

/// A short utterance made only of these keeps BoardAI going (LLD §2.2). The LLD's list plus the
/// spellings speech-to-text produces for them: Whisper `base.en` wrote "MMHM", "MHMM", "Mm-hum.",
/// "Hum", "Hmmm.", "Aha!" and "Yup" for `say`'s backchannels (`spike_turns`).
pub const BACKCHANNELS: &[&str] = &[
    "mm-hm",
    "mm-hmm",
    "mhm",
    "mmhm",
    "mmhmm",
    "mhmm",
    "mm-hum",
    "mm",
    "mm hmm",
    "hmm",
    "hmmm",
    "hum",
    "uh-huh",
    "uh huh",
    "aha",
    "yeah",
    "yep",
    "yup",
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

/// Which running coding-harness task a spoken cancel names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelTarget {
    Research,
    Review,
    Change,
    /// "Cancel that": the task started most recently.
    Latest,
}

/// Utterances that cancel a running harness task (PRODUCT §6.2 voice control). The whole
/// utterance must be one of these, give or take [`HARNESS_CANCEL_FILLERS`], so "stop" alone stays
/// an interruption and "cancel that and explain Raft" goes to the talker as words.
pub const HARNESS_CANCELS: &[(&str, CancelTarget)] = &[
    ("stop that research", CancelTarget::Research),
    ("stop the research", CancelTarget::Research),
    ("stop researching", CancelTarget::Research),
    ("cancel that research", CancelTarget::Research),
    ("cancel the research", CancelTarget::Research),
    ("cancel the search", CancelTarget::Research),
    ("stop that review", CancelTarget::Review),
    ("stop the review", CancelTarget::Review),
    ("cancel that review", CancelTarget::Review),
    ("cancel the review", CancelTarget::Review),
    ("stop that change", CancelTarget::Change),
    ("stop the change", CancelTarget::Change),
    ("cancel that change", CancelTarget::Change),
    ("cancel the change", CancelTarget::Change),
    ("cancel that", CancelTarget::Latest),
    ("cancel it", CancelTarget::Latest),
    ("stop that task", CancelTarget::Latest),
    ("cancel that task", CancelTarget::Latest),
    ("cancel the task", CancelTarget::Latest),
];

/// Words allowed around a spoken cancel ("okay, cancel that please").
pub const HARNESS_CANCEL_FILLERS: &[&str] = &[
    "please", "okay", "ok", "actually", "just", "no", "oh", "hey", "yeah", "now",
];

/// Openers that start an interruption without being part of the question ("Wait — why Redis
/// first?" asks "why Redis first?"). A detour card shows the question without them.
pub const QUESTION_LEAD_INS: &[&str] = &[
    "wait", "hold on", "hang on", "sorry", "actually", "oh", "um", "uh", "hmm", "okay", "ok", "so",
    "hey", "but",
];

/// "Forget that" (PRODUCT §7.4): the whole utterance, so "forget that, tell me about Kafka"
/// stays a question for the talker. Board words ("remove that", "delete that") are not here:
/// they can mean a box on the board; nor is "forget it", which usually means "never mind".
pub const FORGET_LAST_PHRASES: &[&str] = &[
    "forget that",
    "forget that one",
    "forget the last one",
    "forget the last thing",
    "don't remember that",
    "do not remember that",
    "don't keep that",
    "do not keep that",
    "don't save that",
    "do not save that",
];

/// "Don't remember this session" (PRODUCT §7.4, I10), as a whole utterance.
pub const FORGET_SESSION_PHRASES: &[&str] = &[
    "don't remember this session",
    "do not remember this session",
    "don't remember any of this",
    "do not remember any of this",
    "forget this session",
    "forget this whole session",
    "forget this entire session",
    "don't save this session",
    "do not save this session",
    "don't keep this session",
    "do not keep this session",
    "don't record this session",
    "do not record this session",
    "this session is off the record",
    "this is off the record",
    "keep this off the record",
    "off the record",
];

/// Keeping a proposed memory ("yes, keep it"): these, or a plain yes (`AFFIRMATIVES`).
pub const KEEP_PHRASES: &[&str] = &[
    "keep it",
    "keep that",
    "keep that one",
    "keep this",
    "remember it",
    "remember that",
    "save it",
    "save that",
    "please do",
];

/// Closing the open view by voice, as a whole utterance.
pub const CLOSE_VIEW_PHRASES: &[&str] = &[
    "close it",
    "close that",
    "close this",
    "close the library",
    "close the list",
    "close the view",
    "hide it",
    "hide that",
    "hide this",
    "go back",
    "back to the board",
    "never mind",
    "that's all",
    "done",
];

/// Opening the settings screen by voice, as a whole utterance (the first-run screen's footer:
/// "Change any of this later by saying 'open settings'").
pub const OPEN_SETTINGS_PHRASES: &[&str] = &[
    "open settings",
    "open the settings",
    "open your settings",
    "open my settings",
    "show settings",
    "show the settings",
    "show me the settings",
    "show me settings",
    "go to settings",
    "change settings",
    "change the settings",
    "change my settings",
    "open preferences",
    "settings",
];

/// Words allowed around a memory control or a close without changing it ("no, forget that").
pub const CONTROL_FILLERS: &[&str] = &[
    "please", "ok", "okay", "oh", "just", "actually", "hey", "no", "yes", "yeah", "now", "and",
    "thanks", "thank", "you",
];

/// Abbreviations whose period does not end a sentence, so a clause is not cut after them.
pub const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "mr.", "mrs.", "dr.", "approx.",
];

/// Normalized words of an utterance, in order. A typographic apostrophe becomes `'`, as
/// speech-to-text writes either.
#[must_use]
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|ch: char| !ch.is_alphanumeric())
                .replace('’', "'")
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
pub(crate) fn made_of(text: &str, table: &[&str], fillers: &[&str]) -> bool {
    made_of_any(text, &[table], fillers)
}

/// [`made_of`] over several tables at once.
pub(crate) fn made_of_any(text: &str, tables: &[&[&str]], fillers: &[&str]) -> bool {
    let words = words(text);
    let mut index = 0;
    let mut phrases = 0;
    while index < words.len() {
        let longest = tables
            .iter()
            .flat_map(|table| table.iter())
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

/// The harness task an utterance cancels, when the utterance is a cancel and nothing else.
#[must_use]
pub fn harness_cancel(text: &str) -> Option<CancelTarget> {
    let words = words(text);
    let phrases: Vec<&str> = HARNESS_CANCELS.iter().map(|(phrase, _)| *phrase).collect();
    if !made_of(text, &phrases, HARNESS_CANCEL_FILLERS) {
        return None;
    }
    // The longest phrase found decides ("cancel that research" names research, not "that").
    HARNESS_CANCELS
        .iter()
        .filter(|(phrase, _)| {
            (0..words.len()).any(|at| phrase_at(&words, at, phrase, false).is_some())
        })
        .max_by_key(|(phrase, _)| phrase.len())
        .map(|(_, target)| *target)
}

/// The question without its lead-in ("Wait — why Redis first?" → "why Redis first?"), as it was
/// written; empty when the utterance was only a lead-in.
#[must_use]
pub fn without_lead_in(text: &str) -> &str {
    let mut rest = skip_marks(text);
    // Each pass removes at least one written word, so the loop ends.
    loop {
        let words = words(rest);
        let Some(length) = QUESTION_LEAD_INS
            .iter()
            .filter_map(|lead_in| phrase_at(&words, 0, lead_in, false))
            .max()
        else {
            return rest;
        };
        for _ in 0..length {
            rest = skip_marks(rest);
            rest = rest
                .find(char::is_whitespace)
                .map_or("", |end| &rest[end..]);
        }
        rest = skip_marks(rest);
    }
}

/// Skips spaces, dashes and punctuation before the next word.
fn skip_marks(text: &str) -> &str {
    text.trim_start_matches(|ch: char| !ch.is_alphanumeric())
}

/// A memory control the user said, as a whole utterance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryCue {
    ForgetLast,
    ForgetSession,
}

/// "Forget that" or "don't remember this session", or neither. The session phrase is tried
/// first: it is the longer, stronger request.
#[must_use]
pub fn memory_cue(text: &str) -> Option<MemoryCue> {
    if made_of(text, FORGET_SESSION_PHRASES, CONTROL_FILLERS) {
        Some(MemoryCue::ForgetSession)
    } else if made_of(text, FORGET_LAST_PHRASES, CONTROL_FILLERS) {
        Some(MemoryCue::ForgetLast)
    } else {
        None
    }
}

/// The proposal gate's test: "keep it", or a plain yes, and nothing else.
#[must_use]
pub fn is_keep(text: &str) -> bool {
    made_of_any(text, &[KEEP_PHRASES, AFFIRMATIVES], CONTROL_FILLERS)
}

/// "Close that" while a view is open.
#[must_use]
pub fn is_close_view(text: &str) -> bool {
    made_of(text, CLOSE_VIEW_PHRASES, CONTROL_FILLERS)
}

/// "Open settings", whatever is on screen.
#[must_use]
pub fn is_open_settings(text: &str) -> bool {
    made_of(text, OPEN_SETTINGS_PHRASES, CONTROL_FILLERS)
}

/// Marks speech-to-text closes a sentence with. An ellipsis ends with `.` too, but it trails
/// off: [`CONTINUATION_ENDINGS`] is checked first.
pub const SENTENCE_END_MARKS: &[char] = &['.', '?', '!'];

/// Closing marks that may follow a sentence end in a transcript ("it fails.)").
const TRANSCRIPT_CLOSERS: &[char] = &['"', '\'', ')', ']', '”', '’'];

/// How the words so far sound, for the end of the turn (PRODUCT §6.1, LLD §2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    /// Closed with a sentence end, and not on a continuation word: "Why Redis first?"
    Finished,
    /// Neither: no punctuation ("ok go on"), or nothing said.
    Unclear,
    /// A continuation word or a trailing-off mark: "so the leader…", "and then the".
    Unfinished,
}

/// Whether `text` sounds finished, unfinished, or neither. Partial transcripts are not to be
/// trusted with this: Whisper punctuates a partial as if it were the end ("How does a request
/// get?"), so the caller asks only of a final.
#[must_use]
pub fn completeness(text: &str) -> Completeness {
    if ends_unfinished(text) {
        return Completeness::Unfinished;
    }
    let closed = text.trim_end().trim_end_matches(TRANSCRIPT_CLOSERS);
    if closed.ends_with(SENTENCE_END_MARKS) || is_whole_reply(text) {
        Completeness::Finished
    } else {
        Completeness::Unclear
    }
}

/// Words that are a whole reply by what they are, closing mark or not: an acknowledgment or a
/// yes ("mm-hm", "yeah, okay"), going on ("Okay, go on") or going back ("go back", "back
/// out"). Whisper often leaves the mark off a short phrase (`just e2e` heard "Okay, go on." as
/// "Okay, go on"), which made such a turn wait the unclear 600 ms. The table matches the whole
/// utterance, so "go on with the cache" stays unclear. Controls that can't be taken back
/// (ending the session, forgetting) are left out: they keep the longer wait.
fn is_whole_reply(text: &str) -> bool {
    !words(text).is_empty()
        && (is_backchannel(text)
            || super::stage_cues::main_board_cue(text).is_some()
            || super::detail_cues::is_back_out(text))
}

/// The words end with a question mark, give or take closing quotes ("…how it evicts?").
#[must_use]
pub fn ends_on_question(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(TRANSCRIPT_CLOSERS)
        .ends_with('?')
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
        AFFIRMATIVES, BACKCHANNELS, CLOSE_VIEW_PHRASES, CONTINUATION_ENDINGS, CONTINUATION_WORDS,
        CancelTarget, FORGET_LAST_PHRASES, FORGET_SESSION_PHRASES, HARNESS_CANCELS,
        INTERRUPTION_CUES, KEEP_PHRASES, MemoryCue, QUESTION_LEAD_INS, ends_unfinished,
        harness_cancel, has_interruption_cue, is_affirmative, is_backchannel, is_close_view,
        is_keep, memory_cue, without_lead_in, word_count,
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

    /// Whisper's early partials of real cues (`spike_turns`, D-159) stop BoardAI as the cues do.
    #[test]
    fn early_partial_spellings_of_cues_interrupt() {
        for partial in [
            "way", "We", "Hold.", "Hold it.", "Hang!", "action.", "actual", "Hold up.",
        ] {
            assert!(has_interruption_cue(partial), "{partial}");
            assert!(!is_backchannel(partial), "{partial}");
        }
        for not_a_cue in ["Weight", "wean", "holder", "hanger", "Okay.", "Yeah."] {
            assert!(!has_interruption_cue(not_a_cue), "{not_a_cue}");
        }
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

    /// A whole reply sounds finished without its closing mark (D-261): every acknowledgment,
    /// going on, going back and backing out, as Whisper writes them bare. Near misses and the
    /// controls that can't be taken back keep the unclear wait.
    #[test]
    fn a_whole_reply_sounds_finished_without_its_mark() {
        use super::super::detail_cues::BACK_OUT_PHRASES;
        use super::super::stage_cues::{
            BACK_TO_MAIN_PHRASES, CONTINUE_PHRASES, END_SESSION_PHRASES,
        };
        use super::{Completeness, completeness};
        let tables: [&[&str]; 4] = [
            BACKCHANNELS,
            CONTINUE_PHRASES,
            BACK_TO_MAIN_PHRASES,
            BACK_OUT_PHRASES,
        ];
        for phrase in tables.into_iter().flatten() {
            // A continuation word still wins ("back to where we were"), so "go on and" waits.
            let whole = if ends_unfinished(phrase) {
                Completeness::Unfinished
            } else {
                Completeness::Finished
            };
            assert_eq!(completeness(phrase), whole, "{phrase}");
            let noisy = format!("Okay, {}", phrase.to_uppercase());
            assert_eq!(completeness(&noisy), whole, "{noisy}");
            let more = format!("{phrase} with the cache");
            assert_eq!(completeness(&more), Completeness::Unclear, "{more}");
            let unfinished = format!("{phrase} and");
            assert_eq!(
                completeness(&unfinished),
                Completeness::Unfinished,
                "{unfinished}"
            );
        }
        for phrase in END_SESSION_PHRASES.iter().chain(FORGET_SESSION_PHRASES) {
            assert_eq!(completeness(phrase), Completeness::Unclear, "{phrase}");
        }
        for unclear in ["", "  ", "show me the code", "how does it work", "redis"] {
            assert_eq!(completeness(unclear), Completeness::Unclear, "{unclear:?}");
        }
    }

    #[test]
    fn every_spoken_cancel_names_its_task_and_other_words_do_not() {
        for (phrase, target) in HARNESS_CANCELS {
            assert_eq!(harness_cancel(phrase), Some(*target), "{phrase}");
            let noisy = format!("Okay, {}, please.", phrase.to_uppercase());
            assert_eq!(harness_cancel(&noisy), Some(*target), "{noisy}");
        }
        for other in [
            "",
            "stop",
            "wait",
            "cancel",
            "stop talking",
            "cancel that and explain Raft",
            "what did the research find",
        ] {
            assert_eq!(harness_cancel(other), None, "{other:?}");
        }
        assert_eq!(
            harness_cancel("cancel that research"),
            Some(CancelTarget::Research)
        );
    }

    #[test]
    fn a_question_loses_its_lead_in() {
        for lead_in in QUESTION_LEAD_INS {
            let text = format!("{lead_in}, why Redis first?");
            assert_eq!(without_lead_in(&text), "why Redis first?", "{text}");
        }
        assert_eq!(
            without_lead_in("Wait — why Redis first?"),
            "why Redis first?"
        );
        assert_eq!(
            without_lead_in("Hold on, hang on... what's a TTL?"),
            "what's a TTL?"
        );
        assert_eq!(without_lead_in("Wait."), "");
        assert_eq!(without_lead_in("Why Redis?"), "Why Redis?");
        assert_eq!(without_lead_in("Sorting is slow?"), "Sorting is slow?");
    }

    #[test]
    fn every_memory_control_is_recognized_whole_and_only_whole() {
        for phrase in FORGET_LAST_PHRASES {
            assert_eq!(memory_cue(phrase), Some(MemoryCue::ForgetLast), "{phrase}");
            let noisy = format!("No, {}, please.", phrase.to_uppercase());
            assert_eq!(memory_cue(&noisy), Some(MemoryCue::ForgetLast), "{noisy}");
            let more = format!("{phrase} and tell me about Kafka");
            assert_eq!(memory_cue(&more), None, "{more}");
        }
        for phrase in FORGET_SESSION_PHRASES {
            assert_eq!(
                memory_cue(phrase),
                Some(MemoryCue::ForgetSession),
                "{phrase}"
            );
            let more = format!("{phrase} except the Redis part");
            assert_eq!(memory_cue(&more), None, "{more}");
        }
        assert_eq!(
            memory_cue("Don’t remember this session."),
            Some(MemoryCue::ForgetSession)
        );
        for other in [
            "",
            "forget",
            "remove that",
            "delete that",
            "what did we forget",
        ] {
            assert_eq!(memory_cue(other), None, "{other:?}");
        }
    }

    #[test]
    fn keeping_takes_a_keep_phrase_or_a_plain_yes() {
        for phrase in KEEP_PHRASES.iter().chain(AFFIRMATIVES) {
            assert!(is_keep(phrase), "{phrase}");
            assert!(is_keep(&format!("yes, {phrase}, thanks")), "{phrase}");
        }
        for other in [
            "",
            "no",
            "don't keep it",
            "keep going",
            "yes but explain Raft",
        ] {
            assert!(!is_keep(other), "{other:?}");
        }
    }

    #[test]
    fn closing_a_view_is_a_whole_utterance() {
        for phrase in CLOSE_VIEW_PHRASES {
            assert!(is_close_view(phrase), "{phrase}");
            assert!(
                is_close_view(&format!("okay, {phrase}, thanks")),
                "{phrase}"
            );
            assert!(
                !is_close_view(&format!("{phrase} and open Raft")),
                "{phrase}"
            );
        }
        assert!(!is_close_view("close the deal with Redis"));
    }

    #[test]
    fn counts_words_without_punctuation() {
        assert_eq!(word_count("wait, why Redis first?"), 4);
        assert_eq!(word_count(" -- "), 0);
    }
}
