//! What the user says about sources (PRODUCT §7.7), as tables the tests iterate:
//!
//! - **Share by name**: "use the inf-alloc crate", "share the storage module", "add the docs
//!   folder to the infinitydb project". The runtime resolves the name (D-184).
//! - **The tray**: "show sources", "hide sources".
//! - **The project**: "we're working on infinitydb", "switch to the harness project".
//! - **Keeping**: "keep this in the project", "keep the inf-alloc crate in the infinitydb
//!   project", "pin this".
//! - **More detail**: "more detail on the arena", "dig into the slot pool" (D-188).
//! - **Which one**: "the first one", "the crates one", when a name matched several folders.
//!
//! Every match is on normalized words (`conversation::cues::words`). Sharing with "use", "read"
//! or "look at" needs a kind word ("crate", "module"…), so "use Redis" stays a question.

use crate::conversation::cues::{CONTROL_FILLERS, made_of, words};
use crate::sources::{NameKind, name_key};

/// Verbs that share by name on their own ("share inf-alloc").
const SHARE_VERBS: &[&str] = &["share"];

/// Verbs that share by name only with a kind word after the name ("use the inf-alloc crate"):
/// "add jitter to the retries" is a request, not a source.
const SHARE_VERBS_WITH_KIND: &[&str] = &["use", "read", "look at", "study", "open", "add", "load"];

/// Words a name never holds: "jitter to the retries" is a phrase, not a folder's name.
const NAME_BREAKERS: &[&str] = &[
    "to", "in", "into", "for", "of", "with", "on", "from", "and", "or", "a", "an", "some", "how",
    "what", "why", "is", "are",
];

/// Kind words after a name, and what they ask the resolver for.
pub const KIND_WORDS: &[(&str, NameKind)] = &[
    ("crate", NameKind::Package),
    ("package", NameKind::Package),
    ("library", NameKind::Package),
    ("repo", NameKind::Package),
    ("repository", NameKind::Package),
    ("codebase", NameKind::Package),
    ("module", NameKind::Module),
    ("folder", NameKind::Folder),
    ("directory", NameKind::Folder),
    ("file", NameKind::File),
];

/// Words that can't be a source's name: "share this" names nothing to look for.
const NOT_NAMES: &[&str] = &[
    "it",
    "this",
    "that",
    "these",
    "those",
    "them",
    "something",
    "code",
    "one",
    "project",
];

/// Leading words that change nothing ("ok, let's use…", "can you share…").
const LEAD_WORDS: &[&str] = &[
    "ok", "okay", "please", "so", "now", "lets", "let's", "can", "could", "would", "you", "and",
    "then", "hey", "actually", "also", "go", "ahead",
];

/// Showing and hiding the tray, as a whole utterance.
pub const SHOW_SOURCES_PHRASES: &[&str] = &[
    "show sources",
    "show the sources",
    "show me the sources",
    "show my sources",
    "show me my sources",
    "open sources",
    "open the sources",
    "what can you read",
    "show what you can read",
    "sources please",
];
pub const HIDE_SOURCES_PHRASES: &[&str] = &[
    "hide sources",
    "hide the sources",
    "close sources",
    "close the sources",
];

/// Setting the session's project ("we're working on infinitydb").
pub const PROJECT_LEAD_INS: &[&str] = &[
    "we're working on",
    "we are working on",
    "i'm working on",
    "i am working on",
    "let's work on",
    "work on",
    "switch to",
    "this is",
    "we're on",
];

/// "More detail on the arena" and its ways of saying it; the name follows.
pub const DETAIL_PHRASES: &[&str] = &[
    "more detail on",
    "more details on",
    "more detail about",
    "more details about",
    "more detail into",
    "go deeper on",
    "go deeper into",
    "drill into",
    "drill down into",
    "drill down on",
    "dig into",
    "dig deeper into",
    "zoom into",
    "zoom in on",
    "tell me more about",
];

/// Words that end a spoken name ("the arena please").
const NAME_ENDERS: &[&str] = &["please", "and", "then", "now", "for", "in", "to", "so"];

