//! Board layout for the diagram types (LLD §2.5, D-120, invariant I6): sequence diagrams, rows of
//! cells, trees and off-grid placement never move what is already drawn, a lifeline only grows,
//! and a message dropped before it was drawn changes nothing. Plus their Excalidraw export.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use std::collections::BTreeMap;

use boardai_core::board::excalidraw::export_layer;
use boardai_core::board::{
    AnnotationRequest, Board, EdgeRequest, ElementKind, NodeRequest, Placed, Rejected,
};
use boardai_core::geometry::{Point, Rect};
use boardai_core::ids::{ElementId, LayerId, OpId};
use boardai_core::narration::ops::{CellsSpec, Direction, IndexLabels, Placement, Side};
use boardai_core::protocol::{DrawOp, EdgePath, LineStyle, Mark, NodeShape};
use boardai_core::time::SessionTime;

fn id(text: &str) -> ElementId {
    ElementId::parse(text).expect("valid id")
}

const MAIN: LayerId = LayerId::MAIN;

#[derive(Default)]
struct Builder {
    board: Board,
    next_op: u32,
    now: u64,
}

impl Builder {
    fn op(&mut self) -> OpId {
        self.next_op += 1;
        OpId(self.next_op)
    }

    fn start(&mut self, placed: Result<Placed, Rejected>) -> DrawOp {
        let placed = placed.expect("placed");
        self.now += 500;
        let now = SessionTime::from_ms(self.now);
        match placed {
            Placed::New { id, draw } | Placed::Before { id, draw } => {
                let _ = self.board.start(MAIN, &id, now, 300);
                draw
            }
            Placed::Write { target, draw } => {
                if let DrawOp::Write { label, .. } = &draw {
                    self.board.apply_write(MAIN, &target, label);
                }
                draw
            }
            Placed::Strike { target, draw } => {
                if let DrawOp::Strike { label, .. } = &draw {
                    self.board.apply_strike(MAIN, &target, label);
                }
                draw
            }
            Placed::ResumeFrozen | Placed::Unchanged | Placed::Reveal(_) => {
                panic!("nothing new was placed")
            }
        }
    }

    fn node(&mut self, name: &str, shape: Option<NodeShape>, placement: Placement) -> DrawOp {
        self.labelled(name, name, shape, placement)
    }

    fn labelled(
        &mut self,
        name: &str,
        label: &str,
        shape: Option<NodeShape>,
        placement: Placement,
    ) -> DrawOp {
        let op = self.op();
        let node_id = id(name);
        let placed = self.board.place_node(
            MAIN,
            op,
            NodeRequest {
                id: &node_id,
                label,
                shape,
                placement: &placement,
                stack: 1,
                fill: None,
            },
        );
        self.start(placed)
    }

    fn child(&mut self, name: &str, parent: &str, side: Option<Side>) -> DrawOp {
        let placement = Placement::Child {
            parent: id(parent),
            side,
        };
        self.node(name, None, placement)
    }

    fn participant(&mut self, name: &str) -> DrawOp {
        let op = self.op();
        let placed = self.board.place_participant(MAIN, op, &id(name), name);
        self.start(placed)
    }

    fn message(&mut self, from: &str, to: &str) -> Result<Placed, Rejected> {
        let op = self.op();
        let (from, to) = (id(from), id(to));
        self.board.place_message(
            MAIN,
            op,
            EdgeRequest {
                from: &from,
                to: &to,
                label: Some("call"),
                style: LineStyle::Solid,
            },
        )
    }

    fn edge(&mut self, from: &str, to: &str) -> DrawOp {
        let op = self.op();
        let (from, to) = (id(from), id(to));
        let placed = self.board.place_edge(
            MAIN,
            op,
            EdgeRequest {
                from: &from,
                to: &to,
                label: None,
                style: LineStyle::Solid,
            },
        );
        self.start(placed)
    }

    fn cells(&mut self, name: &str, count: u8, vertical: bool, placement: Placement) -> DrawOp {
        let op = self.op();
        let spec = CellsSpec {
            id: id(name),
            label: name.to_owned(),
            values: vec!["3".into(), "1".into()],
            count,
            index: IndexLabels::From(0),
            vertical,
            placement,
        };
        let placed = self.board.place_cells(MAIN, op, &spec);
        self.start(placed)
    }

