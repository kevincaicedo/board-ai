//! Board model, layout, summary and Excalidraw export (LLD §2.5, invariant I6).
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use std::collections::{BTreeMap, BTreeSet};

use boardai_core::board::excalidraw::export_layer;
use boardai_core::board::summary::summary;
use boardai_core::board::{
    AnnotationRequest, Board, EdgeRequest, ElementState, NodeRequest, Placed,
};
use boardai_core::geometry::Rect;
use boardai_core::ids::{ElementId, LayerId, OpId};
use boardai_core::narration::ops::{Direction, Placement};
use boardai_core::protocol::{LineStyle, Mark, NodeShape};
use boardai_core::time::SessionTime;

fn id(text: &str) -> ElementId {
    ElementId::parse(text).expect("valid id")
}

struct Builder {
    board: Board,
    next_op: u32,
    now: u64,
}

impl Builder {
    fn new() -> Self {
        Self {
            board: Board::new(),
            next_op: 0,
            now: 0,
        }
    }

    fn op(&mut self) -> OpId {
        self.next_op += 1;
        OpId(self.next_op)
    }

    fn start(&mut self, placed: Placed) -> Placed {
        self.now += 500;
        if let Placed::New { id, .. } = &placed {
            let _ = self
                .board
                .start(LayerId::MAIN, id, SessionTime::from_ms(self.now), 300);
        }
        placed
    }

    fn node(&mut self, name: &str, label: &str, shape: NodeShape, placement: Placement) -> Placed {
        let op = self.op();
        let node_id = id(name);
        let request = NodeRequest {
            id: &node_id,
            label,
            shape: Some(shape),
            placement: &placement,
            stack: 1,
            fill: None,
        };
        let placed = self
            .board
            .place_node(LayerId::MAIN, op, request)
            .expect("placed");
        self.start(placed)
    }

    fn edge(&mut self, from: &str, to: &str, label: Option<&str>) -> Placed {
        let op = self.op();
        let (from, to) = (id(from), id(to));
        let request = EdgeRequest {
            from: &from,
            to: &to,
            label,
            style: LineStyle::Solid,
        };
        let placed = self
            .board
            .place_edge(LayerId::MAIN, op, request)
            .expect("placed");
        self.start(placed)
    }

    fn rects(&self) -> BTreeMap<String, Rect> {
        self.board
            .layer(LayerId::MAIN)
            .expect("main")
            .elements()
            .iter()
            .map(|element| (element.id.as_str().to_owned(), element.geometry.bounds()))
            .collect()
    }
}

fn near(target: &str, direction: Direction) -> Placement {
    Placement::Near {
        target: id(target),
        direction,
    }
}

/// I6: placing new elements, collisions included, never moves one already placed; groups only
/// grow; nodes never overlap.
#[test]
fn layout_never_moves_an_existing_element() {
    let mut board = Builder::new();
    board.node("client", "Browser", NodeShape::Actor, Placement::Auto);
    let mut before = board.rects();
    let steps: Vec<(&str, Placement, NodeShape)> = vec![
        ("cdn", near("client", Direction::Right), NodeShape::Service),
        ("lb", near("client", Direction::Right), NodeShape::Service),
        ("api", near("lb", Direction::Right), NodeShape::Service),
        ("cache", near("api", Direction::Below), NodeShape::Db),
        ("auth", near("api", Direction::Below), NodeShape::Service),
        ("dns", near("client", Direction::Above), NodeShape::Service),
        ("vpc", near("cache", Direction::Below), NodeShape::Group),
        (
            "db",
            Placement::Inside {
                group: id("vpc"),
                near: None,
            },
            NodeShape::Db,
        ),
        (
            "replica",
            Placement::Inside {
                group: id("vpc"),
                near: Some((id("db"), Direction::Right)),
            },
            NodeShape::Db,
        ),
        ("queue", Placement::Auto, NodeShape::Queue),
    ];
    for (name, placement, shape) in steps {
        board.node(name, &format!("{name} with a long label"), shape, placement);
        let after = board.rects();
        for (element, rect) in &before {
            let now = after[element];
            if element == "vpc" {
                assert_eq!(rect.union(now), now, "the group only grows");
            } else {
                assert_eq!(*rect, now, "{element} moved when {name} was placed");
            }
        }
        before = after;
    }
    let layer = board.board.layer(LayerId::MAIN).expect("main");
    let boxes: Vec<(&str, Rect)> = layer
        .elements()
        .iter()
        .filter(|element| element.id.as_str() != "vpc")
        .map(|element| (element.id.as_str(), element.geometry.bounds()))
        .collect();
    for (index, (a, first)) in boxes.iter().enumerate() {
        for (b, second) in &boxes[index + 1..] {
            assert!(!first.intersects(*second), "{a} overlaps {b}");
        }
    }
    let vpc = before["vpc"];
    for child in ["db", "replica"] {
        assert_eq!(vpc.union(before[child]), vpc, "{child} is inside its group");
    }
}

