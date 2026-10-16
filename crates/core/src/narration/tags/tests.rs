//! The tag grammar: every tag's minimal form, bad values, defaults and synonyms.

use super::{TAG_NAMES, bounded, to_op};
use crate::narration::ops::{
    AxisRef, CellsSpec, Direction, IndexLabels, NarrationOp, Placement, Side,
};
use crate::narration::parser::RawTag;
use crate::protocol::{CellMark, Fill, LineStyle, Mark, NodeShape};

fn tag(name: &str, attributes: &[(&str, &str)]) -> RawTag {
    RawTag {
        name: name.to_owned(),
        attributes: attributes
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        written: String::new(),
    }
}

#[test]
fn a_minimal_valid_form_of_every_tag_parses() {
    let minimal: &[(&str, &[(&str, &str)])] = &[
        ("plan", &[("steps", "Client|Edge|LB")]),
        ("step", &[("n", "2")]),
        ("node", &[("id", "lb")]),
        ("edge", &[("from", "cdn"), ("to", "lb")]),
        ("annotate", &[("target", "lb")]),
        ("strike", &[("target", "sqs"), ("text", "Kafka")]),
        ("focus", &[("target", "lb")]),
        ("detour", &[("title", "cache-aside")]),
        ("resume", &[]),
        ("code", &[("path", "src/lb.rs"), ("lines", "4-9")]),
        ("line", &[("n", "42")]),
        ("trace", &[("path", "api>redis>db")]),
        (
            "compare",
            &[("options", "Redis|Memcached"), ("criteria", "Speed")],
        ),
        (
            "cell",
            &[("option", "0"), ("criterion", "Speed"), ("mark", "yes")],
        ),
        ("close", &[("surface", "code")]),
        (
            "recap",
            &[("bullets", "Reads try Redis first|A miss reads Postgres")],
        ),
        ("participant", &[("id", "client")]),
        ("message", &[("from", "client"), ("to", "server")]),
        ("cells", &[("id", "arr"), ("count", "4")]),
        ("write", &[("target", "arr.0"), ("text", "7")]),
    ];
    assert_eq!(minimal.len(), TAG_NAMES.len());
    for ((name, attributes), expected) in minimal.iter().zip(TAG_NAMES) {
        assert_eq!(name, expected, "the table follows TAG_NAMES");
        assert!(to_op(&tag(name, attributes)).is_ok(), "{name}");
    }
}

#[test]
fn bad_values_and_missing_attributes_drop_the_tag() {
    let bad: &[(&str, &[(&str, &str)])] = &[
        ("node", &[("id", "has space")]),
        ("edge", &[("from", "a")]),
        ("edge", &[("from", "a"), ("to", "a")]),
        ("step", &[("n", "0")]),
        ("line", &[("n", "x")]),
        ("code", &[("path", "a.go"), ("lines", "9-4")]),
        ("trace", &[("path", "only")]),
        ("cell", &[("row", "0"), ("col", "0")]),
        ("cell", &[("option", "Redis"), ("mark", "yes")]),
        ("close", &[("surface", "window")]),
        ("plan", &[("steps", " | ")]),
        ("annotate", &[("line", "0"), ("code", "x")]),
        ("recap", &[("bullets", "|")]),
        ("participant", &[("label", "Client")]),
        ("message", &[("from", "a")]),
        ("cells", &[("id", "arr")]),
        ("cells", &[("id", "arr"), ("count", "many")]),
        ("write", &[("target", "arr.0")]),
    ];
    for (name, attributes) in bad {
        assert!(
            to_op(&tag(name, attributes)).is_err(),
            "{name} {attributes:?}"
        );
    }
}

#[test]
fn node_placement_and_defaults() {
    let op = to_op(&tag(
        "node",
        &[
            ("id", "api"),
            ("near", "lb"),
            ("stack", "3"),
            ("color", "red"),
        ],
    ));
    assert_eq!(
        op,
        Ok(NarrationOp::Node {
            id: crate::ids::ElementId::parse("api").expect("valid"),
            label: "api".to_owned(),
            shape: None,
            placement: Placement::Near {
                target: crate::ids::ElementId::parse("lb").expect("valid"),
                direction: Direction::Right,
            },
            stack: 3,
            fill: None,
        })
    );
}

