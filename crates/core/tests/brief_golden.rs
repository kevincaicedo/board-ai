//! Research brief parser tests (D-185, LLD §7): golden files (a whole brief, a chatty one, a
//! malformed one, an oversized one), the canonical form parsing back to itself, the bounds on
//! every golden file and on input past the size limit, and random input that never panics.
//!
//! The brief arrives whole, as the harness's final answer, so it isn't stream-parsed and has no
//! split-invariance property. `UPDATE_GOLDEN=1 cargo test -p boardai-core --test brief_golden`
//! rewrites the expected files; review the diff before keeping it.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use boardai_core::limits::{
    BRIEF_CITATIONS_MAX, BRIEF_COMPONENTS_MAX, BRIEF_DETAILS_MAX, BRIEF_DIAGRAM_MAX_BYTES,
    BRIEF_FLOW_STEPS_MAX, BRIEF_FLOWS_MAX, BRIEF_MAX_BYTES, BRIEF_OVERVIEW_MAX_BYTES,
    BRIEF_RELATIONS_MAX,
};
use boardai_core::sources::brief::{Brief, parse};

fn golden_inputs() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/briefs");
    let mut inputs: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("golden directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    inputs.sort();
    // An empty directory would pass every property: it is a failure instead.
    assert!(inputs.len() >= 4, "golden briefs missing: {inputs:?}");
    inputs
}

fn render(brief: &Brief) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "usable: {}", brief.is_usable());
    let _ = writeln!(out, "topic: {:?}", brief.topic);
    let _ = writeln!(
        out,
        "overview ({} bytes): {:?}",
        brief.overview.len(),
        brief.overview
    );
    for part in &brief.components {
        let _ = writeln!(
            out,
            "component {} | {} | {} | {} | {}",
            part.id.as_str(),
            part.name,
            part.kind,
            part.path,
            part.role
        );
    }
    for relation in &brief.relations {
        let _ = writeln!(
            out,
            "relation {} -> {}: {}",
            relation.from.as_str(),
            relation.to.as_str(),
            relation.label
        );
    }
    for flow in &brief.flows {
        let _ = writeln!(out, "flow {:?}", flow.name);
        for step in &flow.steps {
            let cited = step
                .citation
                .as_ref()
                .map_or_else(String::new, |citation| format!(" @ {}", citation.label()));
            let _ = writeln!(out, "  step {:?}{cited}", step.text);
        }
    }
    if let Some(diagram) = &brief.diagram {
        let ids: Vec<&str> = diagram.ids.iter().map(|id| id.as_str()).collect();
        let _ = writeln!(
            out,
            "diagram {:?}, {} bytes, ids {ids:?}",
            diagram.title,
            diagram.text.len()
        );
    }
    for detail in &brief.details {
        let nested = detail.diagram.as_ref().map_or(String::new(), |diagram| {
            format!(", diagram {:?}", diagram.title)
        });
        let _ = writeln!(
            out,
            "detail {}: {:?}{nested}",
            detail.component.as_str(),
            detail.notes
        );
    }
    for citation in &brief.citations {
        let _ = writeln!(out, "citation {}", citation.label());
    }
    let _ = writeln!(out, "dropped: {:?}", brief.dropped);
    out
}

#[test]
fn golden_briefs_match() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable brief");
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

/// What the cache keeps and the talker reads parses back to the same brief, with nothing left
/// to drop.
#[test]
fn the_canonical_form_parses_back_to_itself() {
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable brief");
        let brief = parse(&text);
        let again = parse(&brief.to_markdown());
        assert_eq!(
            again.dropped.total(),
            0,
            "{}: {:?}",
            input.display(),
            again.dropped
        );
        assert_eq!(
            Brief {
                dropped: brief.dropped,
                ..again
            },
            brief,
            "{}",
            input.display()
        );
    }
}

fn assert_bounded(brief: &Brief, what: &str) {
    assert!(brief.overview.len() <= BRIEF_OVERVIEW_MAX_BYTES, "{what}");
    assert!(brief.components.len() <= BRIEF_COMPONENTS_MAX, "{what}");
    assert!(brief.relations.len() <= BRIEF_RELATIONS_MAX, "{what}");
    assert!(brief.flows.len() <= BRIEF_FLOWS_MAX, "{what}");
    assert!(
        brief
            .flows
            .iter()
            .all(|flow| flow.steps.len() <= BRIEF_FLOW_STEPS_MAX),
        "{what}"
    );
    assert!(brief.details.len() <= BRIEF_DETAILS_MAX, "{what}");
    assert!(brief.citations.len() <= BRIEF_CITATIONS_MAX, "{what}");
    assert!(brief.all_citations().len() <= BRIEF_CITATIONS_MAX, "{what}");
    let diagrams = brief.diagram.iter().chain(
        brief
            .details
            .iter()
            .filter_map(|detail| detail.diagram.as_ref()),
    );
    for diagram in diagrams {
        assert!(diagram.text.len() <= BRIEF_DIAGRAM_MAX_BYTES, "{what}");
    }
    assert!(brief.to_markdown().len() <= BRIEF_MAX_BYTES * 2, "{what}");
}

#[test]
fn every_brief_holds_to_its_bounds() {
    for input in golden_inputs() {
        let text = fs::read_to_string(&input).expect("readable brief");
        assert_bounded(&parse(&text), &input.display().to_string());
    }
    let mut huge = String::from("# Brief: huge\n## Overview\nIt is big.\n## Components\n");
    let mut index = 0;
    while huge.len() <= BRIEF_MAX_BYTES * 2 {
        let _ = writeln!(
            huge,
            "- part-{index} · Part {index} · module · src/p{index}.rs — big"
        );
        index += 1;
    }
    let brief = parse(&huge);
    assert!(
        brief.dropped.truncated,
        "past the size limit the text is cut, and counted"
    );
    assert_eq!(brief.components.len(), BRIEF_COMPONENTS_MAX);
    assert_bounded(&brief, "huge");
}

/// Random bytes and random mixes of the shape's own lines never panic and stay bounded.
#[test]
fn random_input_never_panics() {
    let pieces = [
        "# Brief: x\n",
        "## Components\n",
        "- a · A · t · p — r\n",
        "## Relations\n",
        "- a → b: c\n",
        "## Flows\n",
        "### f\n",
        "1. s (p.rs:3)\n",
        "## Diagram\n",
        "<diagram title=\"d\">\n",
        "<node id=\"a\"/>\n",
        "</diagram>\n",
        "## Details\n",
        "### a\n",
        "## Citations\n",
        "- p.rs:1-9\n",
        "é😀\u{0}",
        "\n\n",
        "·|→-",
    ];
    let mut state: u64 = 0x5eed_b41e_f00d;
    for _ in 0..500 {
        let mut text = String::new();
        for _ in 0..64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let index = usize::try_from(state >> 33).unwrap_or(0) % pieces.len();
            text.push_str(pieces[index]);
        }
        assert_bounded(&parse(&text), &text);
        let bytes: Vec<u8> = text.bytes().rev().collect();
        assert_bounded(&parse(&String::from_utf8_lossy(&bytes)), "reversed");
    }
}
