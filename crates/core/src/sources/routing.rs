//! Where a `research` call goes while documents are shared (D-267). A document answers from its
//! digest only when the question points at it: its file's name or its digest's title written
//! out ("the layered store paper"), or "the paper", "this PDF", "the document". A question that
//! names the code ("our code", "this repository", a source file's path, a shared code folder's
//! name) is the code's, whatever else it says. Before, any word of five letters or more from a
//! document's name sent a question there, and with a code folder shared every question went to
//! the code first: "how does compaction work in this repository's storage layer" was answered
//! from a paper called storage-engines.pdf, and "what does the layered store paper say about
//! recovery" started a brief about the code folder (review 3, finding 4).

use super::digest::content_words;
use super::name_key;

/// Words that name the code itself.
const CODE_WORDS: &[&str] = &[
    "code",
    "codebase",
    "repo",
    "repos",
    "repository",
    "repositories",
    "crate",
    "crates",
];

/// Extensions that make a word a source file's name: a path names the code. Documents'
/// extensions (`md`, `txt`, `pdf`) aren't here: a shared Markdown file is a document.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "py", "rb", "java", "kt", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "php", "scala", "toml", "sql", "sh", "proto", "vue", "svelte", "zig",
];

/// Words that point at a document without naming it, after "the", "this" or "that".
const DOCUMENT_NOUNS: &[&str] = &["paper", "pdf", "document", "doc", "article"];

/// Determiners that make a document noun a reference ("the paper", not "a paper").
const DOCUMENT_POINTERS: &[&str] = &["the", "this", "that"];

/// A shared name shorter than this (in letters and digits) names nothing by itself: "a.pdf"
/// would match every question with an "a" in it.
pub const DOCUMENT_NAME_MIN_CHARS: usize = 4;

/// A shared document's names, as the question may say them.
#[derive(Clone, Copy, Debug)]
pub struct DocumentNames<'a> {
    /// Its file's name, with or without the extension.
    pub file: &'a str,
    /// Its digest's title; empty before the digest came.
    pub title: &'a str,
}

/// What a research question is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResearchTarget {
    /// The document at this index of the list given, named by its file or title, or the first
    /// one ("the paper").
    Document(usize),
    /// The code: the question names it, a repository or a path.
    Code,
    /// Neither: the caller decides (a shared code source, else the harness).
    Unnamed,
}

/// Where `question` goes. `documents` come in the order a bare "the paper" should pick them
/// (the one last read first); `code` are the shared code sources' titles.
#[must_use]
pub fn research_target(
    question: &str,
    documents: &[DocumentNames<'_>],
    code: &[&str],
) -> ResearchTarget {
    let words = content_words(question);
    if names_code(question, &words, code) {
        return ResearchTarget::Code;
    }
    let named = documents.iter().position(|document| {
        let stem = document
            .file
            .rsplit_once('.')
            .map_or(document.file, |(stem, _)| stem);
        says(&words, stem) || says(&words, document.title)
    });
    if let Some(index) = named {
        return ResearchTarget::Document(index);
    }
    let pointed = words.windows(2).any(|pair| {
        DOCUMENT_POINTERS.contains(&pair[0].as_str()) && DOCUMENT_NOUNS.contains(&pair[1].as_str())
    });
    if pointed && !documents.is_empty() {
        return ResearchTarget::Document(0);
    }
    ResearchTarget::Unnamed
}

/// Whether the question names the code: a code word, a source file's name or path, or a shared
/// code source's title.
fn names_code(question: &str, words: &[String], code: &[&str]) -> bool {
    if words.iter().any(|word| CODE_WORDS.contains(&word.as_str())) {
        return true;
    }
    let file = question.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| !ch.is_alphanumeric());
        token.rsplit_once('.').is_some_and(|(stem, extension)| {
            !stem.is_empty() && CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
    });
    if file {
        return true;
    }
    let key = name_key(question);
    code.iter().any(|title| {
        let title = name_key(title);
        title.len() >= DOCUMENT_NAME_MIN_CHARS && key.contains(&title)
    })
}

/// Whether `words` hold `name`'s words in a row ("layered-store" in "the layered store paper").
fn says(words: &[String], name: &str) -> bool {
    let name_words = content_words(name);
    let letters: usize = name_words.iter().map(String::len).sum();
    if name_words.is_empty() || letters < DOCUMENT_NAME_MIN_CHARS {
        return false;
    }
    words
        .windows(name_words.len())
        .any(|window| window == name_words.as_slice())
}

#[cfg(test)]
mod tests {
    use super::{DocumentNames, ResearchTarget, research_target};

    const PAPER: DocumentNames<'static> = DocumentNames {
        file: "layered-store.pdf",
        title: "Unbundled Transactions",
    };
    const STORAGE: DocumentNames<'static> = DocumentNames {
        file: "storage-engines.pdf",
        title: "",
    };

    type Case<'a> = (
        &'a str,
        &'a [DocumentNames<'a>],
        &'a [&'a str],
        ResearchTarget,
    );

    fn check(table: &[Case<'_>]) {
        for (question, documents, code, expected) in table {
            assert_eq!(
                research_target(question, documents, code),
                *expected,
                "{question}"
            );
        }
    }

    #[test]
    fn a_document_answers_only_when_the_question_points_at_it() {
        check(&[
            (
                "What does the layered store paper say about recovery?",
                &[PAPER],
                &["inf-alloc"],
                ResearchTarget::Document(0),
            ),
            (
                "How do unbundled transactions recover?",
                &[STORAGE, PAPER],
                &[],
                ResearchTarget::Document(1),
            ),
            (
                "What does the paper say about recovery?",
                &[PAPER],
                &[],
                ResearchTarget::Document(0),
            ),
            (
                "Summarize this PDF.",
                &[STORAGE],
                &[],
                ResearchTarget::Document(0),
            ),
            // A document's word alone is no reference.
            (
                "How does compaction work in the storage layer?",
                &[STORAGE],
                &[],
                ResearchTarget::Unnamed,
            ),
            (
                "Is there a paper on this?",
                &[PAPER],
                &[],
                ResearchTarget::Unnamed,
            ),
            (
                "What does the paper say?",
                &[],
                &[],
                ResearchTarget::Unnamed,
            ),
        ]);
    }

    #[test]
    fn the_code_wins_whatever_else_the_question_says() {
        check(&[
            (
                "How does compaction work in this repository's storage layer?",
                &[STORAGE],
                &[],
                ResearchTarget::Code,
            ),
            (
                "Does our code do what the layered store paper says?",
                &[PAPER],
                &[],
                ResearchTarget::Code,
            ),
            (
                "Where does src/arena.rs free a slot, per the paper?",
                &[PAPER],
                &[],
                ResearchTarget::Code,
            ),
            (
                "How does inf-alloc compare with the paper?",
                &[PAPER],
                &["inf-alloc"],
                ResearchTarget::Code,
            ),
            (
                "How does the arena work?",
                &[PAPER],
                &["inf-alloc"],
                ResearchTarget::Unnamed,
            ),
        ]);
    }

    #[test]
    fn a_short_name_names_nothing() {
        let tiny = DocumentNames {
            file: "a.pdf",
            title: "",
        };
        assert_eq!(
            research_target("What is a lock?", &[tiny], &[]),
            ResearchTarget::Unnamed
        );
    }
}