/// Spoken names have at most this many words.
const NAME_WORDS_MAX: usize = 4;

/// Picking one of several candidates by position. "One" is not among them: "the vendor one"
/// picks by the word before it.
const ORDINALS: &[(&str, usize)] = &[
    ("first", 0),
    ("1", 0),
    ("second", 1),
    ("two", 1),
    ("2", 1),
    ("third", 2),
    ("three", 2),
    ("3", 2),
    ("fourth", 3),
    ("four", 3),
    ("4", 3),
];

/// "Use the inf-alloc crate": a name to resolve, and where to put what it finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareCue {
    pub name: String,
    pub kind: NameKind,
    pub keep: KeepIn,
}

/// Where a share or a keep puts the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepIn {
    /// This session only.
    Session,
    /// The current project ("in the project").
    CurrentProject,
    /// A project by name ("in the infinitydb project").
    Named(String),
}

/// "Keep this in the project".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepCue {
    /// The source named ("the inf-alloc crate"); `None` is the newest one shared.
    pub source: Option<String>,
    pub project: KeepIn,
}

/// The tray by voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCue {
    Show,
    Hide,
}

#[must_use]
pub fn tray_cue(text: &str) -> Option<TrayCue> {
    if made_of(text, SHOW_SOURCES_PHRASES, CONTROL_FILLERS) {
        Some(TrayCue::Show)
    } else if made_of(text, HIDE_SOURCES_PHRASES, CONTROL_FILLERS) {
        Some(TrayCue::Hide)
    } else {
        None
    }
}

/// The utterance's words without the leading ones that change nothing.
fn leading_trimmed(text: &str) -> Vec<String> {
    let mut words = words(text);
    let lead = words
        .iter()
        .take_while(|word| LEAD_WORDS.contains(&word.as_str()))
        .count();
    words.drain(..lead);
    words
}

/// The length of `phrase` when it starts `words` at `index`.
fn phrase_at(words: &[String], index: usize, phrase: &str) -> Option<usize> {
    let parts: Vec<&str> = phrase.split(' ').collect();
    let matched = parts
        .iter()
        .enumerate()
        .all(|(offset, part)| words.get(index + offset).is_some_and(|word| word == part));
    matched.then_some(parts.len())
}

fn longest_at(words: &[String], index: usize, table: &[&str]) -> Option<usize> {
    table
        .iter()
        .filter_map(|phrase| phrase_at(words, index, phrase))
        .max()
}

fn kind_of(word: &str) -> Option<NameKind> {
    KIND_WORDS
        .iter()
        .find(|(kind, _)| *kind == word || format!("{kind}s") == word)
        .map(|(_, kind)| *kind)
}

/// A name from `words`: leading articles dropped, at most [`NAME_WORDS_MAX`] words, none of them
/// a word that can't be a name.
fn name_of(words: &[String]) -> Option<String> {
    let start = words
        .iter()
        .take_while(|word| matches!(word.as_str(), "the" | "our" | "my" | "this" | "that"))
        .count();
    let name = &words[start..];
    let valid = !name.is_empty()
        && name.len() <= NAME_WORDS_MAX
        && !(name.len() == 1 && NOT_NAMES.contains(&name[0].as_str()))
        && name
            .iter()
            .all(|word| !name_key(word).is_empty() && !NAME_BREAKERS.contains(&word.as_str()));
    valid.then(|| name.join(" "))
}

/// Where the words after a name put it: nothing (this session), "for this session", "in the
/// project", "in the infinitydb project", "to the … project".
fn keep_tail(words: &[String]) -> Option<KeepIn> {
    match words {
        [] => Some(KeepIn::Session),
        [for_word, this, session]
            if for_word == "for" && this == "this" && session == "session" =>
        {
            Some(KeepIn::Session)
        }
        [preposition, rest @ ..] if matches!(preposition.as_str(), "in" | "to" | "into") => {
            let (project, name) = rest.split_last()?;
            if project != "project" {
                return None;
            }
            match name {
                [] => Some(KeepIn::CurrentProject),
                [only] if matches!(only.as_str(), "the" | "this" | "our" | "my") => {
                    Some(KeepIn::CurrentProject)
                }
                _ => name_of(name).map(KeepIn::Named),
            }
        }
        _ => None,
    }
}

