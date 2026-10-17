//! A document's text layer: what BoardAI extracts on this Mac (D-186). A PDF's pages come from
//! PDFKit's lines, with headings found by font size, captions by their "Figure N" / "Table N"
//! start, and a table's rows under its caption; Markdown and text are read natively, with
//! headings, pipe tables and image links. All of it is bounded; nothing here decides anything
//! about the conversation.

use std::path::{Component, Path, PathBuf};

use boardai_core::sources::DocumentFormat;
use boardai_core::sources::digest::Locator;

use super::pdfkit::{PdfLine, PdfText};
use crate::limits::{
    DOCUMENT_LINES_MAX, DOCUMENT_PAGE_MAX_BYTES, MARKDOWN_IMAGE_LINKS_MAX,
    MARKDOWN_IMAGE_MAX_BYTES, PDF_HEADING_MAX_BYTES, PDF_TABLE_ROWS_MAX, PDF_TITLE_LINES_MAX,
};
use crate::tools::is_credential;

/// What a run of text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Heading,
    /// A figure's or a table's caption.
    Caption,
    /// Body text, its lines joined.
    Paragraph,
    /// Smaller text: a figure's labels, a footnote, a table's row.
    Small,
}

/// A run of text and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// A page of a PDF, or a line of Markdown or text.
    pub at: Locator,
}

/// A caption ("Figure 3: Recovery as a state machine.").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caption {
    pub number: u32,
    pub at: Locator,
    pub caption: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableText {
    pub number: u32,
    pub at: Locator,
    pub caption: String,
    /// Its rows, each a Markdown row (`| a | b |`).
    pub rows: Vec<String>,
}

/// An image a Markdown document links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLink {
    pub alt: String,
    pub line: u32,
    /// The file it names, inside the document's folder, there, and small enough to send; `None`
    /// for a link to the web (never fetched) or a file that isn't readable.
    pub local: Option<LocalImage>,
    pub remote: bool,
}

/// A linked image on this Mac: where it is, and the link as the document writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImage {
    /// Canonical: symlinks resolved, inside the document's folder.
    pub path: PathBuf,
    /// The link's own relative path (`figures/arena.png`, no `.` parts), so a private copy of
    /// the document keeps its links working (D-231).
    pub linked: PathBuf,
}

/// A document's text as BoardAI extracted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLayer {
    pub format: DocumentFormat,
    pub title: Option<String>,
    /// A PDF's length, all its pages; 0 for Markdown and text.
    pub page_count: u32,
    /// The page `pages[0]` is (1-based): 1 for a whole text layer, the first page asked for a
    /// page read's.
    pub first_page: u32,
    /// A PDF's pages read (at most `DOCUMENT_PAGES_MAX`), each its text: paragraphs and other
    /// runs, one per line. Empty for Markdown and text.
    pub pages: Vec<String>,
    /// Markdown or text: its lines (at most `DOCUMENT_LINES_MAX`).
    pub line_count: u32,
    pub blocks: Vec<Block>,
    pub figures: Vec<Caption>,
    pub tables: Vec<TableText>,
    pub images: Vec<ImageLink>,
    /// Pages with no text at all: a scan.
    pub empty_pages: u32,
}

impl TextLayer {
    /// Every page read is an image: a scanned PDF.
    #[must_use]
    pub fn is_scanned(&self) -> bool {
        !self.pages.is_empty()
            && usize::try_from(self.empty_pages).is_ok_and(|empty| empty == self.pages.len())
    }

    /// Page `page`'s text (1-based), when it has any.
    #[must_use]
    pub fn page(&self, page: u32) -> Option<&str> {
        let index = usize::try_from(page.checked_sub(self.first_page)?).ok()?;
        self.pages
            .get(index)
            .map(String::as_str)
            .filter(|text| !text.is_empty())
    }
}

/// `Figure 3: …`, `Fig. 3. …`, `Table 2 – …`: the kind, the number and the rest.
fn caption_start(text: &str) -> Option<(bool, u32, &str)> {
    let lower = text.to_ascii_lowercase();
    let (figure, rest) = [("figure", true), ("fig.", true), ("table", false)]
        .iter()
        .find_map(|(name, figure)| {
            lower
                .starts_with(name)
                .then(|| (*figure, text[name.len()..].trim_start()))
        })?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 4 {
        return None;
    }
    let number: u32 = rest[..digits].parse().ok().filter(|number| *number >= 1)?;
    let after = rest[digits..].trim_start();
    let caption = after
        .strip_prefix([':', '.', '–', '—', '-'])
        .map(str::trim)?;
    Some((figure, number, caption))
}

