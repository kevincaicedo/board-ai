//! The text-only digest (D-255): what BoardAI writes from the text layer alone when no model
//! that reads files and no coding harness is set up, and the floor every other digest is
//! completed from. Its figures are listed by their captions, unread: never lost silently.

use boardai_core::sources::DocumentFormat;
use boardai_core::sources::digest::{
    Digest, DigestFigure, DigestRead, DigestSection, DigestTable, Extent, Locator,
};
use boardai_core::sources::pages::sentences;

use super::layer::{BlockKind, TextLayer};
use crate::limits::DIGEST_SUMMARY_SENTENCES;

/// The digest of what the text layer holds: sections from its headings, figures and tables
/// from their captions (figures unread), no claims (they need a model).
#[must_use]
pub fn local_digest(layer: &TextLayer, file_name: &str) -> Digest {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let title = layer.title.clone().unwrap_or_else(|| stem.to_owned());
    let markdown = Digest {
        title,
        read: read_line(layer, None),
        sections: sections(layer),
        figures: figures(layer),
        tables: tables(layer),
        claims: Vec::new(),
        dropped: boardai_core::sources::digest::DigestDropped::default(),
    }
    .to_markdown();
    // Parsed back so it holds to every bound of the digest.
    boardai_core::sources::digest::parse(&markdown)
}

/// How the document was read; `figures_by` is who read its figures, when anyone did.
#[must_use]
pub fn read_line(layer: &TextLayer, figures_by: Option<String>) -> DigestRead {
    DigestRead {
        extent: Some(match layer.format {
            DocumentFormat::Pdf => Extent::Pages(layer.page_count.max(1)),
            DocumentFormat::Markdown | DocumentFormat::Text => {
                Extent::Lines(layer.line_count.max(1))
            }
        }),
        scanned: layer.is_scanned(),
        figures_by,
    }
}

fn sections(layer: &TextLayer) -> Vec<DigestSection> {
    let headings: Vec<usize> = layer
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.kind == BlockKind::Heading)
        .map(|(index, _)| index)
        // A PDF's title is set large, so its lines read as headings: they aren't sections.
        .filter(|index| {
            layer.format != DocumentFormat::Pdf
                || !layer
                    .title
                    .as_deref()
                    .is_some_and(|title| title.contains(layer.blocks[*index].text.as_str()))
        })
        .collect();
    let mut found = Vec::new();
    for (position, &index) in headings.iter().enumerate() {
        let heading = &layer.blocks[index];
        let next = headings.get(position + 1).map(|next| &layer.blocks[*next]);
        let end = match (heading.at, next.map(|next| next.at)) {
            (Locator::Pages { first, .. }, Some(Locator::Pages { first: next, .. })) => {
                Locator::Pages {
                    first,
                    last: next.max(first),
                }
            }
            (Locator::Pages { first, .. }, _) => Locator::Pages {
                first,
                last: layer.page_count.max(first),
            },
            (Locator::Lines { first, .. }, Some(Locator::Lines { first: next, .. })) => {
                Locator::Lines {
                    first,
                    last: next.saturating_sub(1).max(first),
                }
            }
            (Locator::Lines { first, .. }, _) => Locator::Lines {
                first,
                last: layer.line_count.max(first),
            },
        };
        let paragraph = layer.blocks[index + 1..]
            .iter()
            .take_while(|block| block.kind != BlockKind::Heading)
            .find(|block| block.kind == BlockKind::Paragraph)
            .map(|block| sentences(&block.text, DIGEST_SUMMARY_SENTENCES).join(" "))
            .unwrap_or_default();
        found.push(DigestSection {
            title: heading.text.clone(),
            at: Some(end),
            summary: paragraph,
        });
    }
    found
}

fn figures(layer: &TextLayer) -> Vec<DigestFigure> {
    layer
        .figures
        .iter()
        .map(|figure| {
            let remote = layer
                .images
                .get(usize::try_from(figure.number.saturating_sub(1)).unwrap_or(usize::MAX))
                .is_some_and(|image| image.remote && layer.format == DocumentFormat::Markdown);
            let caption = if remote {
                format!("{} (linked from the web, not fetched)", figure.caption)
            } else {
                figure.caption.clone()
            };
            DigestFigure {
                number: figure.number,
                at: Some(figure.at),
                caption,
                shows: String::new(),
                diagram: None,
            }
        })
        .collect()
}

fn tables(layer: &TextLayer) -> Vec<DigestTable> {
    layer
        .tables
        .iter()
        .map(|table| DigestTable {
            number: table.number,
            at: Some(table.at),
            caption: table.caption.clone(),
            rows: table.rows.clone(),
        })
        .collect()
}

/// A model's (or a harness's) digest completed from the text layer: every figure and table the
/// layer found by its caption and the answer left out is added, unread, so nothing is lost
/// silently (D-186). The model's title and parts come first.
#[must_use]
pub fn completed(mut answer: Digest, local: &Digest, read: DigestRead) -> Digest {
    if answer.title.is_empty() {
        answer.title.clone_from(&local.title);
    }
    if answer.sections.is_empty() {
        answer.sections.clone_from(&local.sections);
    }
    for figure in &local.figures {
        if answer.figure(figure.number).is_none() {
            answer.figures.push(figure.clone());
        }
    }
    answer.figures.sort_by_key(|figure| figure.number);
    for table in &local.tables {
        if answer.table(table.number).is_none() {
            answer.tables.push(table.clone());
        }
    }
    answer.tables.sort_by_key(|table| table.number);
    answer.read = read;
    boardai_core::sources::digest::parse(&answer.to_markdown())
}

#[cfg(test)]
mod tests {
    use boardai_core::sources::digest::{DigestRead, Extent, parse};

    use super::completed;

    /// The planted violation for "never lost silently": an answer that leaves out a figure the
    /// text names gets it back, unread, and the tray says so.
    #[test]
    fn a_figure_the_answer_left_out_comes_back_unread() {
        let local = parse(
            "# Digest: Paper\n## Figures\n### Figure 1 (p. 2): Parts.\n### Figure 2 (p. 4): Steps.\n\
             ## Tables\n### Table 1 (p. 3): Roles.\n| a |\n",
        );
        let answer =
            parse("# Digest: Paper\n## Figures\n### Figure 1 (p. 2): Parts.\nShows the parts.\n");
        let read = DigestRead {
            extent: Some(Extent::Pages(4)),
            scanned: false,
            figures_by: Some("Gemini 3.8 Flash".to_owned()),
        };
        let digest = completed(answer, &local, read);
        let figures: Vec<(u32, bool)> = digest
            .figures
            .iter()
            .map(|figure| (figure.number, figure.is_read()))
            .collect();
        assert_eq!(figures, [(1, true), (2, false)]);
        assert!(digest.table(1).is_some(), "the text's table too");
        assert_eq!(
            digest.tray_state(),
            boardai_core::sources::SourceState::Partial {
                reason: "1 of 2 figures read by Gemini 3.8 Flash · 1 unread".to_owned()
            }
        );
    }
}
