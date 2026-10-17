//! Staged diagrams' global layout (D-187), on the 150-element map modelled on the owner's
//! inf-alloc Excalidraw: four allocators side by side, records with their fields, cells, a state
//! machine, nested containers, notes and connectors. Nothing moves once laid out (I6), so the
//! layout's own properties are what keep a reveal readable: parts never overlap, they sit in
//! their containers, sections stand side by side, records along the bottom, connectors clear of
//! what they don't join where a route exists, and the same spec always gives the same geometry.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "tests pick one kind of part out of the whole diagram"
)]

use boardai_core::board::Board;
use boardai_core::board::staged::StagedDiagram;
use boardai_core::board::staged::metrics::part_area;
use boardai_core::geometry::Rect;
use boardai_core::ids::{ElementId, LayerId, OpId};
use boardai_core::narration::diagram::DiagramSpec;
use boardai_core::narration::ops::NarrationOp;
use boardai_core::narration::{NarrationOutput, NarrationStream};
use boardai_core::protocol::StagedPart;

const INF_ALLOC: &str = include_str!("fixtures/inf_alloc.narration");
const MAIN: LayerId = LayerId::MAIN;

/// The diagram spec a narration stages.
fn spec_of(narration: &str) -> DiagramSpec {
    let mut stream = NarrationStream::new(0);
    let mut out = Vec::new();
    stream.push(narration, &mut out);
    stream.finish(&mut out);
    out.into_iter()
        .find_map(|output| match output {
            NarrationOutput::Op(op) => match op.op {
                NarrationOp::Diagram(spec) => Some(*spec),
                _ => None,
            },
            _ => None,
        })
        .expect("the narration stages a diagram")
}

fn staged(board: &Board) -> &StagedDiagram {
    board.staged(MAIN).first().expect("one diagram")
}

fn inf_alloc() -> Board {
    let mut board = Board::new();
    let staged = board
        .stage(MAIN, OpId(1), &spec_of(INF_ALLOC), true, &[])
        .expect("staged");
    assert_eq!(staged.dropped, 0, "{staged:?}");
    board
}

/// Boxes that hold no other part: what must never overlap.
fn solid(diagram: &StagedDiagram) -> Vec<(&ElementId, Rect)> {
    diagram
        .entries
        .iter()
        .filter(|entry| {
            !matches!(
                entry.draw,
                StagedPart::Frame { .. } | StagedPart::Edge { .. }
            )
        })
        .map(|entry| (&entry.id, part_area(&entry.draw)))
        .collect()
}

#[test]
fn the_map_holds_150_elements_laid_out_before_any_stroke() {
    let board = inf_alloc();
    let layer = board.layer(MAIN).expect("main");
    assert!(
        layer.elements().len() >= 150,
        "{} elements",
        layer.elements().len()
    );
    assert!(
        layer
            .elements()
            .iter()
            .all(|element| !element.state.is_shown()),
        "nothing is drawn by staging"
    );
    assert_eq!(layer.bounds(), None, "the camera has nothing to show yet");
    let diagram = staged(&board);
    assert_eq!(diagram.sections, 4);
    assert_eq!(diagram.legend.len(), 4);
}

#[test]
fn no_two_parts_overlap_and_each_sits_in_its_container() {
    let board = inf_alloc();
    let diagram = staged(&board);
    let parts = solid(diagram);
    for (index, (a, first)) in parts.iter().enumerate() {
        for (b, second) in &parts[index + 1..] {
            assert!(
                !first.intersects(*second),
                "{} {first:?} overlaps {} {second:?}",
                a.as_str(),
                b.as_str()
            );
        }
    }
    for (at, entry) in diagram.entries.iter().enumerate() {
        for ancestor in diagram.ancestors(at) {
            let StagedPart::Frame { rect, .. } = diagram.entries[ancestor].draw else {
                panic!("a container is a frame");
            };
            assert!(
                rect.contains(part_area(&entry.draw)),
                "{} is outside {}",
                entry.id.as_str(),
                diagram.entries[ancestor].id.as_str()
            );
        }
    }
}