#[test]
fn near_right_takes_the_next_cell_and_a_collision_nudges_further() {
    let mut board = Builder::new();
    board.node("client", "Client", NodeShape::Service, Placement::Auto);
    board.node(
        "cdn",
        "CDN",
        NodeShape::Service,
        near("client", Direction::Right),
    );
    board.node(
        "lb",
        "LB",
        NodeShape::Service,
        near("client", Direction::Right),
    );
    let rects = board.rects();
    assert!(rects["cdn"].x > rects["client"].right());
    assert!(rects["lb"].x > rects["cdn"].right(), "nudged past cdn");
    assert_eq!(
        rects["lb"].center().y,
        rects["client"].center().y,
        "same row"
    );
}

#[test]
fn a_repeated_id_strikes_or_does_nothing_and_never_duplicates() {
    let mut board = Builder::new();
    board.node("queue", "SQS", NodeShape::Queue, Placement::Auto);
    let op = OpId(99);
    let queue = id("queue");
    let same = NodeRequest {
        id: &queue,
        label: "SQS",
        shape: Some(NodeShape::Queue),
        placement: &Placement::Auto,
        stack: 1,
        fill: None,
    };
    assert_eq!(
        board.board.place_node(LayerId::MAIN, op, same),
        Ok(Placed::Unchanged)
    );
    let renamed = NodeRequest {
        label: "Kafka",
        ..same
    };
    let placed = board
        .board
        .place_node(LayerId::MAIN, op, renamed)
        .expect("placed");
    assert!(matches!(placed, Placed::Strike { .. }));
    board.board.apply_strike(LayerId::MAIN, &queue, "Kafka");
    let layer = board.board.layer(LayerId::MAIN).expect("main");
    assert_eq!(layer.elements().len(), 1);
    assert_eq!(layer.elements()[0].label, "Kafka");
    assert_eq!(layer.elements()[0].state, ElementState::Struck);
}

#[test]
fn an_edge_to_an_unknown_node_is_rejected() {
    let mut board = Builder::new();
    board.node("a", "A", NodeShape::Service, Placement::Auto);
    let (from, to) = (id("a"), id("ghost"));
    let request = EdgeRequest {
        from: &from,
        to: &to,
        label: None,
        style: LineStyle::Solid,
    };
    assert!(
        board
            .board
            .place_edge(LayerId::MAIN, OpId(5), request)
            .is_err()
    );
}

/// A placement hint that names nothing on the layer (the main board's id inside a detour) never
/// loses the node: it continues the flow. `in` naming a plain node places it beside that node.
#[test]
fn a_placement_hint_naming_nothing_continues_the_flow() {
    let mut board = Builder::new();
    board.node("a", "A", NodeShape::Service, Placement::Auto);
    board.node(
        "b",
        "B",
        NodeShape::Service,
        near("ghost", Direction::Below),
    );
    board.node(
        "c",
        "C",
        NodeShape::Service,
        Placement::Inside {
            group: id("nowhere"),
            near: None,
        },
    );
    board.node(
        "d",
        "D",
        NodeShape::Service,
        Placement::Inside {
            group: id("a"),
            near: None,
        },
    );
    let rects = board.rects();
    let (a, b, c, d) = (rects["a"], rects["b"], rects["c"], rects["d"]);
    assert!(
        a.right() < b.x && b.right() < c.x,
        "the flow continues right: {rects:?}"
    );
    assert_eq!((a.y, b.y, c.y), (a.y, a.y, a.y));
    assert!(
        d.y == a.y || d.x > a.right(),
        "beside its non-group target: {rects:?}"
    );
    let all: Vec<Rect> = rects.values().copied().collect();
    for (index, first) in all.iter().enumerate() {
        for second in &all[index + 1..] {
            assert!(!first.intersects(*second), "{rects:?}");
        }
    }
}

