//! Page citations in the talker's speech, and the passages a Doc card shows (D-257).
//!
//! The talker explains a document and says where: "on page 3", "Figure 1 (p. 3)", "pp. 4–5",
//! "Table 2". [`find_cites`] reads those out of a clause's text, each with the byte offset the
//! voice reaches it at; whether one names something read is the engine's question (law 6).
//! [`sentences`] and [`best_sentence`] cut a passage into the sentences a Doc card shows and pick
//! the one a spoken clause explains, by the words they share. All of it is pure and bounded.

use crate::limits::{
    CITE_JOIN_WORDS_MAX, CITES_PER_CLAUSE_MAX, DOC_CARD_SENTENCE_MAX_BYTES,
    DOC_SENTENCE_OVERLAP_MIN, DOCUMENT_LOCATION_MAX,
};
use crate::narration::tags::bounded;

/// A citation in speech: a page or a range, a figure, a table, or a figure with its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCite {
    /// Byte offset in the clause's text where the citation starts.
    pub at: usize,
    /// Pages `first..=last`.
    pub pages: Option<(u32, u32)>,
    pub figure: Option<u32>,
    pub table: Option<u32>,
}

/// What a word names, once its neighbours are known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Noun {
    Page,
    Pages,
    Figure,
    Table,
}

/// A word, its byte offset and its form without the punctuation around it.
struct Word<'a> {
    at: usize,
    text: &'a str,
}

fn words(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (true, Some(begin)) => {
                out.push(Word {
                    at: begin,
                    text: &text[begin..index],
                });
                start = None;
            }
            (false, None) => start = Some(index),
            (true, None) | (false, Some(_)) => {}
        }
    }
    if let Some(begin) = start {
        out.push(Word {
            at: begin,
            text: &text[begin..],
        });
    }
    out
}

/// The word without the brackets, quotes and punctuation around it ("(p." → "p.").
fn bare(word: &str) -> &str {
    word.trim_start_matches(['(', '[', '"', '\'', '“', '‘'])
        .trim_end_matches([')', ']', ',', ';', ':', '!', '?', '"', '\'', '”', '’'])
}

fn noun(word: &str) -> Option<Noun> {
    match bare(word).to_lowercase().as_str() {
        "p." | "p" | "page" => Some(Noun::Page),
        "pp." | "pp" | "pages" => Some(Noun::Pages),
        "figure" | "fig." | "fig" => Some(Noun::Figure),
        "table" => Some(Noun::Table),
        _ => None,
    }
}

/// A number as written or said: digits, or one to twenty in words.
fn number(word: &str) -> Option<u32> {
    let word = bare(word).trim_end_matches('.');
    if !word.is_empty() && word.len() <= 7 && word.bytes().all(|byte| byte.is_ascii_digit()) {
        return word
            .parse()
            .ok()
            .filter(|number| (1..=DOCUMENT_LOCATION_MAX).contains(number));
    }
    const SPOKEN: [&str; 20] = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    let lower = word.to_lowercase();
    (1u32..)
        .zip(SPOKEN)
        .find_map(|(value, spoken)| (spoken == lower).then_some(value))
}

/// `3–4` or `3-4` in one word.
fn joined_range(word: &str) -> Option<(u32, u32)> {
    let word = bare(word).trim_end_matches('.');
    let (first, last) = word.split_once(['–', '-', '—'])?;
    let (first, last) = (number(first)?, number(last)?);
    (first <= last).then_some((first, last))
}

/// Citations in `text`, in order, at most [`CITES_PER_CLAUSE_MAX`].
#[must_use]
pub fn find_cites(text: &str) -> Vec<PageCite> {
    let words = words(text);
    // Each citation with the words it spans, `[begin, end)`.
    let mut found: Vec<(usize, usize, PageCite)> = Vec::new();
    let mut index = 0;
    while index < words.len() && found.len() < CITES_PER_CLAUSE_MAX * 2 {
        let Some((cite, used)) = cite_at(&words, index) else {
            index += 1;
            continue;
        };
        found.push((index, index + used, cite));
        index += used;
    }
    let mut merged: Vec<(usize, PageCite)> = Vec::new();
    for (begin, end, cite) in found {
        let joined = merged.last_mut().filter(|(last_end, last)| {
            let gap = words.get(*last_end..begin).unwrap_or_default();
            gap.len() <= CITE_JOIN_WORDS_MAX
                && gap.iter().all(|word| JOINERS.contains(&bare(word.text)))
                && joins(last, &cite)
        });
        match joined {
            Some((last_end, last)) => {
                last.pages = last.pages.or(cite.pages);
                last.figure = last.figure.or(cite.figure);
                last.table = last.table.or(cite.table);
                *last_end = end;
            }
            None => merged.push((end, cite)),
        }
    }
    merged
        .into_iter()
        .map(|(_, cite)| cite)
        .take(CITES_PER_CLAUSE_MAX)
        .collect()
}