/// A near miss in an optional value keeps the drawing: synonyms map, unknown values default.
#[test]
fn unknown_optional_values_fall_back_to_their_defaults() {
    let shape = |kind: &str| match to_op(&tag("node", &[("id", "a"), ("kind", kind)])) {
        Ok(NarrationOp::Node { shape, .. }) => shape,
        Ok(_) | Err(_) => None,
    };
    assert_eq!(shape("Database"), Some(NodeShape::Db));
    assert_eq!(shape("cache"), Some(NodeShape::Db));
    assert_eq!(shape("topic"), Some(NodeShape::Queue));
    assert_eq!(shape("browser"), Some(NodeShape::Actor));
    assert_eq!(shape("cluster"), Some(NodeShape::Group));
    assert_eq!(shape("spaceship"), Some(NodeShape::Service));
    assert_eq!(shape("State"), Some(NodeShape::Circle));
    let unparsable = to_op(&tag(
        "node",
        &[("id", "a"), ("near", "crates/app"), ("parent", "not an id")],
    ));
    assert!(
        matches!(
            unparsable,
            Ok(NarrationOp::Node {
                placement: Placement::Auto,
                ..
            })
        ),
        "a hint that can't be an id continues the flow: {unparsable:?}"
    );
    let placed = to_op(&tag(
        "node",
        &[("id", "a"), ("near", "b"), ("dir", "diagonal")],
    ));
    assert!(matches!(
        placed,
        Ok(NarrationOp::Node {
            placement: Placement::Near {
                direction: Direction::Right,
                ..
            },
            ..
        })
    ));
    let edge = to_op(&tag(
        "edge",
        &[("from", "a"), ("to", "b"), ("style", "dull")],
    ));
    assert!(matches!(
        edge,
        Ok(NarrationOp::Edge {
            style: LineStyle::Solid,
            ..
        })
    ));
    let annotate = to_op(&tag("annotate", &[("target", "a"), ("mark", "zigzag")]));
    assert!(matches!(
        annotate,
        Ok(NarrationOp::Annotate {
            mark: Mark::Circle,
            ..
        })
    ));
    let code = to_op(&tag("annotate", &[("line", "4"), ("mark", "zigzag")]));
    assert!(matches!(
        code,
        Ok(NarrationOp::MarkCode {
            mark: Mark::Underline,
            ..
        })
    ));
}

#[test]
fn node_tones_compare_titles_cells_and_code_marks() {
    let node = to_op(&tag(
        "node",
        &[("id", "q"), ("kind", "queue"), ("fill", "Blush")],
    ));
    assert!(matches!(
        node,
        Ok(NarrationOp::Node {
            fill: Some(Fill::Blush),
            ..
        })
    ));
    let unknown = to_op(&tag("node", &[("id", "q"), ("fill", "plaid")]));
    assert!(matches!(unknown, Ok(NarrationOp::Node { fill: None, .. })));
    let compare = to_op(&tag(
        "compare",
        &[
            ("title", "Raft vs Paxos"),
            ("options", "Raft|Paxos"),
            ("criteria", "Easy"),
        ],
    ));
    assert!(matches!(
        compare,
        Ok(NarrationOp::Compare { title: Some(ref title), .. }) if title == "Raft vs Paxos"
    ));
    let by_row = to_op(&tag(
        "cell",
        &[("row", "Easy"), ("col", "Raft"), ("mark", "no")],
    ));
    assert_eq!(
        by_row,
        Ok(NarrationOp::Cell {
            criterion: AxisRef::Name("Easy".into()),
            option: AxisRef::Name("Raft".into()),
            mark: CellMark::No,
        })
    );
    let underline = to_op(&tag(
        "annotate",
        &[("line", "43"), ("code", "s.cache.Get(")],
    ));
    assert_eq!(
        underline,
        Ok(NarrationOp::MarkCode {
            line: 43,
            code: Some("s.cache.Get(".into()),
            mark: Mark::Underline,
        })
    );
    let circle = to_op(&tag("annotate", &[("line", "44"), ("mark", "circle")]));
    assert_eq!(
        circle,
        Ok(NarrationOp::MarkCode {
            line: 44,
            code: None,
            mark: Mark::Circle,
        })
    );
}