#[test]
fn the_summary_reads_as_chains() {
    let mut board = Builder::new();
    board.node("client", "Browser", NodeShape::Actor, Placement::Auto);
    board.node(
        "cdn",
        "CDN",
        NodeShape::Service,
        near("client", Direction::Right),
    );
    board.edge("client", "cdn", None);
    board.node(
        "lb",
        "lb",
        NodeShape::Service,
        near("cdn", Direction::Right),
    );
    board.edge("cdn", "lb", Some("https"));
    let detour = board.board.push_layer("cache-aside").expect("a layer");
    assert_eq!(detour, LayerId(1));
    assert_eq!(
        summary(&board.board),
        "main: client(actor) \"Browser\" → cdn -[https]→ lb\ndetour 1 \"cache-aside\": empty"
    );
    board.board.close_layer(detour);
    assert!(!summary(&board.board).contains("detour"));
}

/// The export is a valid `.excalidraw` v2 document with hand-drawn elements in Excalifont,
/// stable seeds, and bindings that point at real elements.
#[test]
fn the_excalidraw_export_has_the_expected_structure() {
    let mut board = Builder::new();
    board.node("client", "Browser", NodeShape::Actor, Placement::Auto);
    board.node(
        "cdn",
        "CDN",
        NodeShape::Service,
        near("client", Direction::Right),
    );
    board.edge("client", "cdn", Some("https"));
    board.node(
        "vpc",
        "VPC",
        NodeShape::Group,
        near("cdn", Direction::Below),
    );
    board.node(
        "db",
        "Postgres",
        NodeShape::Db,
        Placement::Inside {
            group: id("vpc"),
            near: None,
        },
    );
    let op = OpId(50);
    let placed = board
        .board
        .place_annotation(
            LayerId::MAIN,
            op,
            AnnotationRequest {
                target: &id("cdn"),
                mark: Mark::Circle,
                text: Some("hot"),
                fill: None,
            },
        )
        .expect("placed");
    board.start(placed);
    let layer = board.board.layer(LayerId::MAIN).expect("main");
    let text = export_layer(layer);
    assert_eq!(text, export_layer(layer), "deterministic");
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(document["type"], "excalidraw");
    assert_eq!(document["version"], 2);
    assert!(document["appState"].is_object() && document["files"].is_object());
    let elements = document["elements"].as_array().expect("elements");
    let ids: BTreeSet<&str> = elements
        .iter()
        .filter_map(|element| element["id"].as_str())
        .collect();
    assert_eq!(ids.len(), elements.len(), "unique ids");
    for element in elements {
        check_element(element, &ids);
    }
    let kinds: BTreeSet<&str> = elements
        .iter()
        .filter_map(|element| element["type"].as_str())
        .collect();
    for kind in ["rectangle", "ellipse", "arrow", "text"] {
        assert!(kinds.contains(kind), "{kind} present");
    }
}

/// One exported element: an allowed type, hand-drawn, seeded, complete, with bindings that
/// point at real elements.
fn check_element(element: &serde_json::Value, ids: &BTreeSet<&str>) {
    let allowed = ["rectangle", "ellipse", "arrow", "line", "text"];
    let kind = element["type"].as_str().expect("type");
    assert!(allowed.contains(&kind), "{kind}");
    assert_eq!(element["roughness"], 1);
    assert!(element["seed"].as_u64().is_some_and(|seed| seed > 0));
    for key in [
        "x",
        "y",
        "width",
        "height",
        "strokeColor",
        "version",
        "isDeleted",
    ] {
        assert!(!element[key].is_null(), "{kind} has {key}");
    }
    if kind == "text" {
        assert_eq!(element["fontFamily"], 5, "Excalifont");
        if let Some(container) = element["containerId"].as_str() {
            assert!(ids.contains(container), "text bound to {container}");
        }
    }
    if kind == "arrow" {
        for end in ["startBinding", "endBinding"] {
            if let Some(target) = element[end]["elementId"].as_str() {
                assert!(ids.contains(target), "arrow bound to {target}");
            }
        }
    }
}
