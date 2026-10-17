//! Controls said in one breath ("Don't remember this session, we can stop here."). Each control
//! is matched against a whole utterance (`cues`, `stage_cues`, `sources::cues`), so two joined
//! matched neither,
//! and the words went to the talker, which has no tool to forget a session: nothing was
//! forgotten and the session didn't end.
//!
//! An utterance is joined controls when a mark (",", ".", "—") or a joining word ("and",
//! "then") splits it into parts that are each a control as it would be alone, give or take its
//! fillers, or only fillers ("Okay, …"), with at least two controls. Any part with anything else
//! in it makes the whole utterance the talker's: "explain Paxos, then we can stop here" is a
//! question.

use super::cues::{
    CONTROL_FILLERS, CancelTarget, MemoryCue, harness_cancel, is_backchannel, is_close_view,
    is_open_settings, memory_cue, words,
};
use super::detail_cues::{is_back_out, is_detail};
use super::stage_cues::{STAGE_FILLERS, is_end_session};
use crate::sources::cues::{TrayCue, keep_cue, tray_cue};

/// Words that join two controls ("forget that and close it"). None of the controls' phrases has
/// one of these in it, so a split never cuts a phrase.
pub const CONTROL_JOINERS: &[&str] = &["and", "then", "also", "plus"];

/// Marks that close a part.
const PART_MARKS: &[char] = &[',', '.', ';', ':', '!', '?', '—', '–'];

/// Controls a joined utterance may hold: those the core answers itself, and a spoken cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinedCue {
    Memory(MemoryCue),
    CloseView,
    OpenSettings,
    Cancel(CancelTarget),
    EndSession,
    /// "Back out" of a detail layer (D-188).
    BackOut,
    /// "More detail on the arena": the talker fills the detail layer it opens.
    Detail,
    /// "Show sources", "hide sources" (`sources::cues::tray_cue`).
    Tray(TrayCue),
    /// "Keep this in the project" (`sources::cues::keep_cue`); its words name what and where.
    KeepSource,
}

/// One control of a joined utterance, with its words as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Joined<'a> {
    pub cue: JoinedCue,
    pub words: &'a str,
}

/// The controls of an utterance made only of two or more of them, in the order said.
#[must_use]
pub fn joined(text: &str) -> Option<Vec<Joined<'_>>> {
    let mut controls = Vec::new();
    for part in parts(text) {
        if is_filler(part) {
            continue;
        }
        controls.push(Joined {
            cue: control(part)?,
            words: part,
        });
    }
    (controls.len() >= 2).then_some(controls)
}

/// The control a part is, said as it would be alone.
fn control(part: &str) -> Option<JoinedCue> {
    if let Some(cue) = memory_cue(part) {
        Some(JoinedCue::Memory(cue))
    } else if let Some(target) = harness_cancel(part) {
        Some(JoinedCue::Cancel(target))
    } else if is_end_session(part) {
        Some(JoinedCue::EndSession)
    } else if is_open_settings(part) {
        Some(JoinedCue::OpenSettings)
    } else if is_close_view(part) {
        Some(JoinedCue::CloseView)
    } else if is_back_out(part) {
        Some(JoinedCue::BackOut)
    } else if is_detail(part) {
        Some(JoinedCue::Detail)
    } else if let Some(cue) = tray_cue(part) {
        Some(JoinedCue::Tray(cue))
    } else if keep_cue(part).is_some() {
        Some(JoinedCue::KeepSource)
    } else {
        None
    }
}

/// Only fillers and acknowledgments ("Okay", "great, thanks").
fn is_filler(part: &str) -> bool {
    is_backchannel(part)
        || words(part).iter().all(|word| {
            CONTROL_FILLERS.contains(&word.as_str()) || STAGE_FILLERS.contains(&word.as_str())
        })
}