/// Words that may stand between a figure and its page in one citation ("Figure 1 on page 3").
const JOINERS: &[&str] = &["on", "in", "at", "of", "from", "see", ""];

/// A page joins a figure or a table (either order); two of a kind never join.
fn joins(first: &PageCite, second: &PageCite) -> bool {
    let has_part = |cite: &PageCite| cite.figure.is_some() || cite.table.is_some();
    (has_part(first) && second.pages.is_some() && first.pages.is_none() && !has_part(second))
        || (first.pages.is_some() && has_part(second) && second.pages.is_none() && !has_part(first))
}

/// The citation starting at word `index`, and how many words it took.
fn cite_at(words: &[Word<'_>], index: usize) -> Option<(PageCite, usize)> {
    let word = words.get(index)?;
    let at = word.at;
    let blank = PageCite {
        at,
        pages: None,
        figure: None,
        table: None,
    };
    // "p.3", "pp.3-4": the number joined to its noun.
    let lower = bare(word.text).to_lowercase();
    for prefix in ["pp.", "p."] {
        if let Some(rest) = lower.strip_prefix(prefix).filter(|rest| !rest.is_empty()) {
            let pages = joined_range(rest).or_else(|| number(rest).map(|page| (page, page)))?;
            return Some((
                PageCite {
                    pages: Some(pages),
                    ..blank
                },
                1,
            ));
        }
    }
    let noun = noun(word.text)?;
    let next = words.get(index + 1)?;
    match noun {
        Noun::Figure | Noun::Table => {
            let value = number(next.text)?;
            let cite = if noun == Noun::Figure {
                PageCite {
                    figure: Some(value),
                    ..blank
                }
            } else {
                PageCite {
                    table: Some(value),
                    ..blank
                }
            };
            Some((cite, 2))
        }
        Noun::Page | Noun::Pages => {
            if let Some(pages) = joined_range(next.text) {
                return Some((
                    PageCite {
                        pages: Some(pages),
                        ..blank
                    },
                    2,
                ));
            }
            let first = number(next.text)?;
            // "pages 3 to 4", "pages 3 and 4".
            let last = (noun == Noun::Pages)
                .then(|| {
                    let joiner = words.get(index + 2).map(|word| bare(word.text))?;
                    matches!(joiner, "to" | "and" | "through" | "–" | "-")
                        .then(|| words.get(index + 3).and_then(|word| number(word.text)))
                        .flatten()
                })
                .flatten()
                .filter(|last| *last >= first);
            let used = if last.is_some() { 4 } else { 2 };
            Some((
                PageCite {
                    pages: Some((first, last.unwrap_or(first))),
                    ..blank
                },
                used,
            ))
        }
    }
}

/// Abbreviations a sentence never ends on ("Fig. 3", "p. 4", "e.g. this").
const ABBREVIATIONS: &[&str] = &[
    "fig.", "figs.", "p.", "pp.", "e.g.", "i.e.", "et al.", "al.", "vs.", "cf.", "eq.", "sec.",
    "no.", "dr.", "mr.", "ms.",
];

/// `text` as sentences, each held to [`DOC_CARD_SENTENCE_MAX_BYTES`], at most `max`.
#[must_use]
pub fn sentences(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        let ends = word.ends_with(['.', '?', '!'])
            || word.ends_with(".)")
            || word.ends_with(".\"")
            || word.ends_with(".”");
        let abbreviation = ABBREVIATIONS
            .iter()
            .any(|known| word.to_lowercase().ends_with(known) && word.len() <= known.len() + 1);
        if ends && !abbreviation {
            out.push(bounded(&current, DOC_CARD_SENTENCE_MAX_BYTES));
            current.clear();
            if out.len() >= max {
                return out;
            }
        }
    }
    if !current.is_empty() && out.len() < max {
        out.push(bounded(&current, DOC_CARD_SENTENCE_MAX_BYTES));
    }
    out
}