/// "Use the inf-alloc crate", "share inf-alloc", "add the docs folder to the project".
#[must_use]
pub fn share_cue(text: &str) -> Option<ShareCue> {
    let words = leading_trimmed(text);
    let (verb_length, needs_kind) = longest_at(&words, 0, SHARE_VERBS)
        .map(|length| (length, false))
        .or_else(|| longest_at(&words, 0, SHARE_VERBS_WITH_KIND).map(|length| (length, true)))?;
    let rest = &words[verb_length..];
    // The name runs to a kind word, or to where a keep tail starts.
    for end in 1..=rest.len().min(NAME_WORDS_MAX + 1) {
        let kind = rest.get(end).and_then(|word| kind_of(word));
        let tail_start = if kind.is_some() { end + 1 } else { end };
        if needs_kind && kind.is_none() {
            continue;
        }
        let Some(keep) = keep_tail(&rest[tail_start..]) else {
            continue;
        };
        let Some(name) = name_of(&rest[..end]) else {
            continue;
        };
        return Some(ShareCue {
            name,
            kind: kind.unwrap_or(NameKind::Folder),
            keep,
        });
    }
    None
}

/// "Keep this in the project", "keep the inf-alloc crate in the infinitydb project", "pin this".
#[must_use]
pub fn keep_cue(text: &str) -> Option<KeepCue> {
    let words = leading_trimmed(text);
    let (verb, rest) = words.split_first()?;
    if !matches!(verb.as_str(), "keep" | "pin" | "save" | "add") {
        return None;
    }
    if verb == "pin" && matches!(rest, [this] if matches!(this.as_str(), "this" | "it" | "that")) {
        return Some(KeepCue {
            source: None,
            project: KeepIn::CurrentProject,
        });
    }
    let split = rest
        .iter()
        .position(|word| matches!(word.as_str(), "in" | "to" | "into"))?;
    let project = match keep_tail(&rest[split..])? {
        KeepIn::Session => return None,
        project @ (KeepIn::CurrentProject | KeepIn::Named(_)) => project,
    };
    let target = &rest[..split];
    let source = match target {
        [only] if matches!(only.as_str(), "this" | "it" | "that") => None,
        [] => return None,
        _ => {
            let without_kind = match target.split_last() {
                Some((last, before)) if kind_of(last).is_some() && !before.is_empty() => before,
                Some(_) | None => target,
            };
            Some(name_of(without_kind)?)
        }
    };
    Some(KeepCue { source, project })
}

/// "We're working on infinitydb": the project's name, and the words after a comma that still
/// go to the talker ("we're working on infinitydb, how does the arena work?").
#[must_use]
pub fn project_cue(text: &str) -> Option<(String, Option<String>)> {
    let (head, rest) = match text.split_once(',') {
        Some((head, rest)) if !rest.trim().is_empty() => (head, Some(rest.trim().to_owned())),
        Some((head, _)) => (head, None),
        None => (text, None),
    };
    let words = leading_trimmed(head);
    let lead = longest_at(&words, 0, PROJECT_LEAD_INS)?;
    let mut name = &words[lead..];
    if let Some((last, before)) = name.split_last()
        && last == "project"
    {
        name = before;
    }
    // "This is" asks about the project only when it ends in "project".
    let said_project = words.last().is_some_and(|word| word == "project");
    if words[..lead].join(" ") == "this is" && !said_project {
        return None;
    }
    Some((name_of(name)?, rest))
}