/// The parts of `text` between marks and joining words, trimmed, as written. One pass over the
/// words, so it is bounded by the utterance (`UTTERANCE_MAX_BYTES`).
fn parts(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut begin = 0;
    let mut offset = 0;
    for token in text.split_inclusive(char::is_whitespace) {
        let word = token.trim_end();
        let end = offset + token.len();
        let normalized = words(word);
        if normalized
            .first()
            .is_some_and(|first| CONTROL_JOINERS.contains(&first.as_str()))
            && normalized.len() == 1
        {
            parts.push(&text[begin..offset]);
            begin = end;
        } else if word.ends_with(PART_MARKS) {
            parts.push(&text[begin..offset + word.len()]);
            begin = end;
        }
        offset = end;
    }
    parts.push(&text[begin..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{JoinedCue, joined};
    use crate::conversation::cues::{CancelTarget, MemoryCue};
    use crate::sources::cues::TrayCue;

    fn cues(text: &str) -> Option<Vec<JoinedCue>> {
        joined(text).map(|controls| controls.iter().map(|control| control.cue).collect())
    }

    #[test]
    fn controls_said_in_one_breath_are_each_found_in_order() {
        let session = JoinedCue::Memory(MemoryCue::ForgetSession);
        for (said, expected) in [
            (
                "Okay, don't remember this session, we can stop here.",
                vec![session, JoinedCue::EndSession],
            ),
            (
                "We can stop here, and don't save this session.",
                vec![JoinedCue::EndSession, session],
            ),
            (
                "forget that and close it",
                vec![
                    JoinedCue::Memory(MemoryCue::ForgetLast),
                    JoinedCue::CloseView,
                ],
            ),
            (
                "Cancel that research, then end the session, thanks.",
                vec![
                    JoinedCue::Cancel(CancelTarget::Research),
                    JoinedCue::EndSession,
                ],
            ),
            (
                "Close that — open settings.",
                vec![JoinedCue::CloseView, JoinedCue::OpenSettings],
            ),
            (
                "Back out, then more detail on the slot pool.",
                vec![JoinedCue::BackOut, JoinedCue::Detail],
            ),
            (
                "close that and back out",
                vec![JoinedCue::CloseView, JoinedCue::BackOut],
            ),
        ] {
            assert_eq!(cues(said), Some(expected), "{said:?}");
        }
        let parts = joined("Okay, forget this session, we can stop here.").expect("joined");
        assert_eq!(parts[0].words, "forget this session,");
        assert_eq!(parts[1].words, "we can stop here.");
    }

    /// One control is matched whole as before, and a part with anything else in it makes the
    /// whole utterance the talker's.
    #[test]
    fn a_single_control_or_a_question_is_not_joined() {
        for said in [
            "",
            "we can stop here",
            "Okay, I think we can stop here.",
            "Explain Paxos briefly, we can stop here.",
            "Explain Paxos briefly, then we can stop here.",
            "forget that and tell me about Kafka",
            "don't remember this session except the Redis part, we can stop here",
            "don't end the session, close that",
            "what about settings, and close that",
            "close the deal with Redis, and we can stop here",
            "is that settings? close that",
            "stop, and go back",
            "new session, and close that",
        ] {
            assert_eq!(cues(said), None, "{said:?}");
        }
    }

    /// The sources tray and "keep this in the project" join other controls as they would be
    /// said alone.
    #[test]
    fn source_controls_join_other_controls() {
        let show = JoinedCue::Tray(TrayCue::Show);
        let hide = JoinedCue::Tray(TrayCue::Hide);
        for (said, expected) in [
            (
                "Keep this in the project, then show sources.",
                vec![JoinedCue::KeepSource, show],
            ),
            (
                "Hide sources and open settings.",
                vec![hide, JoinedCue::OpenSettings],
            ),
            (
                "Okay, keep it in the infinitydb project, we can stop here.",
                vec![JoinedCue::KeepSource, JoinedCue::EndSession],
            ),
            (
                "close that, and show me the sources",
                vec![JoinedCue::CloseView, show],
            ),
        ] {
            assert_eq!(cues(said), Some(expected), "{said:?}");
        }
    }

    /// A sources phrase inside a question, or a keep with no project, joins nothing: the
    /// talker hears the words.
    #[test]
    fn source_words_in_a_question_are_not_joined_controls() {
        for said in [
            "show sources",
            "keep this in the project",
            "show sources and explain the arena",
            "keep this in mind, we can stop here",
            "keep it, and close that",
            "show me the sources of the bug, and close that",
            "keep this in the project and tell me about Kafka",
            "what are the sources of truth here, then hide sources",
            "hide the sources of latency, and open settings",
        ] {
            assert_eq!(cues(said), None, "{said:?}");
        }
    }
}