/// Lowercase words of four letters or more: what a clause and a sentence must share.
fn key_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect();
    words.sort();
    words.dedup();
    words
}

/// The sentence `clause` explains: the one sharing the most key words with it, at least
/// [`DOC_SENTENCE_OVERLAP_MIN`]; the first such on a tie.
#[must_use]
pub fn best_sentence(sentences: &[String], clause: &str) -> Option<usize> {
    let spoken = key_words(clause);
    if spoken.is_empty() {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for (index, sentence) in sentences.iter().enumerate() {
        let shared = key_words(sentence)
            .iter()
            .filter(|word| spoken.binary_search(word).is_ok())
            .count();
        if shared >= DOC_SENTENCE_OVERLAP_MIN && best.is_none_or(|(_, most)| shared > most) {
            best = Some((index, shared));
        }
    }
    best.map(|(index, _)| index)
}

/// The window of at most `max` sentences of `text` around the one `clause` explains (two before
/// it when there are), and that sentence's index in the window.
#[must_use]
pub fn passage(text: &str, clause: &str, max: usize) -> (Vec<String>, Option<usize>) {
    let all = sentences(text, usize::MAX >> 1);
    let Some(best) = best_sentence(&all, clause) else {
        return (all.into_iter().take(max).collect(), None);
    };
    let start = best.saturating_sub(2).min(all.len().saturating_sub(max));
    let window: Vec<String> = all.into_iter().skip(start).take(max).collect();
    (window, Some(best - start))
}

#[cfg(test)]
mod tests {
    use super::{PageCite, best_sentence, find_cites, passage, sentences};

    fn cite(
        at: usize,
        pages: Option<(u32, u32)>,
        figure: Option<u32>,
        table: Option<u32>,
    ) -> PageCite {
        PageCite {
            at,
            pages,
            figure,
            table,
        }
    }

    #[test]
    fn citations_read_as_people_say_them() {
        assert_eq!(
            find_cites("As Figure 1 (p. 3) shows, clients ask first."),
            [cite(3, Some((3, 3)), Some(1), None)]
        );
        assert_eq!(
            find_cites("On page 3 the proxies answer."),
            [cite(3, Some((3, 3)), None, None)]
        );
        let text = "See pp. 4–5 and Table two.";
        assert_eq!(
            find_cites(text),
            [
                cite(4, Some((4, 5)), None, None),
                cite(text.find("Table").unwrap_or(0), None, None, Some(2))
            ]
        );
        let text = "pages 6 to 7, then p.9";
        assert_eq!(
            find_cites(text),
            [
                cite(0, Some((6, 7)), None, None),
                cite(text.find("p.9").unwrap_or(0), Some((9, 9)), None, None)
            ]
        );
        assert_eq!(
            find_cites("Figure one on page three"),
            [cite(0, Some((3, 3)), Some(1), None)]
        );
        for nothing in [
            "Turn the page and read on.",
            "Figure it out.",
            "A table of contents.",
            "p. 0 and pp. 9–3",
            "",
        ] {
            assert_eq!(find_cites(nothing), [], "{nothing}");
        }
        let many = "Figure 1, Figure 2, Figure 3, Figure 4, Figure 5, Figure 6.";
        assert_eq!(find_cites(many).len(), 4, "bounded per clause");
    }

    #[test]
    fn passages_split_into_sentences_around_the_one_explained() {
        let text = "FoundationDB unbundles its roles. See Fig. 1 for the parts. A client \
                    first asks the proxies for a read version. The sequencer hands it out. \
                    Resolvers check conflicts, e.g. overlapping writes.";
        let all = sentences(text, 8);
        assert_eq!(all.len(), 5, "{all:?}");
        assert_eq!(all[1], "See Fig. 1 for the parts.");
        assert_eq!(
            best_sentence(&all, "A client first asks the proxies for a read version."),
            Some(2)
        );
        assert_eq!(best_sentence(&all, "It is fast."), None);
        let (window, at) = passage(text, "the sequencer hands out versions", 3);
        assert_eq!(window.len(), 3);
        assert_eq!(
            window.get(at.unwrap_or(9)).map(String::as_str),
            Some("The sequencer hands it out.")
        );
    }
}
