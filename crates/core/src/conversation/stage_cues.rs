//! Cues about the stage itself rather than what is on it (D-144, D-147, D-177): ending the
//! session, going back to the main board from a detour, and starting a fresh page. Like the
//! memory controls (`cues`), each is matched against the whole utterance, so a request with
//! anything else in it stays the talker's; a fresh page may also lead or end a request ("clear
//! the board and explain DNS").
//!
//! The words around a cue may be fillers and acknowledgments ("I think that is okay, I think we
//! can stop here."), never content: "explain Paxos, we can stop here" is a question.

use super::cues::{CONTROL_FILLERS, made_of, words};

/// Ending the session by voice (D-144): the core answers, the session ends, and the next words
/// open a new one. Only explicit forms: a bare "new session" can be a question about one, and
/// "stop here" alone can mean "stop talking".
pub const END_SESSION_PHRASES: &[&str] = &[
    "end the session",
    "end this session",
    "end our session",
    "end session",
    "finish the session",
    "finish this session",
    "close the session",
    "close this session",
    "stop the session",
    "stop this session",
    "start a new session",
    "start another session",
    "start a fresh session",
    "begin a new session",
    "open a new session",
    "start a new conversation",
    "we can stop here",
    "we can stop now",
    "we can stop for today",
    "let's stop here",
    "let's stop now",
    "let's stop for today",
    "lets stop here",
    "lets stop now",
    "let us stop here",
    "done for today",
    "that's it for today",
    "that's all for today",
    "let's call it a day",
];

/// Going back to the main board (D-147): every open detour closes, whatever the talker does.
pub const BACK_TO_MAIN_PHRASES: &[&str] = &[
    "back to the main board",
    "go back to the main board",
    "back to the board",
    "go back to the board",
    "go back",
    "let's go back",
    "lets go back",
    "back to the main topic",
    "back to the main explanation",
    "back to the main thread",
    "back to the main one",
    "back to where we were",
    "where were we",
    "return to the main board",
    "close the detour",
    "close this detour",
    "close the detours",
    "leave the detour",
    "exit the detour",
    "end the detour",
    "done with the detour",
];

/// Asking to go on once a detour's answer has ended (D-147): the main explanation continues.
pub const CONTINUE_PHRASES: &[&str] = &[
    "go on",
    "continue",
    "keep going",
    "carry on",
    "move on",
    "let's move on",
    "lets move on",
    "let's continue",
    "lets continue",
    "please continue",
    "you can continue",
    "you can go on",
    "keep explaining",
    "continue explaining",
    "next",
];

/// Words around a stage cue that don't change it: the memory controls' fillers, plus how people
/// close a topic ("got it, that makes sense, go on").
pub const STAGE_FILLERS: &[&str] = &[
    "i",
    "think",
    "we",
    "we're",
    "are",
    "can",
    "could",
    "let's",
    "lets",
    "want",
    "to",
    "would",
    "like",
    "go",
    "ahead",
    "so",
    "maybe",
    "that",
    "that's",
    "is",
    "it's",
    "seems",
    "fine",
    "good",
    "great",
    "cool",
    "alright",
    "all",
    "right",
    "got",
    "it",
    "makes",
    "sense",
    "clear",
    "perfect",
    "understood",
    "i'm",
    "see",
];

/// Starting a fresh page (D-177): the board turns to a new, empty one and the old page is kept.
/// "Start fresh" was an end-of-session phrase (D-144, amended by D-182); the owner used it for a
/// clean board ("clean the current board and start fresh"), and a page turn loses nothing.
pub const PAGE_PHRASES: &[&str] = &[
    "clear the board",
    "clean the board",
    "clear this board",
    "clean this board",
    "clear the current board",
    "clean the current board",
    "clear the whiteboard",
    "clean the whiteboard",
    "clear up the board",
    "clean up the board",
    "clear out the board",
    "wipe the board",
    "erase the board",
    "clear the canvas",
    "clean the canvas",
    "new page",
    "a new page",
    "fresh page",
    "a fresh page",
    "blank page",
    "a blank page",
    "new board",
    "a new board",
    "fresh board",
    "a fresh board",
    "clean board",
    "a clean board",
    "clean slate",
    "a clean slate",
    "start fresh",
    "start from scratch",
    "turn the page",
];

/// Words that join a page cue to the request it leads or ends ("clear the board and explain
/// DNS", "explain DNS on a fresh page"). Without one (or a comma), "new page tables" is a
/// question about paging, not a cue.
const PAGE_JOINERS: &[&str] = &[
    "and", "then", "so", "with", "now", "for", "on", "in", "onto", "but", "first", "before",
    "after", "to", "please",
];

/// How a page cue was said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCue {
    /// Only a clean board was asked for: the core answers.
    Only,
    /// A clean board, then a request (or a request on a clean board): the talker answers.
    WithRequest,
}