/// Sequence diagrams, cells, trees and highlights (D-150).
#[test]
fn diagram_tags_parse_with_their_defaults() {
    let id = |text: &str| crate::ids::ElementId::parse(text).expect("valid");
    assert_eq!(
        to_op(&tag(
            "message",
            &[
                ("from", "c"),
                ("to", "c"),
                ("style", "return"),
                ("label", "retry")
            ]
        )),
        Ok(NarrationOp::Message {
            from: id("c"),
            to: id("c"),
            label: Some("retry".into()),
            style: LineStyle::Dashed,
        }),
        "a message to itself is allowed, and a return is dashed"
    );
    let cells = to_op(&tag(
        "cells",
        &[
            ("id", "arr"),
            ("values", " 3 |1||4"),
            ("count", "6"),
            ("label", "nums"),
            ("vertical", "yes"),
            ("near", "head"),
            ("dir", "below"),
        ],
    ));
    assert_eq!(
        cells,
        Ok(NarrationOp::Cells(CellsSpec {
            id: id("arr"),
            label: "nums".into(),
            values: vec!["3".into(), "1".into(), String::new(), "4".into()],
            count: 6,
            index: IndexLabels::From(0),
            vertical: true,
            placement: Placement::Near {
                target: id("head"),
                direction: Direction::Below,
            },
        }))
    );
}

/// Index labels, a count past the cap, tree children and highlights (D-150).
#[test]
fn cells_indexes_tree_children_and_highlights_parse() {
    let index = |value: &str| match to_op(&tag(
        "cells",
        &[("id", "a"), ("values", "x|y"), ("index", value)],
    )) {
        Ok(NarrationOp::Cells(spec)) => Some((spec.index, spec.count)),
        Ok(_) | Err(_) => None,
    };
    assert_eq!(index("none"), Some((IndexLabels::None, 2)));
    assert_eq!(index("1"), Some((IndexLabels::From(1), 2)));
    assert_eq!(
        index("0x10|0x14"),
        Some((IndexLabels::Named(vec!["0x10".into(), "0x14".into()]), 2))
    );
    let count = to_op(&tag("cells", &[("id", "a"), ("count", "500")]));
    assert!(
        matches!(count, Ok(NarrationOp::Cells(ref spec)) if usize::from(spec.count) == crate::limits::CELLS_PER_STRIP_MAX)
    );
    let child = to_op(&tag(
        "node",
        &[
            ("id", "n3"),
            ("label", "3"),
            ("parent", "n5"),
            ("side", "Left"),
        ],
    ));
    assert!(matches!(
        child,
        Ok(NarrationOp::Node {
            shape: None,
            placement: Placement::Child {
                side: Some(Side::Left),
                ..
            },
            ..
        })
    ));
    let highlight = to_op(&tag(
        "annotate",
        &[("target", "arr.2"), ("mark", "highlight"), ("fill", "mint")],
    ));
    assert!(matches!(
        highlight,
        Ok(NarrationOp::Annotate {
            mark: Mark::Highlight,
            fill: Some(Fill::Mint),
            ..
        })
    ));
    let pen = to_op(&tag("annotate", &[("target", "a"), ("fill", "mint")]));
    assert!(
        matches!(pen, Ok(NarrationOp::Annotate { fill: None, .. })),
        "a pen mark has no tone"
    );
}

#[test]
fn long_text_is_cut_with_an_ellipsis() {
    let cut = bounded(&"é".repeat(50), 11);
    assert!(cut.len() <= 11, "{cut}");
    assert!(cut.ends_with('…'));
}
