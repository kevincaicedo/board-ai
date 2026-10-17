//! The digest parser: one line at a time through an explicit section state, never recursive,
//! and bounded at every step (the whole text, each line, each part, each list). What doesn't fit
//! the shape is dropped and counted in [`DigestDropped`](super::DigestDropped).
//!
//! A digest arrives whole (a model's answer, a harness's, or BoardAI's own), so there is no
//! stream to split.

use super::{
    Digest, DigestClaim, DigestFigure, DigestRead, DigestSection, DigestTable, Extent, Locator,
    bounded_claim, bounded_reader,
};
use crate::limits::{
    DIGEST_CAPTION_MAX_BYTES, DIGEST_CLAIMS_MAX, DIGEST_DIAGRAM_MAX_BYTES,
    DIGEST_FIGURE_SHOWS_MAX_BYTES, DIGEST_FIGURES_MAX, DIGEST_LINE_MAX_BYTES, DIGEST_SECTIONS_MAX,
    DIGEST_SUMMARY_MAX_BYTES, DIGEST_TABLE_ROW_MAX_BYTES, DIGEST_TABLE_ROWS_MAX, DIGEST_TABLES_MAX,
    DOCUMENT_DIGEST_MAX_BYTES, DOCUMENT_LOCATION_MAX,
};
use crate::untrusted::PlainText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// Before the first `##`: the title, the `Read:` line, and chatter that is dropped.
    Preamble,
    Sections,
    Figures,
    Tables,
    Claims,
    /// A `##` heading the shape doesn't have: its lines are dropped.
    Unknown,
}

/// What the current `###` heading opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    Nothing,
    Section,
    Figure,
    Table,
    /// A heading that was refused (past a bound, or malformed): its lines go with it.
    Skipping,
}

/// A `<diagram>` block being read: kept for the current figure, or read to its end and dropped
/// (outside a figure, a figure's second block, past its bound).
enum Block {
    Keep(String),
    Skip,
}

struct Parser {
    digest: Digest,
    section: Section,
    open: Open,
    block: Option<Block>,
}

/// Parses a digest. Never fails: an answer with nothing usable gives a digest whose
/// [`Digest::is_usable`] is false, with the drops counted.
#[must_use]
pub fn parse(text: &str) -> Digest {
    let mut parser = Parser {
        digest: Digest::default(),
        section: Section::Preamble,
        open: Open::Nothing,
        block: None,
    };
    let (text, cut) = cut_at_line(text, DOCUMENT_DIGEST_MAX_BYTES);
    parser.digest.dropped.truncated |= cut;
    for line in text.lines() {
        parser.line(line);
    }
    parser.finish()
}