/// The size most of a document's text is set in: its body.
fn body_size(text: &PdfText) -> f32 {
    let mut sizes: Vec<(i32, usize)> = Vec::new();
    for line in text.pages.iter().flatten() {
        #[allow(clippy::cast_possible_truncation, reason = "font sizes are small")]
        let key = (line.size * 10.0).round() as i32;
        match sizes.iter_mut().find(|(size, _)| *size == key) {
            Some((_, count)) => *count += line.text.len(),
            None => sizes.push((key, line.text.len())),
        }
    }
    #[allow(clippy::cast_precision_loss, reason = "a font size in tenths")]
    sizes
        .iter()
        .max_by_key(|(size, count)| (*count, *size))
        .map_or(10.0, |(size, _)| *size as f32 / 10.0)
}

fn is_heading(line: &PdfLine, body: f32) -> bool {
    let text = line.text.trim();
    let short =
        !text.is_empty() && text.len() <= PDF_HEADING_MAX_BYTES && !text.ends_with(['.', ',', ';']);
    short && (line.size >= body + 1.5 || (line.bold && line.size >= body + 0.5))
}

/// A line that is only a page number, at a page's end.
fn is_folio(line: &PdfLine) -> bool {
    let text = line.text.trim();
    !text.is_empty() && text.len() <= 4 && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Joins a line onto a paragraph, mending a word split at the line's end ("gad-" + "gets").
fn join_line(paragraph: &mut String, line: &str) {
    let line = line.trim();
    if paragraph.is_empty() {
        paragraph.push_str(line);
        return;
    }
    let mended = paragraph.ends_with('-')
        && !paragraph.ends_with("--")
        && line.chars().next().is_some_and(char::is_lowercase);
    if mended {
        paragraph.pop();
    } else {
        paragraph.push(' ');
    }
    paragraph.push_str(line);
}

/// What a PDF's lines say, page by page.
#[must_use]
pub fn from_pdf(text: &PdfText) -> TextLayer {
    let body = body_size(text);
    let mut layer = TextLayer {
        format: DocumentFormat::Pdf,
        // Only page 1 has the title.
        title: (text.first_page == 1)
            .then(|| pdf_title(text, body))
            .flatten(),
        page_count: text.page_count,
        first_page: text.first_page,
        pages: Vec::new(),
        line_count: 0,
        blocks: Vec::new(),
        figures: Vec::new(),
        tables: Vec::new(),
        images: Vec::new(),
        empty_pages: 0,
    };
    for (number, lines) in (text.first_page..).zip(&text.pages) {
        let mut lines: &[PdfLine] = lines;
        while let Some((last, rest)) = lines.split_last() {
            if !is_folio(last) && !last.text.trim().is_empty() {
                break;
            }
            lines = rest;
        }
        if lines.is_empty() {
            layer.empty_pages += 1;
            layer.pages.push(String::new());
            continue;
        }
        let blocks = page_blocks(lines, body, number);
        layer.pages.push(page_text(&blocks));
        layer.blocks.extend(blocks);
    }
    collect_parts(&mut layer);
    layer
}

/// The largest text at the top of page 1, its lines joined.
fn pdf_title(text: &PdfText, body: f32) -> Option<String> {
    let first = text.pages.first()?;
    let largest = first
        .iter()
        .take(PDF_TITLE_LINES_MAX)
        .map(|line| line.size)
        .fold(0.0_f32, f32::max);
    if largest < body + 2.0 {
        return None;
    }
    let mut title = String::new();
    for line in first.iter().skip_while(|line| line.size < largest) {
        if (line.size - largest).abs() > 0.05 {
            break;
        }
        join_line(&mut title, &line.text);
    }
    (!title.is_empty()).then_some(title)
}

fn page_blocks(lines: &[PdfLine], body: f32, page: u32) -> Vec<Block> {
    let at = Locator::Pages {
        first: page,
        last: page,
    };
    let mut blocks: Vec<Block> = Vec::new();
    let mut previous: Option<(BlockKind, f32)> = None;
    for line in lines {
        let text = line.text.trim();
        if text.is_empty() {
            previous = None;
            continue;
        }
        let kind = if caption_start(text).is_some() {
            BlockKind::Caption
        } else if is_heading(line, body) {
            BlockKind::Heading
        } else if (line.size - body).abs() <= 0.5 {
            BlockKind::Paragraph
        } else {
            BlockKind::Small
        };
        // A paragraph goes on in the body size; a caption goes on in its own size until it ends
        // its sentence.
        let continues = match (previous, blocks.last()) {
            (Some((BlockKind::Paragraph, _)), Some(_)) => kind == BlockKind::Paragraph,
            (Some((BlockKind::Caption, size)), Some(last)) => {
                kind == BlockKind::Small
                    && (line.size - size).abs() <= 0.05
                    && !last.text.ends_with('.')
            }
            (Some(_) | None, _) => false,
        };
        match (continues, blocks.last_mut()) {
            (true, Some(last)) => join_line(&mut last.text, text),
            (true | false, _) => blocks.push(Block {
                kind,
                text: text.to_owned(),
                at,
            }),
        }
        if !continues {
            previous = Some((kind, line.size));
        }
    }
    blocks
}

/// A page's blocks as its text: one run per line, held to `DOCUMENT_PAGE_MAX_BYTES`.
fn page_text(blocks: &[Block]) -> String {
    let mut text = String::new();
    for block in blocks {
        if text.len() + block.text.len() + 1 > DOCUMENT_PAGE_MAX_BYTES {
            let room = DOCUMENT_PAGE_MAX_BYTES.saturating_sub(text.len() + 1);
            let mut end = room.min(block.text.len());
            while !block.text.is_char_boundary(end) {
                end -= 1;
            }
            if end > 0 {
                text.push_str(&block.text[..end]);
            }
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&block.text);
    }
    text
}

/// Figures and tables from the captions; a table's rows are the smaller lines under its
/// caption, up to the next heading, caption or body paragraph.
fn collect_parts(layer: &mut TextLayer) {
    let blocks = layer.blocks.clone();
    for (index, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Caption {
            continue;
        }
        let Some((figure, number, caption)) = caption_start(&block.text) else {
            continue;
        };
        if figure {
            if !layer.figures.iter().any(|known| known.number == number) {
                layer.figures.push(Caption {
                    number,
                    at: block.at,
                    caption: caption.to_owned(),
                });
            }
            continue;
        }
        if layer.tables.iter().any(|known| known.number == number) {
            continue;
        }
        let rows = blocks[index + 1..]
            .iter()
            .take_while(|next| next.kind == BlockKind::Small && next.at == block.at)
            .take(PDF_TABLE_ROWS_MAX)
            .map(|row| format!("| {} |", row.text.replace('|', "/")))
            .collect();
        layer.tables.push(TableText {
            number,
            at: block.at,
            caption: caption.to_owned(),
            rows,
        });
    }
}

/// Lines of Markdown or text, read natively. `folder` is the document's folder, for the images
/// it links.
#[must_use]
pub fn from_lines(text: &str, format: DocumentFormat, folder: &Path) -> TextLayer {
    let mut layer = TextLayer {
        format,
        title: None,
        page_count: 0,
        first_page: 1,
        pages: Vec::new(),
        line_count: 0,
        blocks: Vec::new(),
        figures: Vec::new(),
        tables: Vec::new(),
        images: Vec::new(),
        empty_pages: 0,
    };
    let lines: Vec<&str> = text.lines().take(DOCUMENT_LINES_MAX).collect();
    layer.line_count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let markdown = format == DocumentFormat::Markdown;
    let mut fenced = false;
    let mut heading: Option<String> = None;
    let mut table: Option<(u32, Vec<String>)> = None;
    for (number, line) in (1_u32..).zip(&lines) {
        let trimmed = line.trim();
        if markdown && trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let at = Locator::Lines {
            first: number,
            last: number,
        };
        if markdown && !fenced && trimmed.starts_with('|') {
            table
                .get_or_insert_with(|| (number, Vec::new()))
                .1
                .push(trimmed.to_owned());
            continue;
        }
        if let Some((first, rows)) = table.take() {
            push_table(&mut layer, first, number - 1, heading.as_deref(), rows);
        }
        if markdown
            && !fenced
            && let Some(title) = markdown_heading(trimmed)
        {
            push_heading(&mut layer, title, trimmed.starts_with("# "), at);
            heading = Some(title.to_owned());
            continue;
        }
        let text = if markdown && !fenced {
            images_on(&mut layer, trimmed, number, folder);
            without_images(trimmed)
        } else {
            trimmed.to_owned()
        };
        if !text.is_empty() {
            layer.blocks.push(Block {
                kind: BlockKind::Paragraph,
                text,
                at,
            });
        }
    }
    if let Some((first, rows)) = table.take() {
        let last = layer.line_count;
        push_table(&mut layer, first, last, heading.as_deref(), rows);
    }
    layer
}

/// A Markdown heading; the first top-level one titles the document.
fn push_heading(layer: &mut TextLayer, title: &str, top: bool, at: Locator) {
    if layer.title.is_none() && top {
        layer.title = Some(title.to_owned());
    }
    layer.blocks.push(Block {
        kind: BlockKind::Heading,
        text: title.to_owned(),
        at,
    });
}

fn markdown_heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let title = line[hashes..]
        .strip_prefix(' ')?
        .trim()
        .trim_end_matches('#')
        .trim();
    (!title.is_empty()).then_some(title)
}

