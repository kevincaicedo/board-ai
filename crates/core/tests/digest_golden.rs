//! Document digest parser tests (D-186, D-254, LLD §7): golden files (a whole digest, a chatty
//! one, a malformed one, an oversized one, one that talks back), the canonical form parsing
//! back to itself, the bounds on every golden file and on input past the size limit, random
//! input that never panics, and every kept figure diagram staging through the narration's own
//! `<diagram>` parser (D-234).
//!
//! A digest arrives whole, so it isn't stream-parsed and has no split-invariance property.
//! `UPDATE_GOLDEN=1 cargo test -p boardai-core --test digest_golden` rewrites the expected
//! files; review the diff before keeping it.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use boardai_core::limits::{
    DIGEST_CAPTION_MAX_BYTES, DIGEST_CLAIM_MAX_BYTES, DIGEST_CLAIMS_MAX, DIGEST_DIAGRAM_MAX_BYTES,
    DIGEST_FIGURE_SHOWS_MAX_BYTES, DIGEST_FIGURES_MAX, DIGEST_SECTIONS_MAX,
    DIGEST_SUMMARY_MAX_BYTES, DIGEST_TABLE_ROW_MAX_BYTES, DIGEST_TABLE_ROWS_MAX, DIGEST_TABLES_MAX,
    DOCUMENT_DIGEST_MAX_BYTES,
};
use boardai_core::narration::ops::NarrationOp;
use boardai_core::narration::{NarrationOutput, NarrationStream};
use boardai_core::sources::digest::{Digest, Locator, parse};

fn golden_inputs() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/digests");
    let mut inputs: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("golden directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    inputs.sort();
    // An empty directory would pass every property: it is a failure instead.
    assert!(inputs.len() >= 5, "golden digests missing: {inputs:?}");
    inputs
}

fn at(at: Option<Locator>) -> String {
    at.map_or_else(|| "-".to_owned(), |at| at.label())
}

fn render(digest: &Digest) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "usable: {}", digest.is_usable());
    let _ = writeln!(out, "title: {:?}", digest.title);
    let _ = writeln!(out, "read: {:?}", digest.read);
    for section in &digest.sections {
        let _ = writeln!(
            out,
            "section {:?} @ {}: {:?}",
            section.title,
            at(section.at),
            section.summary
        );
    }
    for figure in &digest.figures {
        let diagram = figure.diagram.as_ref().map_or_else(String::new, |text| {
            format!(", diagram {} bytes", text.len())
        });
        let _ = writeln!(
            out,
            "figure {} @ {}: {:?} shows {:?}{diagram}",
            figure.number,
            at(figure.at),
            figure.caption,
            figure.shows
        );
    }
    for table in &digest.tables {
        let _ = writeln!(
            out,
            "table {} @ {}: {:?}, {} rows",
            table.number,
            at(table.at),
            table.caption,
            table.rows.len()
        );
    }
    for claim in &digest.claims {
        let _ = writeln!(out, "claim @ {}: {:?}", at(claim.at), claim.text);
    }
    let _ = writeln!(out, "tray: {:?}", digest.tray_state());
    let _ = writeln!(out, "dropped: {:?}", digest.dropped);
    out
}

#[test]
fn golden_digests_match() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable digest");
        let rendered = render(&parse(&text));
        let expected_path = input.with_extension("expected");
        if update {
            fs::write(&expected_path, &rendered).expect("written");
            continue;
        }
        let expected = fs::read_to_string(&expected_path).unwrap_or_default();
        assert_eq!(rendered, expected, "{}", input.display());
    }
}

/// What the cache keeps and the talker reads parses back to the same digest, with nothing left
/// to drop.
#[test]
fn the_canonical_form_parses_back_to_itself() {
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable digest");
        let digest = parse(&text);
        let again = parse(&digest.to_markdown());
        assert_eq!(
            again.dropped.total(),
            0,
            "{}: {:?}",
            input.display(),
            again.dropped
        );
        assert_eq!(
            Digest {
                dropped: digest.dropped.clone(),
                ..again
            },
            digest,
            "{}",
            input.display()
        );
    }
}

fn assert_bounded(digest: &Digest, what: &str) {
    assert!(digest.title.len() <= DIGEST_CAPTION_MAX_BYTES, "{what}");
    assert!(digest.sections.len() <= DIGEST_SECTIONS_MAX, "{what}");
    assert!(digest.figures.len() <= DIGEST_FIGURES_MAX, "{what}");
    assert!(digest.tables.len() <= DIGEST_TABLES_MAX, "{what}");
    assert!(digest.claims.len() <= DIGEST_CLAIMS_MAX, "{what}");
    for section in &digest.sections {
        assert!(section.title.len() <= DIGEST_CAPTION_MAX_BYTES, "{what}");
        assert!(section.summary.len() <= DIGEST_SUMMARY_MAX_BYTES, "{what}");
    }
    for figure in &digest.figures {
        assert!(figure.caption.len() <= DIGEST_CAPTION_MAX_BYTES, "{what}");
        assert!(
            figure.shows.len() <= DIGEST_FIGURE_SHOWS_MAX_BYTES,
            "{what}"
        );
        let diagram = figure.diagram.as_deref().unwrap_or_default();
        assert!(diagram.len() <= DIGEST_DIAGRAM_MAX_BYTES, "{what}");
    }
    for table in &digest.tables {
        assert!(table.rows.len() <= DIGEST_TABLE_ROWS_MAX, "{what}");
        assert!(
            table
                .rows
                .iter()
                .all(|row| row.len() <= DIGEST_TABLE_ROW_MAX_BYTES),
            "{what}"
        );
    }
    for claim in &digest.claims {
        assert!(claim.text.len() <= DIGEST_CLAIM_MAX_BYTES, "{what}");
    }
    assert!(
        digest.to_markdown().len() <= DOCUMENT_DIGEST_MAX_BYTES * 2,
        "{what}"
    );
}

