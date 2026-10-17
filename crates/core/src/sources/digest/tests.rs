use super::{Digest, DigestRead, Extent, Locator, parse};
use crate::sources::SourceState;

#[test]
fn locators_read_pages_and_lines_as_people_write_them() {
    let read = |text: &str| Locator::parse(text).map(|at| at.label());
    for (text, expected) in [
        ("p. 3", Some("p. 3")),
        ("(p.3)", Some("p. 3")),
        ("pp. 3–4", Some("pp. 3–4")),
        ("pp 3-4", Some("pp. 3–4")),
        ("page 12", Some("p. 12")),
        ("pages 2 to 5", Some("pp. 2–5")),
        ("l. 42", Some("l. 42")),
        ("ll. 40–58", Some("ll. 40–58")),
        ("lines 7-9", Some("ll. 7–9")),
        ("p. 1–p. 1", Some("p. 1")),
        ("pp. 3-pp. 4", Some("pp. 3–4")),
        ("p. 0", None),
        ("pp. 9–3", None),
        ("p. three", None),
        ("p. 12345678", None),
        ("section 3", None),
        ("", None),
    ] {
        assert_eq!(read(text).as_deref(), expected, "{text}");
    }
    let at = Locator::parse("pp. 3–5");
    assert!(at.is_some_and(|at| at.covers_page(4) && !at.covers_page(6)));
    assert!(Locator::parse("l. 4").is_some_and(|at| !at.covers_page(4)));
}

const PAPER: &str = "\
# Digest: FoundationDB
Read: 14 pages; figures by Gemini 3.8 Flash

## Sections
### 1 Introduction (pp. 1–2)
Why an unbundled design.
### 2 Design (pp. 3–5)
The transaction system and the log.

## Figures
### Figure 1 (p. 3): The architecture of FoundationDB.
Clients ask the proxies for a read version.
<diagram title=\"Figure 1\">
<node id=\"proxy\" label=\"Proxies\" kind=\"service\"/>
<node id=\"seq\" label=\"Sequencer\" kind=\"service\"/>
<edge from=\"proxy\" to=\"seq\" label=\"get read version\"/>
</diagram>
### Figure 2 (p. 6): Recovery.

## Tables
### Table 1 (p. 9): Latency by operation.
| Operation | p50 |
|---|---|
| read | 1 ms |

## Claims
- Commits are ordered by the sequencer. (p. 4)
";

#[test]
fn the_tray_says_what_was_read_and_what_was_not() {
    let digest = parse(PAPER);
    assert_eq!(
        digest.tray_state(),
        SourceState::Partial {
            reason: "1 of 2 figures read by Gemini 3.8 Flash · 1 unread".to_owned()
        },
        "Figure 2 has no description: never silently lost"
    );
    let mut all_read = digest.clone();
    all_read.figures[1].shows = "The recovery steps.".to_owned();
    assert_eq!(
        all_read.tray_state(),
        SourceState::Ready {
            detail: "14 pages · text, 1 table and 2 of 2 figures read · figures by Gemini 3.8 \
                     Flash"
                .to_owned()
        }
    );
    let mut text_only = digest.clone();
    text_only.read.figures_by = None;
    assert_eq!(
        text_only.tray_state(),
        SourceState::Partial {
            reason: "Text read · 1 figure needs a model that reads files".to_owned()
        }
    );
    let scanned = Digest {
        read: DigestRead {
            extent: Some(Extent::Pages(1)),
            scanned: true,
            figures_by: None,
        },
        ..Digest::default()
    };
    assert_eq!(
        scanned.tray_state(),
        SourceState::Partial {
            reason: "Scanned pages · its text needs a model that reads files".to_owned()
        }
    );
    let notes = Digest {
        read: DigestRead {
            extent: Some(Extent::Lines(736)),
            scanned: false,
            figures_by: Some("Claude Code".to_owned()),
        },
        ..Digest::default()
    };
    assert_eq!(
        notes.tray_state(),
        SourceState::Ready {
            detail: "Digest ready · 736 lines".to_owned()
        }
    );
}

#[test]
fn a_page_is_covered_by_anything_on_it() {
    let digest = parse(PAPER);
    for page in [1, 2, 3, 4, 5, 6, 9] {
        assert!(digest.covers_page(page), "p. {page}");
    }
    for page in [7, 8, 10, 14] {
        assert!(!digest.covers_page(page), "p. {page}");
    }
    assert_eq!(
        digest
            .section_named("the design section")
            .map(|s| s.title.as_str()),
        Some("2 Design")
    );
    assert!(digest.section_named("the cat").is_none());
}

#[test]
fn ranges_merge_into_one_digest_with_each_figure_once() {
    let first = parse(
        "# Digest: Paper\n## Figures\n### Figure 1 (p. 2): A.\n### Figure 2 (p. 3): B.\nShows B.\n",
    );
    let second = parse(
        "# Digest: Paper (pages 4-6)\n## Figures\n### Figure 1 (p. 2): A.\nShows A.\n\
         ### Figure 3 (p. 5): C.\nShows C.\n## Claims\n- C holds. (p. 5)\n",
    );
    let read = DigestRead {
        extent: Some(Extent::Pages(6)),
        scanned: false,
        figures_by: Some("Claude Haiku 5.5".to_owned()),
    };
    let merged = Digest::merge(vec![first, second], read);
    assert_eq!(merged.title, "Paper");
    let figures: Vec<(u32, bool)> = merged
        .figures
        .iter()
        .map(|figure| (figure.number, figure.is_read()))
        .collect();
    assert_eq!(figures, [(1, true), (2, true), (3, true)]);
    assert_eq!(merged.claims.len(), 1);
    assert_eq!(merged.dropped.total(), 0);
    assert_eq!(parse(&merged.to_markdown()), merged);
}

#[test]
fn diagram_blocks_are_kept_whole_or_dropped() {
    let digest = parse(PAPER);
    let diagram = digest.figure(1).and_then(|figure| figure.diagram.clone());
    assert!(
        diagram.is_some_and(|text| text.starts_with("<diagram") && text.ends_with("</diagram>"))
    );
    let unclosed =
        parse("## Figures\n### Figure 1 (p. 1): A.\n<diagram title=\"x\">\n<node id=\"a\"/>\n");
    assert_eq!(unclosed.figures[0].diagram, None);
    assert_eq!(unclosed.dropped.diagrams, 1);
    let stray = parse(
        "## Sections\n### Intro (p. 1)\n<diagram title=\"x\">\n<node id=\"a\" label=\"### no\"/>\n</diagram>\nIt starts.\n",
    );
    assert_eq!(stray.dropped.diagrams, 1);
    assert_eq!(stray.sections[0].summary, "It starts.");
}