#[test]
fn sections_stand_side_by_side_and_records_along_their_bottom() {
    let board = inf_alloc();
    let diagram = staged(&board);
    let frame = |name: &str| {
        let entry = diagram
            .entries
            .iter()
            .find(|entry| entry.id.as_str() == name)
            .expect("a section");
        match entry.draw {
            StagedPart::Frame { rect, .. } => rect,
            _ => panic!("{name} is a frame"),
        }
    };
    let sections: Vec<Rect> = ["arena", "pool", "slots", "region"]
        .into_iter()
        .map(frame)
        .collect();
    for pair in sections.windows(2) {
        assert_eq!(pair[0].y, pair[1].y, "one row");
        assert_eq!(pair[0].h, pair[1].h, "as tall as the tallest");
        assert!(pair[1].x > pair[0].right(), "left to right, apart");
    }
    let record = |name: &str| {
        let entry = diagram
            .entries
            .iter()
            .find(|entry| entry.id.as_str() == name)
            .expect("a record");
        part_area(&entry.draw)
    };
    for (section, last) in [
        ("arena", "arena_s"),
        ("pool", "pool_s"),
        ("slots", "slot_s"),
    ] {
        let gap = frame(section).bottom() - record(last).bottom();
        assert!(
            (0..=24).contains(&gap),
            "{last} is {gap} above {section}'s bottom"
        );
    }
    // A record written among other parts stays where it was written.
    assert!(record("addr").y < record("arena_s").y - 200);
}

#[test]
fn the_same_spec_lays_out_the_same_and_the_legend_stands_above() {
    let first = inf_alloc();
    let second = inf_alloc();
    assert_eq!(staged(&first), staged(&second));
    let diagram = staged(&first);
    let top = diagram
        .entries
        .iter()
        .map(|entry| part_area(&entry.draw).y)
        .min()
        .expect("parts");
    for legend in &diagram.legend {
        assert!(legend.rect.bottom() <= top, "the legend is above the parts");
    }
}

#[test]
fn connectors_between_neighbours_are_straight_and_labels_sit_off_the_line() {
    let board = inf_alloc();
    let diagram = staged(&board);
    let edge = |name: &str| {
        diagram
            .entries
            .iter()
            .find(|entry| entry.id.as_str() == name)
            .map(|entry| entry.draw.clone())
            .expect("an edge")
    };
    let StagedPart::Edge { path, label, .. } = edge("alloc_in>class") else {
        panic!("an edge");
    };
    assert!(path.control.is_none(), "boxes side by side join straight");
    let label = label.expect("labelled");
    assert!(label.at.y < path.start.y, "the label sits above the line");
}

/// A diagram on a page with drawing goes under it, clear of everything there, the user's
/// shapes included; one too big for what is left of the page asks for a fresh page (D-177).
#[test]
fn a_diagram_keeps_clear_of_what_is_drawn_and_turns_the_page_when_too_big() {
    use boardai_core::board::NodeRequest;
    use boardai_core::board::staged::StageRefused;
    use boardai_core::narration::ops::{Direction, Placement};
    use boardai_core::time::SessionTime;
    let mut board = Board::new();
    // A tall sketch: ten boxes one under another, drawn.
    let ids: Vec<ElementId> = (0..10)
        .map(|index| ElementId::parse(&format!("n{index}")).expect("id"))
        .collect();
    for (index, id) in ids.iter().enumerate() {
        let placement = match index.checked_sub(1) {
            None => Placement::Auto,
            Some(previous) => Placement::Near {
                target: ids[previous].clone(),
                direction: Direction::Below,
            },
        };
        let op = OpId(u32::try_from(index).expect("small"));
        board
            .place_node(
                MAIN,
                op,
                NodeRequest {
                    id,
                    label: "box",
                    shape: None,
                    placement: &placement,
                    stack: 1,
                    fill: None,
                },
            )
            .expect("placed");
        board.start(MAIN, id, SessionTime::ZERO, 200);
    }
    let api = ids[0].clone();
    let spec = spec_of(INF_ALLOC);
    assert_eq!(
        board.stage(MAIN, OpId(20), &spec, true, &[]),
        Err(StageRefused::NeedsPage),
        "the map and the sketch don't fit one page"
    );
    let small = spec_of(concat!(
        "<diagram title=\"t\"><node id=\"a\" kind=\"group\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\" in=\"a\"/></diagram>"
    ));
    board
        .stage(MAIN, OpId(21), &small, true, &[])
        .expect("fits");
    let sketch = board
        .layer(MAIN)
        .and_then(boardai_core::board::Layer::bounds)
        .expect("the sketch");
    let _ = api;
    for (id, area) in solid(staged(&board)) {
        assert!(
            !area.intersects(sketch),
            "{} lands on the sketch",
            id.as_str()
        );
    }
}

