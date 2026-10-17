//! Cues about the stage itself rather than what is on it (D-181, D-184): ending the session, and
//! going back to the main board from a detour. Like the memory controls (`cues`), each is matched
//! against the whole utterance, so a request with anything else in it stays the talker's.
//!
//! The words around a cue may be fillers and acknowledgments ("I think that is okay, I think we
//! can stop here."), never content: "explain Paxos, we can stop here" is a question.

use super::cues::{CONTROL_FILLERS, made_of};

/// Ending the session by voice (D-181): the core answers, the session ends, and the next words
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
    "start fresh",
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

/// Going back to the main board (D-184): every open detour closes, whatever the talker does.
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

/// Asking to go on once a detour's answer has ended (D-184): the main explanation continues.
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
        BACK_TO_MAIN_PHRASES, CONTINUE_PHRASES, END_SESSION_PHRASES, MainBoardCue, is_end_session,
        main_board_cue,
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
            "Let's start fresh.",
        ] {
            assert!(is_end_session(said), "{said}");
        }
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
}