    fn areas(&self) -> BTreeMap<String, (Rect, bool)> {
        self.board
            .layer(MAIN)
            .expect("main")
            .elements()
            .iter()
            .map(|element| {
                let grows = matches!(element.kind, ElementKind::Participant { .. });
                (element.id.as_str().to_owned(), (element.area(), grows))
            })
            .collect()
    }
}

fn near(target: &str, direction: Direction) -> Placement {
    Placement::Near {
        target: id(target),
        direction,
    }
}

/// I6 across every diagram type: each new drawing leaves every earlier one where it was (a
/// lifeline only grows downward), and no two solid drawings overlap.
#[test]
fn diagrams_never_move_what_is_already_drawn() {
    let mut board = Builder::default();
    let mut before = board.areas();
    let mut check = |board: &Builder, step: &str| {
        let after = board.areas();
        for (name, (area, grows)) in &before {
            let (now, _) = after[name];
            if *grows {
                assert_eq!(
                    (now.x, now.y, now.w),
                    (area.x, area.y, area.w),
                    "{name} moved at {step}"
                );
                assert!(now.bottom() >= area.bottom(), "{name} shrank at {step}");
            } else {
                assert_eq!(now, *area, "{name} moved at {step}");
            }
        }
        before = after;
    };
    board.node("api", None, Placement::Auto);
    check(&board, "api");
    board.cells("arr", 6, false, near("api", Direction::Below));
    check(&board, "arr");
    board.node(
        "root",
        Some(NodeShape::Circle),
        near("arr", Direction::Right),
    );
    check(&board, "root");
    for (name, parent, side) in [
        ("l", "root", Some(Side::Left)),
        ("r", "root", Some(Side::Right)),
        ("ll", "l", Some(Side::Left)),
        ("lr", "l", Some(Side::Right)),
        ("rl", "r", Some(Side::Left)),
        ("x", "r", None),
        ("y", "r", None),
    ] {
        board.child(name, parent, side);
        check(&board, name);
    }
    board.cells("ht", 4, true, Placement::Auto);
    check(&board, "ht");
    board.node("k1", None, near("ht.1", Direction::Right));
    check(&board, "k1");
    board.node("k2", None, near("k1", Direction::Right));
    check(&board, "k2");
    board.participant("p");
    board.participant("q");
    check(&board, "participants");
    for step in 0..6 {
        let placed = board.message("p", if step % 3 == 2 { "p" } else { "q" });
        board.start(placed);
        check(&board, "message");
    }
    board.node("after", None, Placement::Auto);
    check(&board, "after");
    assert_apart(&board);
}

/// No two solid drawings (nodes, rows of cells, participants with their lifelines) overlap.
fn assert_apart(board: &Builder) {
    let layer = board.board.layer(MAIN).expect("main");
    let solid: Vec<(&str, Rect)> = layer
        .elements()
        .iter()
        .filter(|element| match &element.kind {
            ElementKind::Node { .. } | ElementKind::Cells { .. } => true,
            ElementKind::Participant { .. } | ElementKind::User { .. } => true,
            ElementKind::Edge { .. }
            | ElementKind::Annotation { .. }
            | ElementKind::Trace
            | ElementKind::Message { .. }
            | ElementKind::Cell { .. } => false,
        })
        .map(|element| (element.id.as_str(), element.area()))
        .collect();
    for (index, (a, first)) in solid.iter().enumerate() {
        for (b, second) in &solid[index + 1..] {
            assert!(!first.intersects(*second), "{a} overlaps {b}");
        }
    }
}

/// A message laid out and then dropped by an interruption leaves the sequence as it was drawn:
/// the next message takes its row, and its lifeline stretches start where the drawn lines end.
#[test]
fn a_dropped_message_changes_nothing() {
    let mut board = Builder::default();
    board.participant("a");
    board.participant("b");
    for _ in 0..2 {
        let placed = board.message("a", "b");
        board.start(placed);
    }
    let lifeline = |board: &Builder, name: &str| match board
        .board
        .layer(MAIN)
        .and_then(|layer| layer.element(&id(name)))
        .map(|element| element.kind.clone())
    {
        Some(ElementKind::Participant {
            lifeline_bottom, ..
        }) => lifeline_bottom,
        other => panic!("{name} is a participant: {other:?}"),
    };
    let drawn = [lifeline(&board, "a"), lifeline(&board, "b")];
    let Ok(Placed::New {
        id: dropped,
        draw: DrawOp::Message { points, extend, .. },
    }) = board.message("b", "a")
    else {
        panic!("a new message");
    };
    assert_eq!(extend.len(), 2, "the third message needs longer lifelines");
    board.board.remove_pending(MAIN, &dropped);
    assert_eq!(
        [lifeline(&board, "a"), lifeline(&board, "b")],
        drawn,
        "laying out a message grows nothing until it is drawn"
    );
    let placed = board.message("a", "b");
    let DrawOp::Message {
        points: next,
        extend,
        ..
    } = board.start(placed)
    else {
        panic!("a message");
    };
    assert_eq!(next[0].y, points[0].y, "the dropped row is taken again");
    let tops: Vec<i32> = extend.iter().map(|stretch| stretch.top).collect();
    assert_eq!(tops, drawn, "each stretch starts where its line ends");
}