/// "More detail on the arena": the name after the phrase, wherever the phrase is.
#[must_use]
pub fn detail_cue(text: &str) -> Option<String> {
    let words = words(text);
    for index in 0..words.len() {
        let Some(length) = longest_at(&words, index, DETAIL_PHRASES) else {
            continue;
        };
        let after = &words[index + length..];
        let end = after
            .iter()
            .position(|word| NAME_ENDERS.contains(&word.as_str()))
            .unwrap_or(after.len());
        return name_of(&after[..end]);
    }
    None
}

/// Which of `candidates` (their shown paths) the words pick: an ordinal, "the last one", or a
/// word that names exactly one of them. `None` when nothing picks exactly one.
#[must_use]
pub fn choice(text: &str, candidates: &[&str]) -> Option<usize> {
    let words = words(text);
    if words.len() > NAME_WORDS_MAX + 3 {
        return None;
    }
    let last = candidates.len().checked_sub(1);
    let ordinal = (0..words.len()).find_map(|at| match words[at].as_str() {
        // "Last" picks only as an ordinal: "the vendor one, not the last" or "the one I used
        // last week" pick nothing by it.
        "last" => is_ordinal_at(&words, at).then_some(last).flatten(),
        word => ORDINALS
            .iter()
            .find(|(name, _)| *name == word)
            .filter(|_| is_ordinal_at(&words, at))
            .map(|(_, index)| *index),
    });
    if let Some(index) = ordinal {
        return (index < candidates.len()).then_some(index);
    }
    let parts: Vec<Vec<String>> = candidates
        .iter()
        .map(|path| path.split('/').map(name_key).collect())
        .collect();
    let mut picked = None;
    for word in &words {
        let key = name_key(word);
        if key.is_empty() || matches!(word.as_str(), "the" | "one" | "in") {
            continue;
        }
        let matching: Vec<usize> = (0..parts.len())
            .filter(|index| parts[*index].contains(&key))
            .collect();
        if let [only] = matching[..] {
            if picked.is_some_and(|known| known != only) {
                return None;
            }
            picked = Some(only);
        }
    }
    picked
}

/// Whether the word at `at` picks by its place: nothing negates it, and what follows it is
/// nothing, "one" or "of" ("the second one", "the last of them", "last").
fn is_ordinal_at(words: &[String], at: usize) -> bool {
    let negated = at
        .checked_sub(2)
        .into_iter()
        .chain(at.checked_sub(1))
        .any(|before| matches!(words[before].as_str(), "not" | "no" | "never"));
    let next = words.get(at + 1).map(String::as_str);
    !negated && matches!(next, None | Some("one" | "of" | "please"))
}

#[cfg(test)]
mod tests {
    use super::{
        HIDE_SOURCES_PHRASES, KeepCue, KeepIn, SHOW_SOURCES_PHRASES, ShareCue, TrayCue, choice,
        detail_cue, keep_cue, project_cue, share_cue, tray_cue,
    };
    use crate::sources::NameKind;

    fn share(name: &str, kind: NameKind, keep: KeepIn) -> Option<ShareCue> {
        Some(ShareCue {
            name: name.to_owned(),
            kind,
            keep,
        })
    }

    #[test]
    fn sharing_by_name_needs_a_name_and_for_some_verbs_a_kind() {
        let table = [
            (
                "Use the inf-alloc crate.",
                share("inf-alloc", NameKind::Package, KeepIn::Session),
            ),
            (
                "share the storage module",
                share("storage", NameKind::Module, KeepIn::Session),
            ),
            (
                "Okay, let's use the inf alloc crate",
                share("inf alloc", NameKind::Package, KeepIn::Session),
            ),
            (
                "Share inf-alloc.",
                share("inf-alloc", NameKind::Folder, KeepIn::Session),
            ),
            ("Can you add jitter to the retries?", None),
            ("add a task to check the TTLs", None),
            (
                "add the docs folder to the infinitydb project",
                share("docs", NameKind::Folder, KeepIn::Named("infinitydb".into())),
            ),
            (
                "use the inf-alloc crate in the project",
                share("inf-alloc", NameKind::Package, KeepIn::CurrentProject),
            ),
            (
                "read the README file",
                share("readme", NameKind::File, KeepIn::Session),
            ),
            ("Use Redis.", None),
            ("Should we use the cache?", None),
            ("share this", None),
            ("use it", None),
            ("use the very long name of a crate here crate", None),
        ];
        for (text, expected) in table {
            assert_eq!(share_cue(text), expected, "{text}");
        }
    }

