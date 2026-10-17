//! Document digests (D-186, D-254): what BoardAI knows of a shared document, in one fixed
//! Markdown shape, parsed here into a typed, bounded [`Digest`].
//!
//! ```text
//! # Digest: <title>
//! Read: <n> pages; figures by <reader>            (the runtime's line, never the model's)
//! ## Sections   ### <title> (pp. a–b)   <summary>
//! ## Figures    ### Figure <n> (p. <page>): <caption>   <what it shows>   <diagram title="Figure n">…</diagram>
//! ## Tables     ### Table <n> (p. <page>): <caption>   | a | b |
//! ## Claims     - <claim> (p. <page>)
//! ```
//!
//! A digest comes from a model that read the document's file, from a coding harness, or from
//! the text BoardAI extracted itself; all of it quotes the document, so it is untrusted data
//! (D-107). The parser keeps what has the shape and drops and counts the rest. A figure's
//! `<diagram>` block is kept raw and whole, in the narration vocabulary (D-234), so it can be
//! staged; until then the talker gets it as a reference. The canonical form, [`Digest::to_markdown`],
//! is what the cache keeps and what parses back to itself.

mod parse;
mod request;

use std::fmt::Write as _;

use crate::limits::{
    DIGEST_CLAIM_MAX_BYTES, DIGEST_READER_MAX_BYTES, DOCUMENT_DIGEST_MAX_BYTES,
    DOCUMENT_LOCATION_MAX,
};
use crate::narration::tags::bounded;
use crate::sources::SourceState;

pub use parse::parse;
pub use request::{DigestScope, request_prompt};

/// Where something is in a document: pages of a PDF, lines of Markdown or text. Both are
/// 1-based and inclusive, as people write them (`pp. 3–4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locator {
    Pages { first: u32, last: u32 },
    Lines { first: u32, last: u32 },
}

impl Locator {
    /// `p. 3`, `pp. 3–4`, `page 3`, `pages 3-4`, `l. 42`, `ll. 40–58`, `line 42`, `lines 40 to 58`.
    /// The numbers are at least 1, in order and at most [`DOCUMENT_LOCATION_MAX`].
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_matches(|ch| ch == '(' || ch == ')').trim();
        let lower = text.to_ascii_lowercase();
        let (pages, rest) = [
            ("pages", true),
            ("page", true),
            ("pp.", true),
            ("pp", true),
            ("p.", true),
            ("lines", false),
            ("line", false),
            ("ll.", false),
            ("l.", false),
        ]
        .iter()
        .find_map(|(prefix, pages)| lower.strip_prefix(prefix).map(|rest| (*pages, rest)))?;
        let (first, last) = number_range(rest)?;
        Some(if pages {
            Self::Pages { first, last }
        } else {
            Self::Lines { first, last }
        })
    }

    /// As the canonical form and the Doc card's chip write it.
    #[must_use]
    pub fn label(&self) -> String {
        match *self {
            Self::Pages { first, last } if first == last => format!("p. {first}"),
            Self::Pages { first, last } => format!("pp. {first}–{last}"),
            Self::Lines { first, last } if first == last => format!("l. {first}"),
            Self::Lines { first, last } => format!("ll. {first}–{last}"),
        }
    }

    /// Whether it names page `page` of a PDF.
    #[must_use]
    pub const fn covers_page(&self, page: u32) -> bool {
        match *self {
            Self::Pages { first, last } => first <= page && page <= last,
            Self::Lines { .. } => false,
        }
    }

    /// Its first page, for a PDF.
    #[must_use]
    pub const fn first_page(&self) -> Option<u32> {
        match *self {
            Self::Pages { first, .. } => Some(first),
            Self::Lines { .. } => None,
        }
    }
}

/// `3`, `3–4`, `3-4`, `3 to 4`: the numbers of a locator.
fn number_range(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let (first, last) = text
        .split_once(['–', '-', '—'])
        .or_else(|| text.split_once(" to "))
        .unwrap_or((text, text));
    // "p. 1–p. 2", as models write it: the second number repeats the prefix.
    let last = last
        .trim()
        .trim_start_matches(|ch: char| ch.is_ascii_alphabetic() || ch == '.')
        .trim();
    let number = |digits: &str| {
        let digits = digits.trim();
        (!digits.is_empty() && digits.len() <= 7 && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.parse::<u32>().ok())
            .flatten()
            .filter(|number| (1..=DOCUMENT_LOCATION_MAX).contains(number))
    };
    let (first, last) = (number(first)?, number(last)?);
    (first <= last).then_some((first, last))
}

