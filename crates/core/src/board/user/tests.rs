//! Your shapes on the board: parsing, ids, pen strokes joining, labels and notes, undo, layout
//! keeping clear of them, the description, the summary and the export.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use super::{Drawn, Refused, Undone, describe};
use crate::board::excalidraw::export_layer;
use crate::board::summary::summary;
use crate::board::{Board, EdgeRequest, NodeRequest, Placed, Rejected};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{USER_COORDINATE_MAX, USER_STROKE_POINTS_MAX};
use crate::narration::ops::{Direction, Placement};
use crate::protocol::{DrawnShape, LineStyle, NodeShape, UserShape, UserShapeKind};
use crate::time::SessionTime;

const MAIN: LayerId = LayerId::MAIN;

fn id(text: &str) -> ElementId {
    ElementId::parse(text).expect("a valid id")
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

/// An ellipse-ish pen stroke around `area`, as a hand draws one.
fn loop_around(area: Rect) -> Vec<Point> {
    let (left, top, right, bottom) = (area.x, area.y, area.right(), area.bottom());
    vec![
        point(left, top),
        point(right, top),
        point(right, bottom),
        point(left, bottom),
        point(left, top + 2),
    ]
}

fn add(board: &mut Board, drawn: Drawn) -> UserShape {
    board.add_user_shape(MAIN, drawn, None).expect("kept")
}

/// Places and starts a node of BoardAI's.
fn node(board: &mut Board, op: u32, name: &str, placement: &Placement) -> Rect {
    let node_id = id(name);
    let request = NodeRequest {
        id: &node_id,
        label: name,
        shape: Some(NodeShape::Service),
        placement,
        stack: 1,
        fill: None,
    };
    let placed = board.place_node(MAIN, OpId(op), request).expect("placed");
    let Placed::New { id: placed_id, .. } = &placed else {
        panic!("a new node: {placed:?}");
    };
    let _ = board.start(
        MAIN,
        placed_id,
        SessionTime::from_ms(u64::from(op) * 500),
        300,
    );
    board
        .layer(MAIN)
        .and_then(|layer| layer.element(placed_id))
        .map(|element| element.geometry.bounds())
        .expect("drawn")
}

#[test]
fn shapes_are_parsed_at_the_boundary() {
    let too_many = vec![point(0, 0); USER_STROKE_POINTS_MAX + 1];
    assert_eq!(
        Drawn::parse(DrawnShape::Pen { points: too_many }),
        Err(Refused::TooManyPoints)
    );
    let far = USER_COORDINATE_MAX + 1;
    assert_eq!(
        Drawn::parse(DrawnShape::Arrow {
            start: point(0, 0),
            end: point(far, 0)
        }),
        Err(Refused::OutOfRange)
    );
    for click in [
        DrawnShape::Box {
            rect: rect(10, 10, 3, 40),
        },
        DrawnShape::Arrow {
            start: point(5, 5),
            end: point(7, 8),
        },
        DrawnShape::Pen {
            points: vec![point(1, 1)],
        },
    ] {
        assert_eq!(Drawn::parse(click), Err(Refused::TooSmall));
    }
    assert!(
        Drawn::parse(DrawnShape::Box {
            rect: rect(-50, 20, 120, 70)
        })
        .is_ok()
    );
}

#[test]
fn ids_count_up_and_skip_one_the_talker_used() {
    let mut board = Board::new();
    let first = add(&mut board, Drawn::Box(rect(0, 0, 100, 60)));
    assert_eq!(first.id.as_str(), "u1");
    node(&mut board, 1, "u2", &Placement::Auto);
    let next = add(&mut board, Drawn::Box(rect(0, 300, 100, 60)));
    assert_eq!(next.id.as_str(), "u3");
}

#[test]
fn a_stroke_over_the_last_pen_shape_joins_it_and_one_leaving_it_does_not() {
    let mut board = Board::new();
    let body = loop_around(rect(800, 160, 90, 90));
    let shape = add(&mut board, Drawn::Pen(body));
    let lid = vec![point(805, 172), point(845, 182), point(885, 172)];
    let joined = board
        .add_user_shape(MAIN, Drawn::Pen(lid), Some(&shape.id))
        .expect("kept");
    assert_eq!(joined.id, shape.id);
    let UserShapeKind::Ink { strokes, bounds } = &joined.kind else {
        panic!("a pen shape: {joined:?}");
    };
    assert_eq!(strokes.len(), 2);
    assert_eq!(*bounds, rect(800, 160, 90, 90));
    // A stroke that leaves the shape (an arrow drawn with the pen) is a shape of its own.
    let away = vec![point(880, 200), point(1100, 205)];
    let other = board
        .add_user_shape(MAIN, Drawn::Pen(away), Some(&shape.id))
        .expect("kept");
    assert_ne!(other.id, shape.id);
}

#[test]
fn the_first_words_label_a_shape_and_the_next_are_its_note() {
    let mut board = Board::new();
    let shape = add(&mut board, Drawn::Box(rect(0, 0, 100, 60)));
    let labelled = board
        .label_user_shape(MAIN, &shape.id, "Postgres")
        .expect("labelled");
    assert_eq!(labelled.label.as_deref(), Some("Postgres"));
    let noted = board
        .label_user_shape(MAIN, &shape.id, "source of truth")
        .expect("noted");
    assert_eq!(noted.label.as_deref(), Some("Postgres"));
    assert_eq!(noted.note.as_deref(), Some("source of truth"));
    // A node of BoardAI's takes no words of yours.
    node(&mut board, 1, "api", &Placement::Auto);
    assert!(board.label_user_shape(MAIN, &id("api"), "cache").is_none());
}

#[test]
fn undo_takes_a_pen_shapes_last_stroke_then_the_shape() {
    let mut board = Board::new();
    let body = loop_around(rect(0, 0, 90, 90));
    let shape = add(&mut board, Drawn::Pen(body));
    let lid = vec![point(5, 10), point(85, 12)];
    board
        .add_user_shape(MAIN, Drawn::Pen(lid), Some(&shape.id))
        .expect("joined");
    let Some(Undone::Changed(changed)) = board.undo_user_shape(MAIN, &shape.id) else {
        panic!("the lid goes first");
    };
    assert!(matches!(&changed.kind, UserShapeKind::Ink { strokes, .. } if strokes.len() == 1));
    assert_eq!(
        board.undo_user_shape(MAIN, &shape.id),
        Some(Undone::Removed)
    );
    assert!(
        board
            .layer(MAIN)
            .expect("main")
            .element(&shape.id)
            .is_none()
    );
    // Only your shapes undo.
    node(&mut board, 1, "api", &Placement::Auto);
    assert_eq!(board.undo_user_shape(MAIN, &id("api")), None);
}

#[test]
fn a_shape_an_arrow_joins_is_not_undone() {
    let mut board = Board::new();
    let client = add(&mut board, Drawn::Box(rect(0, 0, 100, 60)));
    let api = add(&mut board, Drawn::Box(rect(300, 0, 100, 60)));
    let arrow = add(
        &mut board,
        Drawn::Arrow {
            start: point(50, 30),
            end: point(310, 30),
        },
    );
    let UserShapeKind::Arrow { from, to, .. } = &arrow.kind else {
        panic!("an arrow: {arrow:?}");
    };
    assert_eq!(
        (from.as_ref(), to.as_ref()),
        (Some(&client.id), Some(&api.id))
    );
    assert_eq!(board.undo_user_shape(MAIN, &api.id), None);
    assert_eq!(
        board.undo_user_shape(MAIN, &arrow.id),
        Some(Undone::Removed)
    );
    assert_eq!(board.undo_user_shape(MAIN, &api.id), Some(Undone::Removed));
}

/// Layout treats your shapes as taken: the flow, a placement beside them and an edge to them all
/// keep clear, and nothing of BoardAI's may redraw, strike or write over one.
#[test]
fn layout_keeps_clear_of_your_shapes_and_never_redraws_them() {
    let mut board = Board::new();
    // Your box over the first two grid cells, where the flow would start.
    let yours = add(&mut board, Drawn::Box(rect(20, 20, 300, 100)));
    let first = node(&mut board, 1, "api", &Placement::Auto);
    assert!(!first.intersects(rect(20, 20, 300, 100)), "{first:?}");
    let beside = node(
        &mut board,
        2,
        "cache",
        &Placement::Near {
            target: yours.id.clone(),
            direction: Direction::Below,
        },
    );
    assert!(!beside.intersects(rect(20, 20, 300, 100)), "{beside:?}");
    assert!(beside.y > 120, "below your box: {beside:?}");
    let (from, to) = (id("cache"), yours.id.clone());
    let edge = EdgeRequest {
        from: &from,
        to: &to,
        label: None,
        style: LineStyle::Solid,
    };
    assert!(board.place_edge(MAIN, OpId(3), edge).is_ok());
    let redraw = NodeRequest {
        id: &yours.id,
        label: "API",
        shape: None,
        placement: &Placement::Auto,
        stack: 1,
        fill: None,
    };
    assert_eq!(
        board.place_node(MAIN, OpId(4), redraw),
        Err(Rejected::Yours(yours.id.clone()))
    );
    assert_eq!(
        board.place_strike(MAIN, OpId(5), &yours.id, "API"),
        Err(Rejected::Yours(yours.id.clone()))
    );
    assert_eq!(
        board.place_write(MAIN, OpId(6), &yours.id, "API"),
        Err(Rejected::Yours(yours.id))
    );
}

/// Off the grid too: a node placed beside one of your shapes is nudged past another of yours.
#[test]
fn a_node_beside_your_shape_is_nudged_past_your_next_one() {
    let mut board = Board::new();
    let first = add(&mut board, Drawn::Box(rect(0, 0, 100, 60)));
    // Where a node beside the first would land: 64 units to its right.
    add(&mut board, Drawn::Box(rect(164, 0, 120, 60)));
    let beside = node(
        &mut board,
        1,
        "api",
        &Placement::Near {
            target: first.id,
            direction: Direction::Right,
        },
    );
    assert!(!beside.intersects(rect(164, 0, 120, 60)), "{beside:?}");
    assert!(beside.x > 284, "past your second box: {beside:?}");
}

/// Artboard 13's drawing, described as the talker reads it.
fn artboard(board: &mut Board) {
    let client = add(board, Drawn::Box(rect(262, 246, 120, 72)));
    board.label_user_shape(MAIN, &client.id, "Client");
    add(
        board,
        Drawn::Arrow {
            start: point(384, 284),
            end: point(522, 282),
        },
    );
    let api = add(board, Drawn::Box(rect(530, 238, 142, 88)));
    board.label_user_shape(MAIN, &api.id, "API");
    add(
        board,
        Drawn::Arrow {
            start: point(674, 283),
            end: point(842, 283),
        },
    );
    let body = loop_around(rect(852, 236, 91, 95));
    let postgres = add(board, Drawn::Pen(body));
    let lid = vec![point(852, 246), point(897, 258), point(942, 246)];
    board
        .add_user_shape(MAIN, Drawn::Pen(lid), Some(&postgres.id))
        .expect("joined");
    board.label_user_shape(MAIN, &postgres.id, "Postgres");
    board.label_user_shape(MAIN, &postgres.id, "source of truth");
    let cache = add(board, Drawn::Box(rect(548, 430, 116, 60)));
    board.label_user_shape(MAIN, &cache.id, "cache");
}

#[test]
fn the_description_follows_the_arrows_and_places_the_rest() {
    let mut board = Board::new();
    artboard(&mut board);
    let layer = board.layer(MAIN).expect("main");
    assert_eq!(
        describe(layer).as_deref(),
        Some(
            "u1 \"Client\" → u3 \"API\" → u5 \"Postgres\" (source of truth); u6 \"cache\" under \
             u3, not connected yet"
        )
    );
    assert_eq!(describe(Board::new().layer(MAIN).expect("main")), None);
}

#[test]
fn loose_arrows_and_unlabelled_shapes_are_described_too() {
    let mut board = Board::new();
    add(&mut board, Drawn::Box(rect(0, 0, 100, 60)));
    add(
        &mut board,
        Drawn::Arrow {
            start: point(50, 30),
            end: point(400, 30),
        },
    );
    add(
        &mut board,
        Drawn::Arrow {
            start: point(0, 400),
            end: point(0, 600),
        },
    );
    let layer = board.layer(MAIN).expect("main");
    assert_eq!(
        describe(layer).as_deref(),
        Some(
            "u1 (a box, no label); u2 → from u1 pointing right, to nothing yet; u3 → pointing \
             down, joining nothing"
        )
    );
}

#[test]
fn the_summary_and_the_export_carry_your_drawing() {
    let mut board = Board::new();
    artboard(&mut board);
    let text = summary(&board);
    assert!(
        text.starts_with("main: drawn by the user: u1 \"Client\" → u3 \"API\""),
        "{text}"
    );
    let export = export_layer(board.layer(MAIN).expect("main"));
    let json: serde_json::Value = serde_json::from_str(&export).expect("valid JSON");
    let elements = json["elements"].as_array().expect("elements");
    let yours = elements
        .iter()
        .filter(|element| element["strokeColor"] == "#5b5bd6")
        .count();
    assert!(yours >= 10, "{export}");
    let freehand = elements
        .iter()
        .filter(|element| element["type"] == "freedraw")
        .count();
    assert_eq!(freehand, 2, "the pen shape's two strokes");
    let bound = elements
        .iter()
        .filter(|element| {
            element["type"] == "arrow" && element["endBinding"]["elementId"] == "l0:u3"
        })
        .count();
    assert_eq!(bound, 1, "the first arrow is bound to the API box");
}