    #[test]
    fn keeping_names_a_project() {
        let keep = |source: Option<&str>, project: KeepIn| {
            Some(KeepCue {
                source: source.map(str::to_owned),
                project,
            })
        };
        let table = [
            (
                "Keep this in the project.",
                keep(None, KeepIn::CurrentProject),
            ),
            (
                "keep it in the infinitydb project",
                keep(None, KeepIn::Named("infinitydb".into())),
            ),
            (
                "Keep the inf-alloc crate in the infinitydb project.",
                keep(Some("inf-alloc"), KeepIn::Named("infinitydb".into())),
            ),
            ("pin this", keep(None, KeepIn::CurrentProject)),
            ("keep it", None),
            ("keep this in mind", None),
            ("yes, keep it", None),
        ];
        for (text, expected) in table {
            assert_eq!(keep_cue(text), expected, "{text}");
        }
    }

    #[test]
    fn the_project_is_named_alone_or_before_a_question() {
        assert_eq!(
            project_cue("We're working on infinitydb."),
            Some(("infinitydb".into(), None))
        );
        assert_eq!(
            project_cue("we're working on infinitydb, how does the arena work?"),
            Some(("infinitydb".into(), Some("how does the arena work?".into())))
        );
        assert_eq!(
            project_cue("Switch to the harness project."),
            Some(("harness".into(), None))
        );
        assert_eq!(
            project_cue("This is the harness project"),
            Some(("harness".into(), None))
        );
        assert_eq!(project_cue("This is a cache"), None);
        assert_eq!(project_cue("How does it work?"), None);
    }

    #[test]
    fn more_detail_names_a_part_anywhere_in_the_words() {
        assert_eq!(
            detail_cue("More detail on the arena, please"),
            Some("arena".into())
        );
        assert_eq!(
            detail_cue("ok can you dig into the slot pool"),
            Some("slot pool".into())
        );
        assert_eq!(detail_cue("Tell me more about it"), None);
        assert_eq!(detail_cue("What is the arena?"), None);
    }

    #[test]
    fn a_choice_is_an_ordinal_or_a_word_only_one_path_has() {
        let candidates = ["crates/inf-alloc", "vendor/inf-alloc"];
        assert_eq!(choice("The first one.", &candidates), Some(0));
        assert_eq!(choice("the second", &candidates), Some(1));
        assert_eq!(choice("the last one", &candidates), Some(1));
        assert_eq!(choice("Last.", &candidates), Some(1));
        // "Last" picks only as an ordinal (review 3 hygiene).
        assert_eq!(choice("the crates one, not the last", &candidates), Some(0));
        assert_eq!(choice("the one I opened last week", &candidates), None);
        assert_eq!(choice("the vendor one", &candidates), Some(1));
        assert_eq!(choice("crates", &candidates), Some(0));
        assert_eq!(choice("inf-alloc", &candidates), None);
        assert_eq!(choice("the third", &candidates), None);
        assert_eq!(
            choice("how does the arena work in the crates folder?", &candidates),
            None
        );
    }

    #[test]
    fn the_tray_opens_and_closes_by_voice() {
        for phrase in SHOW_SOURCES_PHRASES {
            assert_eq!(tray_cue(phrase), Some(TrayCue::Show), "{phrase}");
        }
        for phrase in HIDE_SOURCES_PHRASES {
            assert_eq!(tray_cue(phrase), Some(TrayCue::Hide), "{phrase}");
        }
        assert_eq!(tray_cue("Show sources, please."), Some(TrayCue::Show));
        assert_eq!(tray_cue("show me the sources of the bug"), None);
    }
}