/// A pipe table of at least a header and a divider.
fn push_table(
    layer: &mut TextLayer,
    first: u32,
    last: u32,
    heading: Option<&str>,
    rows: Vec<String>,
) {
    let divided = rows.get(1).is_some_and(|divider| {
        divider
            .chars()
            .all(|ch| matches!(ch, '|' | '-' | ':' | ' '))
    });
    if !divided {
        return;
    }
    let number = u32::try_from(layer.tables.len())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    layer.tables.push(TableText {
        number,
        at: Locator::Lines {
            first,
            last: last.max(first),
        },
        caption: heading.unwrap_or_default().to_owned(),
        rows,
    });
}

/// A line with each `![alt](target)` written as `[image: alt]`, and nothing when the line holds
/// only images.
fn without_images(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        let after = &rest[start + 2..];
        let Some((alt, tail)) = after.split_once("](") else {
            break;
        };
        let Some((_, tail)) = tail.split_once(')') else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(&format!("[image: {}]", alt.trim()));
        rest = tail;
    }
    out.push_str(rest);
    let only_images = out.split("[image:").all(|part| {
        part.split_once(']')
            .map_or(part, |(_, after)| after)
            .trim()
            .is_empty()
    });
    if only_images {
        String::new()
    } else {
        out.trim().to_owned()
    }
}