#[test]
fn every_digest_holds_to_its_bounds() {
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable digest");
        assert_bounded(&parse(&text), &input.display().to_string());
    }
    let mut huge = String::from("# Digest: huge\n## Claims\n");
    let mut index = 0;
    while huge.len() <= DOCUMENT_DIGEST_MAX_BYTES * 2 {
        let _ = writeln!(huge, "- Claim {index} holds. (p. {})", index % 900 + 1);
        index += 1;
    }
    let digest = parse(&huge);
    assert!(
        digest.dropped.truncated,
        "past the size limit the text is cut, and counted"
    );
    assert_eq!(digest.claims.len(), DIGEST_CLAIMS_MAX);
    assert_bounded(&digest, "huge");
}

/// Random mixes of the shape's own lines never panic and stay bounded.
#[test]
fn random_input_never_panics() {
    let pieces = [
        "# Digest: x\n",
        "Read: 3 pages; figures by M\n",
        "## Sections\n",
        "### Intro (pp. 1–2)\n",
        "## Figures\n",
        "### Figure 1 (p. 2): c\n",
        "### Fig. 9: d (p. 3)\n",
        "<diagram title=\"f\">\n",
        "<node id=\"a\"/>\n",
        "</diagram>\n",
        "## Tables\n",
        "### Table 1 (p. 4)\n",
        "| a | b |\n",
        "## Claims\n",
        "- c (p. 5)\n",
        "é😀\u{0}",
        "\n\n",
        "(p. ()–)",
    ];
    let mut state: u64 = 0x5eed_d16e_57ed;
    for _ in 0..500 {
        let mut text = String::new();
        for _ in 0..64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let index = usize::try_from(state >> 33).unwrap_or(0) % pieces.len();
            text.push_str(pieces[index]);
        }
        let digest = parse(&text);
        assert_bounded(&digest, &text);
        assert_eq!(parse(&digest.to_markdown()).dropped.total(), 0, "{text}");
        let bytes: Vec<u8> = text.bytes().rev().collect();
        assert_bounded(&parse(&String::from_utf8_lossy(&bytes)), "reversed");
    }
}

/// A figure's diagram as the narration stages it: each spec's part count and dropped tags.
fn staged(diagram: &str) -> Vec<(usize, u32)> {
    let mut stream = NarrationStream::new(0);
    let mut out = Vec::new();
    stream.push(
        &format!("Here it is. {diagram} That's the figure."),
        &mut out,
    );
    stream.finish(&mut out);
    out.iter()
        .filter_map(|output| match output {
            NarrationOutput::Op(op) => {
                if let NarrationOp::Diagram(spec) = &op.op {
                    Some((spec.items.len(), spec.dropped))
                } else {
                    None
                }
            }
            NarrationOutput::Clause(_) | NarrationOutput::Dropped(_) => None,
        })
        .collect()
}

/// The planted violation: a block with a tag a diagram can't hold stages with a drop, so the
/// property below can fail.
#[test]
fn a_diagram_with_a_foreign_tag_is_caught() {
    let foreign = "<diagram title=\"Figure 9\">\n<node id=\"a\" label=\"A\"/>\n\
                   <message from=\"a\" to=\"a\"/>\n</diagram>";
    let staged = staged(foreign);
    assert_eq!(staged.len(), 1, "{staged:?}");
    assert!(staged[0].1 > 0, "{staged:?}");
}

/// Every figure diagram a digest keeps is a whole block the narration stages (D-234): fed as
/// the talker's own narration, it becomes one `Diagram` op with its parts and nothing dropped.
#[test]
fn kept_figure_diagrams_stage_through_the_narration_parser() {
    let mut count = 0;
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable digest");
        for figure in parse(&text).figures {
            let Some(diagram) = figure.diagram else {
                continue;
            };
            let specs = staged(&diagram);
            let what = format!("{}: figure {}", input.display(), figure.number);
            assert_eq!(specs.len(), 1, "{what}");
            assert!(specs[0].0 > 0, "{what}");
            assert_eq!(specs[0].1, 0, "{what}");
            count += 1;
        }
    }
    assert!(count >= 2, "the golden digests hold figure diagrams");
}