#[test]
fn a_cell_id_already_taken_drops_the_row_and_a_missing_parent_continues_the_flow() {
    let mut board = Builder::default();
    board.node("arr.1", None, Placement::Auto);
    let op = board.op();
    let spec = CellsSpec {
        id: id("arr"),
        label: String::new(),
        values: Vec::new(),
        count: 3,
        index: IndexLabels::None,
        vertical: false,
        placement: Placement::Auto,
    };
    assert_eq!(
        board.board.place_cells(MAIN, op, &spec),
        Err(Rejected::IdTaken(id("arr.1")))
    );
    let DrawOp::Node { link, rect, .. } = board.child("orphan", "nobody", Some(Side::Left)) else {
        panic!("a node");
    };
    assert_eq!(link, None, "no parent, no line");
    assert!(rect.w > 0);
}

#[test]
fn writing_fills_an_empty_cell_and_strikes_a_full_one() {
    let mut board = Builder::default();
    board.cells("arr", 3, false, Placement::Auto);
    let op = board.op();
    let write = board.board.place_write(MAIN, op, &id("arr.2"), "8");
    assert!(matches!(board.start(write), DrawOp::Write { .. }));
    let op = board.op();
    let again = board.board.place_write(MAIN, op, &id("arr.2"), "9");
    assert!(matches!(board.start(again), DrawOp::Strike { .. }));
    let op = board.op();
    let full = board.board.place_write(MAIN, op, &id("arr.0"), "5");
    assert!(matches!(board.start(full), DrawOp::Strike { .. }));
    let op = board.op();
    assert_eq!(
        board.board.place_write(MAIN, op, &id("arr.2"), "9"),
        Ok(Placed::Unchanged)
    );
}

/// Sequence diagrams, cells, highlights and trees export as Excalidraw elements: dashed
/// lifelines, one rectangle per cell with its value, a tinted highlight, a tree's link with no
/// arrowhead.
#[test]
fn diagrams_export_to_excalidraw() {
    let mut board = Builder::default();
    board.participant("p");
    board.participant("q");
    let placed = board.message("p", "q");
    board.start(placed);
    board.cells("arr", 3, false, Placement::Auto);
    let op = board.op();
    let target = id("arr.1");
    let highlight = board.board.place_annotation(
        MAIN,
        op,
        AnnotationRequest {
            target: &target,
            mark: Mark::Highlight,
            text: None,
            fill: None,
        },
    );
    board.start(highlight);
    board.node("root", Some(NodeShape::Circle), Placement::Auto);
    board.child("leaf", "root", Some(Side::Right));
    let text = export_layer(board.board.layer(MAIN).expect("main"));
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let elements = document["elements"].as_array().expect("elements");
    let find = |id: &str| {
        elements
            .iter()
            .find(|element| element["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {text}"))
    };
    assert_eq!(find("l0:p#lifeline")["strokeStyle"], "dashed");
    assert_eq!(find("l0:m:3")["type"], "arrow");
    assert_eq!(find("l0:arr.0")["type"], "rectangle");
    assert_eq!(find("l0:arr.0#value")["text"], "3");
    assert_eq!(find("l0:arr.2#index")["text"], "2");
    assert_eq!(find("l0:arr#label")["text"], "arr");
    assert_eq!(find("l0:a:5")["backgroundColor"], "#f7d3e0");
    assert_eq!(find("l0:root")["type"], "ellipse");
    assert_eq!(
        find("l0:root>leaf")["endArrowhead"],
        serde_json::Value::Null
    );
}