/// How long staging the 150-element map takes, parsing excluded: run with
/// `cargo test --release -p boardai-core --test board_staged -- --ignored --nocapture`. It must
/// stay well under a frame (16 ms) on the orchestrator task; the report quotes it.
#[test]
#[ignore = "a measurement, not a check: timing depends on the machine"]
#[allow(
    clippy::disallowed_methods,
    reason = "measures the layout's time; nothing is decided with the clock"
)]
fn staging_150_elements_takes_well_under_a_frame() {
    let spec = spec_of(INF_ALLOC);
    let runs = 200;
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut board = Board::new();
        let begin = std::time::Instant::now();
        let staged = board
            .stage(MAIN, OpId(1), &spec, true, &[])
            .expect("staged");
        times.push(begin.elapsed());
        assert!(staged.elements >= 150);
    }
    times.sort();
    let p50 = times[runs / 2];
    let max = times[runs - 1];
    eprintln!("staging 150+ elements: p50 {p50:?}, max {max:?} over {runs} runs");
    // Unoptimized, as `just dev` builds the core, it still fits a frame.
    let bound_ms = if cfg!(debug_assertions) { 16 } else { 4 };
    assert!(max < std::time::Duration::from_millis(bound_ms), "{max:?}");
}

/// Reveals `target` (deep) on the main layer at once, as the engine would.
fn reveal(board: &mut Board, op: u32, target: &str) {
    use boardai_core::time::SessionTime;
    let target = ElementId::parse(target).expect("id");
    let reveal = board
        .plan_reveal(MAIN, OpId(op), &target, true)
        .expect("staged");
    board.start_reveal(MAIN, OpId(op), &reveal, SessionTime::ZERO, 100);
    board.settle(SessionTime::from_ms(1_000));
}

/// The talker's summary tells drawn from planned (D-187): a planned part is never told as
/// drawn, and the parts still to reveal are listed by id.
#[test]
fn the_summary_tells_drawn_parts_from_planned_ones() {
    use boardai_core::board::summary::summary;
    let mut board = inf_alloc();
    let before = summary(&board);
    assert!(
        before.contains("diagram alloc \"inf-alloc · memory allocation\", staged"),
        "{before}"
    );
    assert!(before.contains("arena planned: addr, "), "{before}");
    reveal(&mut board, 2, "arena");
    let after = summary(&board);
    assert!(after.contains("arena drawn;"), "{after}");
    assert!(
        after.contains("pool \"AlignedPool · AlignedBox\" planned: "),
        "{after}"
    );
    assert!(!after.contains("main: empty"));
}

/// The export holds what was drawn, as the diagram drew it (category strokes, dashed frames,
/// the legend), and nothing still planned.
#[test]
fn the_export_holds_the_revealed_parts_only() {
    let mut board = inf_alloc();
    reveal(&mut board, 2, "arena");
    let layer = board.layer(MAIN).expect("main");
    let exported = boardai_core::board::excalidraw::export_layer(layer);
    assert!(
        exported.contains("\"strokeColor\":\"#9aa3b2\""),
        "the arena's slate frame"
    );
    assert!(exported.contains("\"strokeStyle\":\"dashed\""));
    assert!(exported.contains("struct Arena"));
    assert!(
        exported.contains("RAM records"),
        "the legend came with the first part"
    );
    assert!(
        !exported.contains("AlignedPool"),
        "the pool is still planned"
    );
}