/// What going back to the main board was asked as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainBoardCue {
    /// "Back to the main board", "go back": any time a detour is open.
    Back,
    /// "Go on", "continue": only once the detour's answer ended (the words didn't cut BoardAI
    /// off), since mid-answer they ask for more of the detour.
    Continue,
}

fn made_of_stage(text: &str, table: &[&str]) -> bool {
    let fillers: Vec<&str> = CONTROL_FILLERS
        .iter()
        .chain(STAGE_FILLERS)
        .copied()
        .collect();
    made_of(text, table, &fillers)
}

/// "End the session", "we can stop here", "start a new session", and nothing else.
#[must_use]
pub fn is_end_session(text: &str) -> bool {
    made_of_stage(text, END_SESSION_PHRASES)
}

/// "Clear the board" or "new page", alone or joined to a request at its start or end.
#[must_use]
pub fn page_cue(text: &str) -> Option<PageCue> {
    if made_of_stage(text, PAGE_PHRASES) {
        return Some(PageCue::Only);
    }
    let tokens = tokens(text);
    (leads_request(&tokens) || ends_request(&tokens)).then_some(PageCue::WithRequest)
}

/// A word of the utterance, and whether a comma or another mark closed it.
type Token = (String, bool);

fn tokens(text: &str) -> Vec<Token> {
    text.split_whitespace()
        .filter_map(|raw| {
            let word = words(raw).into_iter().next()?;
            let marked = raw.ends_with([',', ':', ';', '.', '!', '—', '-']);
            Some((word, marked))
        })
        .collect()
}

fn page_fillers() -> Vec<&'static str> {
    CONTROL_FILLERS
        .iter()
        .chain(STAGE_FILLERS)
        .copied()
        .filter(|filler| !PAGE_JOINERS.contains(filler))
        .collect()
}

/// The longest page phrase starting at `index`, as a word count.
fn page_phrase_at(tokens: &[Token], index: usize) -> Option<usize> {
    PAGE_PHRASES
        .iter()
        .filter(|phrase| {
            phrase.split(' ').enumerate().all(|(offset, part)| {
                tokens
                    .get(index + offset)
                    .is_some_and(|(word, _)| word == part)
            })
        })
        .map(|phrase| phrase.split(' ').count())
        .max()
}

/// "Clear the board and explain DNS": fillers, a page phrase, then a joiner or a mark, then
/// the request.
fn leads_request(tokens: &[Token]) -> bool {
    let fillers = page_fillers();
    // Fillers may stand before the phrase, and a phrase may start with one ("clear").
    let Some((start, length)) = tokens
        .iter()
        .enumerate()
        .map(|(index, (word, _))| (index, word, page_phrase_at(tokens, index)))
        .take_while(|(_, word, phrase)| phrase.is_some() || fillers.contains(&word.as_str()))
        .find_map(|(index, _, phrase)| Some((index, phrase?)))
    else {
        return false;
    };
    let end = start + length;
    let marked = tokens.get(end - 1).is_some_and(|(_, marked)| *marked);
    let joined = tokens
        .get(end)
        .is_some_and(|(word, _)| PAGE_JOINERS.contains(&word.as_str()));
    let rest = tokens[end..]
        .iter()
        .filter(|(word, _)| {
            !PAGE_JOINERS.contains(&word.as_str()) && !fillers.contains(&word.as_str())
        })
        .count();
    (marked || joined) && rest > 0
}

/// "Explain DNS on a fresh page", "explain DNS, but clear the board first": the request, a
/// joiner or a mark, then a page phrase and nothing but fillers.
fn ends_request(tokens: &[Token]) -> bool {
    let fillers = page_fillers();
    let tail = |word: &str| fillers.contains(&word) || matches!(word, "first" | "please");
    let end = tokens
        .iter()
        .rposition(|(word, _)| !tail(word))
        .map_or(0, |last| last + 1);
    (1..end).any(|start| {
        page_phrase_at(tokens, start).is_some_and(|length| start + length == end)
            && tokens
                .get(start - 1)
                .is_some_and(|(word, marked)| *marked || PAGE_JOINERS.contains(&word.as_str()))
            && tokens[..start - 1].iter().any(|(word, _)| {
                !fillers.contains(&word.as_str()) && !PAGE_JOINERS.contains(&word.as_str())
            })
    })
}