/// How much of the document was read, and by whom: the runtime's `Read:` line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DigestRead {
    pub extent: Option<Extent>,
    /// Its pages are images (a scan): any text in the digest came from a model that read them.
    pub scanned: bool,
    /// Who read the figures ("Gemini 3.8 Flash", "Claude Code"); `None`: text only.
    pub figures_by: Option<String>,
}

/// The document's length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    Pages(u32),
    Lines(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestSection {
    pub title: String,
    pub at: Option<Locator>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestFigure {
    pub number: u32,
    pub at: Option<Locator>,
    pub caption: String,
    /// What it shows, in words; empty when nothing read it (no model that reads files).
    pub shows: String,
    /// Its `<diagram>` block, raw and whole (D-234).
    pub diagram: Option<String>,
}

impl DigestFigure {
    /// A model described it.
    #[must_use]
    pub fn is_read(&self) -> bool {
        !self.shows.is_empty() || self.diagram.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestTable {
    pub number: u32,
    pub at: Option<Locator>,
    pub caption: String,
    /// Its rows as Markdown (`| a | b |`), header and divider included.
    pub rows: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestClaim {
    pub text: String,
    pub at: Option<Locator>,
}

/// What the parser dropped; the tray counts dropped figures as unread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DigestDropped {
    pub sections: u32,
    pub figures: u32,
    pub tables: u32,
    pub rows: u32,
    pub claims: u32,
    /// `<diagram>` blocks past their bound.
    pub diagrams: u32,
    /// Locators past the document's own length (its `Read:` line): the part stays, unplaced.
    pub locators: u32,
    /// Lines that fit no section's shape.
    pub lines: u32,
    /// Bytes past the digest's or a part's bound.
    pub truncated: bool,
}

impl DigestDropped {
    #[must_use]
    pub fn total(&self) -> u32 {
        self.sections
            + self.figures
            + self.tables
            + self.rows
            + self.claims
            + self.diagrams
            + self.locators
            + self.lines
            + u32::from(self.truncated)
    }
}

/// A parsed, bounded digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digest {
    pub title: String,
    pub read: DigestRead,
    pub sections: Vec<DigestSection>,
    pub figures: Vec<DigestFigure>,
    pub tables: Vec<DigestTable>,
    pub claims: Vec<DigestClaim>,
    pub dropped: DigestDropped,
}

impl Digest {
    /// Something to explain from.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.sections.is_empty()
            || !self.figures.is_empty()
            || !self.tables.is_empty()
            || !self.claims.is_empty()
    }

    #[must_use]
    pub fn figure(&self, number: u32) -> Option<&DigestFigure> {
        self.figures.iter().find(|figure| figure.number == number)
    }

    #[must_use]
    pub fn table(&self, number: u32) -> Option<&DigestTable> {
        self.tables.iter().find(|table| table.number == number)
    }

    /// Whether anything in the digest is on page `page` (law 6: a page citation checks out
    /// against the digest).
    #[must_use]
    pub fn covers_page(&self, page: u32) -> bool {
        let covers = |at: &Option<Locator>| at.is_some_and(|at| at.covers_page(page));
        self.sections.iter().any(|section| covers(&section.at))
            || self.figures.iter().any(|figure| covers(&figure.at))
            || self.tables.iter().any(|table| covers(&table.at))
            || self.claims.iter().any(|claim| covers(&claim.at))
    }

    /// The section whose title the words name best (at least one word of three letters or
    /// more in common), for "the section on recovery".
    #[must_use]
    pub fn section_named(&self, words: &str) -> Option<&DigestSection> {
        let asked = content_words(words);
        self.sections
            .iter()
            .map(|section| {
                let title = content_words(&section.title);
                let shared = asked.iter().filter(|word| title.contains(word)).count();
                (shared, section)
            })
            .filter(|(shared, _)| *shared > 0)
            .max_by_key(|(shared, section)| (*shared, std::cmp::Reverse(section.title.len())))
            .map(|(_, section)| section)
    }

    /// Figures counted for the tray: those kept and those dropped past the bound.
    #[must_use]
    pub fn figures_total(&self) -> u32 {
        u32::try_from(self.figures.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.dropped.figures)
    }

    #[must_use]
    pub fn figures_read(&self) -> u32 {
        let read = self
            .figures
            .iter()
            .filter(|figure| figure.is_read())
            .count();
        u32::try_from(read).unwrap_or(u32::MAX)
    }

    /// What the tray says about the document once its digest is known (artboard 14): ready when
    /// every figure was read, partial with the reason otherwise. A figure is never lost
    /// silently (D-186).
    #[must_use]
    pub fn tray_state(&self) -> SourceState {
        let (total, read) = (self.figures_total(), self.figures_read());
        let tables = self.tables.len();
        let by = self.read.figures_by.as_deref();
        let noun = |count: u32, one: &str, many: &str| {
            if count == 1 {
                format!("1 {one}")
            } else {
                format!("{count} {many}")
            }
        };
        let images = matches!(self.read.extent, Some(Extent::Lines(_)));
        let (figure_one, figure_many) = if images {
            ("image", "images")
        } else {
            ("figure", "figures")
        };
        if let Some(reason) = self.partial_reason((figure_one, figure_many)) {
            return SourceState::Partial {
                reason: bounded(&reason, crate::limits::SOURCE_REASON_MAX_BYTES),
            };
        }
        let detail = match self.read.extent {
            Some(Extent::Lines(lines)) => {
                let mut detail = format!("Digest ready · {}", noun(lines, "line", "lines"));
                if total > 0 {
                    let _ = write!(detail, ", {}", noun(total, figure_one, figure_many));
                }
                detail
            }
            Some(Extent::Pages(_)) | None => {
                let mut read_parts = vec!["text".to_owned()];
                if tables > 0 {
                    read_parts.push(noun(
                        u32::try_from(tables).unwrap_or(u32::MAX),
                        "table",
                        "tables",
                    ));
                }
                if total > 0 {
                    read_parts.push(format!("{read} of {total} {figure_many}"));
                }
                let listed = join_and(&read_parts);
                match self.read.extent {
                    Some(Extent::Pages(pages)) => {
                        format!("{} · {listed} read", noun(pages, "page", "pages"))
                    }
                    Some(Extent::Lines(_)) | None => format!("{listed} read"),
                }
            }
        };
        let detail = match by {
            Some(by) if total > 0 => format!("{detail} · {figure_many} by {by}"),
            Some(_) | None => detail,
        };
        SourceState::Ready {
            detail: bounded(&detail, crate::limits::SOURCE_REASON_MAX_BYTES),
        }
    }

    /// Why the document is only partly read: a scan with nothing to read it, or figures no
    /// model read. `None` when every figure was read.
    fn partial_reason(&self, (one, many): (&str, &str)) -> Option<String> {
        let (total, read) = (self.figures_total(), self.figures_read());
        let unread = total.saturating_sub(read);
        let by = self.read.figures_by.as_deref();
        if self.read.scanned && by.is_none() {
            return Some("Scanned pages · its text needs a model that reads files".to_owned());
        }
        if unread == 0 {
            return None;
        }
        let count = |count: u32| {
            if count == 1 {
                format!("1 {one}")
            } else {
                format!("{count} {many}")
            }
        };
        Some(match by {
            None => format!(
                "Text read · {} {} a model that reads files",
                count(unread),
                if unread == 1 { "needs" } else { "need" }
            ),
            Some(by) => format!("{read} of {} read by {by} · {unread} unread", count(total)),
        })
    }

    /// The digest in its canonical, bounded Markdown: what the cache keeps and the talker reads.
    /// Parsing it again gives the same digest, with nothing dropped.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# Digest: {}\n", self.title);
        if let Some(line) = self.read_line() {
            let _ = writeln!(out, "{line}");
        }
        if !self.sections.is_empty() {
            out.push_str("\n## Sections\n");
            for section in &self.sections {
                let _ = writeln!(out, "### {}{}", section.title, at_suffix(section.at));
                if !section.summary.is_empty() {
                    let _ = writeln!(out, "{}", section.summary);
                }
            }
        }
        if !self.figures.is_empty() {
            out.push_str("\n## Figures\n");
            for figure in &self.figures {
                render_figure(&mut out, figure);
            }
        }
        if !self.tables.is_empty() {
            out.push_str("\n## Tables\n");
            for table in &self.tables {
                let _ = writeln!(
                    out,
                    "### Table {}{}{}",
                    table.number,
                    at_suffix(table.at),
                    caption_suffix(&table.caption)
                );
                for row in &table.rows {
                    let _ = writeln!(out, "{row}");
                }
            }
        }
        if !self.claims.is_empty() {
            out.push_str("\n## Claims\n");
            for claim in &self.claims {
                let _ = writeln!(out, "- {}{}", claim.text, at_suffix(claim.at));
            }
        }
        out
    }

    /// The `Read:` line, when anything is known of how it was read.
    fn read_line(&self) -> Option<String> {
        let read = &self.read;
        let mut parts = Vec::new();
        match read.extent {
            Some(Extent::Pages(1)) => parts.push("1 page".to_owned()),
            Some(Extent::Pages(pages)) => parts.push(format!("{pages} pages")),
            Some(Extent::Lines(1)) => parts.push("1 line".to_owned()),
            Some(Extent::Lines(lines)) => parts.push(format!("{lines} lines")),
            None => {}
        }
        if read.scanned {
            parts.push("scanned".to_owned());
        }
        match &read.figures_by {
            Some(by) => parts.push(format!("figures by {by}")),
            None if !parts.is_empty() => parts.push("text only".to_owned()),
            None => {}
        }
        (!parts.is_empty()).then(|| format!("Read: {}", parts.join("; ")))
    }

    /// Several digests of one document, read a page range at a time, as one: the first title,
    /// every part in order, a figure or table number only once (its first, read one preferred).
    /// The result is held to the digest's bounds by parsing its canonical form.
    #[must_use]
    pub fn merge(parts: Vec<Self>, read: DigestRead) -> Self {
        let mut merged = Self {
            read,
            ..Self::default()
        };
        for part in parts {
            if merged.title.is_empty() {
                merged.title = part.title;
            }
            merged.sections.extend(part.sections);
            for figure in part.figures {
                match merged
                    .figures
                    .iter_mut()
                    .find(|known| known.number == figure.number)
                {
                    Some(known) if !known.is_read() && figure.is_read() => *known = figure,
                    Some(_) => {}
                    None => merged.figures.push(figure),
                }
            }
            for table in part.tables {
                if merged.table(table.number).is_none() {
                    merged.tables.push(table);
                }
            }
            merged.claims.extend(part.claims);
        }
        parse(&merged.to_markdown())
    }
}

fn render_figure(out: &mut String, figure: &DigestFigure) {
    let _ = writeln!(
        out,
        "### Figure {}{}{}",
        figure.number,
        at_suffix(figure.at),
        caption_suffix(&figure.caption)
    );
    if !figure.shows.is_empty() {
        let _ = writeln!(out, "{}", figure.shows);
    }
    if let Some(diagram) = &figure.diagram {
        let _ = writeln!(out, "{diagram}");
    }
}

fn at_suffix(at: Option<Locator>) -> String {
    at.map_or_else(String::new, |at| format!(" ({})", at.label()))
}

fn caption_suffix(caption: &str) -> String {
    if caption.is_empty() {
        String::new()
    } else {
        format!(": {caption}")
    }
}

/// "a", "a and b", "a, b and c".
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Lowercase words of three letters or more, for matching titles and sentences.
#[must_use]
pub fn content_words(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// A claim or a reader's name, held to its bound.
pub(crate) fn bounded_claim(text: &str) -> String {
    bounded(text.trim(), DIGEST_CLAIM_MAX_BYTES)
}

/// Who read the figures: named in BoardAI's notes and on the Doc card, so plain text (D-266).
pub(crate) fn bounded_reader(text: &str) -> String {
    crate::untrusted::PlainText::parse(text, DIGEST_READER_MAX_BYTES).into_string()
}

/// The digest's whole bound, for callers that cut before parsing.
pub const DIGEST_MAX_BYTES: usize = DOCUMENT_DIGEST_MAX_BYTES;

#[cfg(test)]
mod tests;