/// `line` cut at a character within `max_bytes`.
fn cut_at_char(line: &str, max_bytes: usize) -> (&str, bool) {
    if line.len() <= max_bytes {
        return (line, false);
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

/// `text` cut at the last line end within `max_bytes`.
fn cut_at_line(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').unwrap_or(0);
    (&text[..end], true)
}

impl Parser {
    fn line(&mut self, line: &str) {
        let (line, cut) = cut_at_char(line, DIGEST_LINE_MAX_BYTES);
        self.digest.dropped.truncated |= cut;
        let trimmed = line.trim();
        if self.block.is_some() {
            if !trimmed.starts_with("##") {
                self.block_line(line);
                return;
            }
            // A heading ends a block that lost its closing tag, so it can't swallow the rest.
            self.unclosed_block();
        }
        if trimmed.is_empty() {
            return;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            self.section = section_named(heading);
            self.open = Open::Nothing;
            if self.section == Section::Unknown {
                self.digest.dropped.lines += 1;
            }
            return;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            self.subheading(heading.trim());
            return;
        }
        if self.open == Open::Skipping {
            return;
        }
        if trimmed.starts_with("<diagram") {
            self.open_block(trimmed);
            return;
        }
        self.content(trimmed);
    }

    fn content(&mut self, line: &str) {
        match (self.section, self.open) {
            (Section::Preamble, _) => self.preamble(line),
            (Section::Sections, Open::Section) => {
                if let Some(section) = self.digest.sections.last_mut()
                    && !append(&mut section.summary, line, DIGEST_SUMMARY_MAX_BYTES)
                {
                    self.digest.dropped.truncated = true;
                }
            }
            (Section::Figures, Open::Figure) => {
                if let Some(figure) = self.digest.figures.last_mut()
                    && !append(&mut figure.shows, line, DIGEST_FIGURE_SHOWS_MAX_BYTES)
                {
                    self.digest.dropped.truncated = true;
                }
            }
            (Section::Tables, Open::Table) => self.table_row(line),
            (Section::Claims, _) => self.claim(line),
            (
                Section::Sections | Section::Figures | Section::Tables | Section::Unknown,
                Open::Nothing | Open::Section | Open::Figure | Open::Table | Open::Skipping,
            ) => self.digest.dropped.lines += 1,
        }
    }

    fn preamble(&mut self, line: &str) {
        if let Some(title) = line.strip_prefix("# ") {
            if self.digest.title.is_empty() {
                let title = strip_label(title.trim(), "digest:");
                self.digest.title = plain(title, DIGEST_CAPTION_MAX_BYTES);
            } else {
                self.digest.dropped.lines += 1;
            }
            return;
        }
        match strip_prefix_ignoring_case(line, "read:") {
            Some(read) if self.digest.read == DigestRead::default() => {
                self.digest.read = read_line(read);
            }
            Some(_) | None => self.digest.dropped.lines += 1,
        }
    }

    fn subheading(&mut self, heading: &str) {
        self.open = Open::Skipping;
        match self.section {
            Section::Sections => self.section_heading(heading),
            Section::Figures => self.figure_heading(heading),
            Section::Tables => self.table_heading(heading),
            Section::Preamble | Section::Claims | Section::Unknown => {
                self.digest.dropped.lines += 1;
            }
        }
    }

    fn section_heading(&mut self, heading: &str) {
        if self.digest.sections.len() >= DIGEST_SECTIONS_MAX {
            self.digest.dropped.sections += 1;
            return;
        }
        let (title, at) = trailing_locator(heading);
        let title = plain(title, DIGEST_CAPTION_MAX_BYTES);
        if title.is_empty() {
            self.digest.dropped.sections += 1;
            return;
        }
        self.digest.sections.push(DigestSection {
            title,
            at,
            summary: String::new(),
        });
        self.open = Open::Section;
    }

    fn figure_heading(&mut self, heading: &str) {
        let Some((number, at, caption)) = numbered_heading(heading, &["figure", "fig.", "fig"])
        else {
            self.digest.dropped.figures += 1;
            return;
        };
        if self.digest.figures.len() >= DIGEST_FIGURES_MAX || self.digest.figure(number).is_some() {
            self.digest.dropped.figures += 1;
            return;
        }
        self.digest.figures.push(DigestFigure {
            number,
            at,
            caption,
            shows: String::new(),
            diagram: None,
        });
        self.open = Open::Figure;
    }

    fn table_heading(&mut self, heading: &str) {
        let Some((number, at, caption)) = numbered_heading(heading, &["table"]) else {
            self.digest.dropped.tables += 1;
            return;
        };
        if self.digest.tables.len() >= DIGEST_TABLES_MAX || self.digest.table(number).is_some() {
            self.digest.dropped.tables += 1;
            return;
        }
        self.digest.tables.push(DigestTable {
            number,
            at,
            caption,
            rows: Vec::new(),
        });
        self.open = Open::Table;
    }

    fn table_row(&mut self, line: &str) {
        let Some(table) = self.digest.tables.last_mut() else {
            self.digest.dropped.lines += 1;
            return;
        };
        if !line.starts_with('|') {
            self.digest.dropped.lines += 1;
            return;
        }
        if table.rows.len() >= DIGEST_TABLE_ROWS_MAX {
            self.digest.dropped.rows += 1;
            return;
        }
        let (row, cut) = cut_at_char(line, DIGEST_TABLE_ROW_MAX_BYTES);
        self.digest.dropped.truncated |= cut;
        table.rows.push(row.trim_end().to_owned());
    }

    fn claim(&mut self, line: &str) {
        let Some(text) = bullet(line) else {
            self.digest.dropped.lines += 1;
            return;
        };
        if self.digest.claims.len() >= DIGEST_CLAIMS_MAX {
            self.digest.dropped.claims += 1;
            return;
        }
        let (text, at) = trailing_locator(text);
        let text = bounded_claim(text);
        if text.is_empty() {
            self.digest.dropped.claims += 1;
            return;
        }
        self.digest.claims.push(DigestClaim { text, at });
    }

    /// `<diagram …>` opens a block under the current figure; anywhere else it is dropped.
    fn open_block(&mut self, line: &str) {
        let has_room = self
            .digest
            .figures
            .last()
            .is_some_and(|figure| figure.diagram.is_none());
        self.block = if self.open == Open::Figure && has_room {
            Some(Block::Keep(String::new()))
        } else {
            self.digest.dropped.diagrams += 1;
            Some(Block::Skip)
        };
        self.block_line(line);
    }

    fn block_line(&mut self, line: &str) {
        let closes = line.contains("</diagram>");
        match self.block.as_mut() {
            Some(Block::Keep(text)) if text.len() + line.len() < DIGEST_DIAGRAM_MAX_BYTES => {
                text.push_str(line);
                text.push('\n');
            }
            Some(Block::Keep(_)) => {
                // Past the bound the block can't be staged whole: it is dropped, never cut.
                self.digest.dropped.diagrams += 1;
                self.block = Some(Block::Skip);
            }
            Some(Block::Skip) | None => {}
        }
        if !closes {
            return;
        }
        if let Some(Block::Keep(text)) = self.block.take()
            && let Some(figure) = self.digest.figures.last_mut()
        {
            figure.diagram = Some(text.trim_end().to_owned());
        }
    }

    /// A block that lost its end can't be staged: it is dropped.
    fn unclosed_block(&mut self) {
        if let Some(Block::Keep(_)) = self.block.take() {
            self.digest.dropped.diagrams += 1;
            self.digest.dropped.truncated = true;
        }
    }

    fn finish(mut self) -> Digest {
        self.unclosed_block();
        drop_locators_past_the_end(&mut self.digest);
        self.digest
    }
}

/// A title or a caption: the document's words, which BoardAI's own notes and the Doc card may
/// name, parsed into plain text where they enter (D-266).
fn plain(text: &str, max_bytes: usize) -> String {
    PlainText::parse(text, max_bytes).into_string()
}

/// I17, law 6: a locator is a model's claim about where something is. One past the document's
/// own length (its `Read:` line, written by the runtime) would make a page "held" that doesn't
/// exist, so it is dropped and counted; the part keeps its words.
fn drop_locators_past_the_end(digest: &mut Digest) {
    let Some(extent) = digest.read.extent else {
        return;
    };
    let past = |at: &mut Option<Locator>, dropped: &mut u32| {
        let beyond = match (*at, extent) {
            (Some(Locator::Pages { last, .. }), Extent::Pages(count))
            | (Some(Locator::Lines { last, .. }), Extent::Lines(count)) => last > count,
            (Some(Locator::Pages { .. }), Extent::Lines(_))
            | (Some(Locator::Lines { .. }), Extent::Pages(_))
            | (None, _) => false,
        };
        if beyond {
            *at = None;
            *dropped += 1;
        }
    };
    let dropped = &mut digest.dropped.locators;
    for section in &mut digest.sections {
        past(&mut section.at, dropped);
    }
    for figure in &mut digest.figures {
        past(&mut figure.at, dropped);
    }
    for table in &mut digest.tables {
        past(&mut table.at, dropped);
    }
    for claim in &mut digest.claims {
        past(&mut claim.at, dropped);
    }
}

fn section_named(heading: &str) -> Section {
    let name = heading.trim().trim_end_matches(':').to_ascii_lowercase();
    match name.as_str() {
        "sections" | "outline" | "contents" => Section::Sections,
        "figures" | "figures and diagrams" | "diagrams" => Section::Figures,
        "tables" => Section::Tables,
        "claims" | "key claims" => Section::Claims,
        _ => Section::Unknown,
    }
}

/// `14 pages; scanned; figures by Gemini 3.8 Flash` (or `text only`).
fn read_line(text: &str) -> DigestRead {
    let mut read = DigestRead::default();
    for part in text.split(';').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        if lower == "scanned" {
            read.scanned = true;
        } else if let Some(by) = strip_prefix_ignoring_case(part, "figures by ") {
            let by = bounded_reader(by);
            read.figures_by = (!by.is_empty()).then_some(by);
        } else if let Some((count, unit)) = part.split_once(' ') {
            let count = count
                .parse::<u32>()
                .ok()
                .filter(|count| (1..=DOCUMENT_LOCATION_MAX).contains(count));
            read.extent = match (count, unit.trim()) {
                (Some(count), "page" | "pages") => Some(Extent::Pages(count)),
                (Some(count), "line" | "lines") => Some(Extent::Lines(count)),
                _ => read.extent,
            };
        }
    }
    read
}

/// `Figure 3 (p. 5): caption`, `Figure 3: caption (p. 5)`, `Fig. 3 (p. 5) caption`, `Table 2`.
fn numbered_heading(heading: &str, names: &[&str]) -> Option<(u32, Option<Locator>, String)> {
    let lower = heading.to_ascii_lowercase();
    let name = names.iter().find(|name| lower.starts_with(**name))?;
    let rest = heading[name.len()..].trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 4 {
        return None;
    }
    let number: u32 = rest[..digits].parse().ok().filter(|number| *number >= 1)?;
    let rest = rest[digits..].trim_start();
    let (at, rest) = match leading_locator(rest) {
        Some((at, rest)) => (Some(at), rest),
        None => (None, rest),
    };
    let rest = rest
        .trim_start()
        .trim_start_matches([':', '.', '—', '–', '-'])
        .trim();
    let (caption, at) = match at {
        Some(at) => (rest, Some(at)),
        None => trailing_locator(rest),
    };
    Some((number, at, plain(caption, DIGEST_CAPTION_MAX_BYTES)))
}

/// A locator in parentheses at the start of `text`, and what follows it.
fn leading_locator(text: &str) -> Option<(Locator, &str)> {
    let inner = text.strip_prefix('(')?;
    let end = inner.find(')')?;
    let at = Locator::parse(&inner[..end])?;
    Some((at, &inner[end + 1..]))
}

/// `text (p. 5)` → `text` and the locator; without one, `text` unchanged.
fn trailing_locator(text: &str) -> (&str, Option<Locator>) {
    let trimmed = text.trim_end().trim_end_matches('.');
    let Some(inner) = trimmed.strip_suffix(')') else {
        return (text.trim(), None);
    };
    let Some(open) = inner.rfind('(') else {
        return (text.trim(), None);
    };
    match Locator::parse(&inner[open + 1..]) {
        Some(at) => (inner[..open].trim_end(), Some(at)),
        None => (text.trim(), None),
    }
}

/// Appends a line to a paragraph within `max_bytes`; false when it didn't fit.
fn append(target: &mut String, line: &str, max_bytes: usize) -> bool {
    let extra = usize::from(!target.is_empty());
    if target.len() + extra + line.len() > max_bytes {
        return false;
    }
    if extra == 1 {
        target.push(' ');
    }
    target.push_str(line);
    true
}

/// `text` after a case-insensitive label ("Digest: x" → "x"), else `text`.
fn strip_label<'a>(text: &'a str, label: &str) -> &'a str {
    strip_prefix_ignoring_case(text, label).map_or(text, str::trim)
}

fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| text[prefix.len()..].trim())
}

/// A list item's text without its bullet.
fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "• "]
        .iter()
        .find_map(|mark| line.strip_prefix(mark))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
}