/// An op laid out and dropped by an interruption goes with everything it made: a row with its
/// cells, a tree node with the line from its parent.
#[test]
fn a_dropped_row_or_tree_node_leaves_nothing_behind() {
    let mut board = Builder::default();
    board.node("root", Some(NodeShape::Circle), Placement::Auto);
    let before: Vec<String> = board.areas().into_keys().collect();
    let op = board.op();
    let spec = CellsSpec {
        id: id("arr"),
        label: String::new(),
        values: vec!["1".into()],
        count: 3,
        index: IndexLabels::None,
        vertical: false,
        placement: Placement::Auto,
    };
    let row = board.board.place_cells(MAIN, op, &spec);
    assert!(matches!(row, Ok(Placed::New { .. })));
    board.board.remove_pending(MAIN, &id("arr"));
    let op = board.op();
    let leaf = id("leaf");
    let child = board.board.place_node(
        MAIN,
        op,
        NodeRequest {
            id: &leaf,
            label: "leaf",
            shape: None,
            placement: &Placement::Child {
                parent: id("root"),
                side: Some(Side::Left),
            },
            stack: 1,
            fill: None,
        },
    );
    assert!(matches!(child, Ok(Placed::New { .. })));
    board.board.remove_pending(MAIN, &leaf);
    let after: Vec<String> = board.areas().into_keys().collect();
    assert_eq!(after, before, "no cell, node or line was left behind");
}

/// A drawing placed beside something taken is moved just past it, with room for an arrow, and a
/// group's whole frame is kept for its members (a live run put a node on a group's frame).
#[test]
fn off_grid_drawings_keep_clear_with_room_between() {
    let mut board = Builder::default();
    board.cells("ht", 4, true, Placement::Auto);
    let first = board.node("k1", None, near("ht.1", Direction::Right));
    let second = board.node("k2", None, near("ht.1", Direction::Right));
    let (DrawOp::Node { rect: first, .. }, DrawOp::Node { rect: second, .. }) = (first, second)
    else {
        panic!("two nodes");
    };
    assert_eq!(
        second.center().y,
        first.center().y,
        "still lined up with the cell"
    );
    assert!(
        second.x >= first.right() + 48,
        "{second:?} is past {first:?} with room for an arrow"
    );
    let mut grouped = Builder::default();
    grouped.node("root", Some(NodeShape::Group), Placement::Auto);
    grouped.node("five", None, near("root", Direction::Right));
    let DrawOp::Node { rect: three, .. } =
        grouped.node("three", None, near("five", Direction::Left))
    else {
        panic!("a node");
    };
    let frame = grouped.areas()["root"].0;
    assert!(
        !three.intersects(frame),
        "{three:?} on the group's frame {frame:?}"
    );
}

/// Where a quadratic edge passes, at `steps` points along it.
fn curve_points(path: EdgePath, steps: u32) -> Vec<(f64, f64)> {
    let control = path.control.unwrap_or(Point {
        x: (path.start.x + path.end.x) / 2,
        y: (path.start.y + path.end.y) / 2,
    });
    (0..=steps)
        .map(|step| {
            let t = f64::from(step) / f64::from(steps);
            let along = |a: i32, b: i32, c: i32| {
                (1.0 - t) * (1.0 - t) * f64::from(a)
                    + 2.0 * (1.0 - t) * t * f64::from(b)
                    + t * t * f64::from(c)
            };
            (
                along(path.start.x, control.x, path.end.x),
                along(path.start.y, control.y, path.end.y),
            )
        })
        .collect()
}

/// A blocked edge's curve bows around what is in its way and clears it. A live hash table
/// (gemma4) chained `key1 → key2` past a wide box between them: the one fixed bow lifts a curve
/// least near its ends, so it ran through the box's corner, by the box beside its end.
#[test]
fn an_edge_bowing_around_a_node_clears_it() {
    let mut board = Builder::default();
    board.cells("index", 4, true, Placement::Auto);
    board.labelled("key1", "Key1", None, near("index.0", Direction::Right));
    board.labelled(
        "collision1",
        "Collision Detection",
        None,
        near("key1", Direction::Right),
    );
    board.labelled("key2", "Key2", None, near("key1", Direction::Right));
    let DrawOp::Edge { path, .. } = board.edge("key1", "key2") else {
        panic!("an edge");
    };
    assert!(path.control.is_some(), "the box between them is in the way");
    let boxed = board.areas()["collision1"].0;
    for (x, y) in curve_points(path, 64) {
        let inside = x > f64::from(boxed.x)
            && x < f64::from(boxed.right())
            && y > f64::from(boxed.y)
            && y < f64::from(boxed.bottom());
        assert!(
            !inside,
            "{path:?} runs through {boxed:?} at ({x:.0}, {y:.0})"
        );
    }
}