/// Every `![alt](target)` on a line.
fn images_on(layer: &mut TextLayer, line: &str, number: u32, folder: &Path) {
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        let after = &rest[start + 2..];
        let Some((alt, tail)) = after.split_once("](") else {
            return;
        };
        let Some((target, tail)) = tail.split_once(')') else {
            return;
        };
        rest = tail;
        if layer.images.len() >= MARKDOWN_IMAGE_LINKS_MAX {
            return;
        }
        let target = target.split_whitespace().next().unwrap_or_default();
        let remote = target.contains("://") || target.starts_with("data:");
        let local = if remote {
            None
        } else {
            local_image(folder, target)
        };
        layer.images.push(ImageLink {
            alt: alt.trim().to_owned(),
            line: number,
            local,
            remote,
        });
        let number = u32::try_from(layer.images.len()).unwrap_or(u32::MAX);
        layer.figures.push(Caption {
            number,
            at: Locator::Lines {
                first: layer.images.last().map_or(1, |image| image.line),
                last: layer.images.last().map_or(1, |image| image.line),
            },
            caption: layer
                .images
                .last()
                .map(|image| image.alt.clone())
                .unwrap_or_default(),
        });
    }
}

/// The image file a link names: relative, inside the folder (by where it leads), a file of an
/// image kind within its size, and never a credential path. Anything else is never read.
fn local_image(folder: &Path, target: &str) -> Option<LocalImage> {
    let relative = Path::new(target);
    let inside = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !inside {
        return None;
    }
    let linked: PathBuf = relative
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .collect();
    let extension = linked.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return None;
    }
    let folder = std::fs::canonicalize(folder).ok()?;
    let path = std::fs::canonicalize(folder.join(&linked)).ok()?;
    let metadata = std::fs::metadata(&path).ok()?;
    let sendable = path.starts_with(&folder)
        && !is_credential(&path)
        && metadata.is_file()
        && metadata.len() <= MARKDOWN_IMAGE_MAX_BYTES;
    sendable.then_some(LocalImage { path, linked })
}