/// "Back to the main board" or "go on", and nothing else. Going back is tried first: "go back"
/// is the stronger request.
#[must_use]
pub fn main_board_cue(text: &str) -> Option<MainBoardCue> {
    if made_of_stage(text, BACK_TO_MAIN_PHRASES) {
        Some(MainBoardCue::Back)
    } else if made_of_stage(text, CONTINUE_PHRASES) {
        Some(MainBoardCue::Continue)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACK_TO_MAIN_PHRASES, CONTINUE_PHRASES, END_SESSION_PHRASES, MainBoardCue, PAGE_PHRASES,
        PageCue, is_end_session, main_board_cue, page_cue,
    };

    #[test]
    fn every_end_phrase_ends_the_session_whole_and_only_whole() {
        for phrase in END_SESSION_PHRASES {
            assert!(is_end_session(phrase), "{phrase}");
            let noisy = format!("Okay, I think {}.", phrase.to_uppercase());
            assert!(is_end_session(&noisy), "{noisy}");
            let more = format!("explain Raft, then {phrase}");
            assert!(!is_end_session(&more), "{more}");
            assert_eq!(main_board_cue(phrase), None, "{phrase}");
        }
    }

    /// The owner's own words for finishing (session log of 2026-10-16), and near misses that
    /// must stay questions for the talker.
    #[test]
    fn the_owners_words_end_it_and_questions_do_not() {
        for said in [
            "I think that is okay, I think we can stop here.",
            "Yeah, that seems okay. Can we start another session?",
            "Can you end the session?",
        ] {
            assert!(is_end_session(said), "{said}");
        }
        assert!(
            !is_end_session("Let's start fresh."),
            "a fresh page now (D-177)"
        );
        for asked in [
            "",
            "stop",
            "stop here",
            "new session",
            "is that a new session?",
            "how does a session end?",
            "No, I think this is ok, let's just recall these sessions, we can stop here.",
            "Explain Paxos briefly, we can stop here.",
            "don't end the session",
        ] {
            assert!(!is_end_session(asked), "{asked:?}");
        }
    }

    #[test]
    fn every_back_phrase_goes_back_and_every_continue_phrase_continues() {
        for phrase in BACK_TO_MAIN_PHRASES {
            assert_eq!(main_board_cue(phrase), Some(MainBoardCue::Back), "{phrase}");
            let noisy = format!("Okay, got it, {}, please.", phrase.to_uppercase());
            assert_eq!(main_board_cue(&noisy), Some(MainBoardCue::Back), "{noisy}");
            let more = format!("{phrase} and draw the cache");
            assert_eq!(main_board_cue(&more), None, "{more}");
        }
        for phrase in CONTINUE_PHRASES {
            assert_eq!(
                main_board_cue(phrase),
                Some(MainBoardCue::Continue),
                "{phrase}"
            );
            let noisy = format!("Ok, that makes sense, {phrase}.");
            assert_eq!(
                main_board_cue(&noisy),
                Some(MainBoardCue::Continue),
                "{noisy}"
            );
            let more = format!("{phrase} with the inode table");
            assert_eq!(main_board_cue(&more), None, "{more}");
        }
        for other in [
            "",
            "why go back to Redis?",
            "what comes next in Raft?",
            "okay",
        ] {
            assert_eq!(main_board_cue(other), None, "{other:?}");
        }
    }

    #[test]
    fn every_page_phrase_turns_the_page_alone_or_joined_to_a_request() {
        for phrase in PAGE_PHRASES {
            assert_eq!(page_cue(phrase), Some(PageCue::Only), "{phrase}");
            let noisy = format!("Okay, can you {}, please?", phrase.to_uppercase());
            assert_eq!(page_cue(&noisy), Some(PageCue::Only), "{noisy}");
            let leads = format!("{phrase} and explain DNS");
            assert_eq!(page_cue(&leads), Some(PageCue::WithRequest), "{leads}");
            let ends = format!("explain DNS, but {phrase} first");
            assert_eq!(page_cue(&ends), Some(PageCue::WithRequest), "{ends}");
            assert!(!is_end_session(phrase), "{phrase}");
            assert_eq!(main_board_cue(phrase), None, "{phrase}");
        }
    }

    /// The owner's words (2026-10-16) and the measurement's, then near misses that stay the
    /// talker's.
    #[test]
    fn the_owners_words_turn_the_page_and_questions_do_not() {
        for (said, cue) in [
            ("Clean the current board and start fresh.", PageCue::Only),
            ("Let's start fresh.", PageCue::Only),
            ("clear the board and explain DNS", PageCue::WithRequest),
            (
                "Clear the board, then show me how DNS works.",
                PageCue::WithRequest,
            ),
            ("New page: walk me through Raft.", PageCue::WithRequest),
            ("Now explain DNS on a fresh page.", PageCue::WithRequest),
        ] {
            assert_eq!(page_cue(said), Some(cue), "{said}");
        }
        for asked in [
            "",
            "clear",
            "new page tables in the kernel",
            "what happens on a new page fault?",
            "why do we clear the board?",
            "don't clear the board",
            "how does the kernel find a free page",
            "explain the board meeting",
            "start a fresh session",
        ] {
            assert_eq!(page_cue(asked), None, "{asked:?}");
        }
    }
}
