//! Simulator scenarios for the MVP diagram types (PRODUCT §7.1, D-120): a sequence diagram whose
//! lifelines grow as messages arrive, an array filled and marked cell by cell, a binary tree
//! growing, and a hash table with a collision chain. Every run also holds I1 (nothing drawn
//! before its word), I6 (nothing drawn moves; a lifeline only grows from where it ended) and I11
//! (every stroke inside the camera frame), which the simulator checks on every op.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::board::summary::summary;
use boardai_core::geometry::{Point, Rect};
use boardai_core::history::{Message, UserPart};
use boardai_core::ids::{ElementId, LayerId};
use boardai_core::input::Input;
use boardai_core::protocol::{
    CellBox, DrawOp, Fill, LineStyle, Mark, NodeShape, PointerTarget, UiCommand, UiEvent,
};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{assert_clean, config, dropped};

fn ops(sim: &Sim) -> Vec<DrawOp> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart { op, .. } => Some(op.clone()),
            _ => None,
        })
        .collect()
}

fn run(script: &str, question: &str) -> Sim {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(script));
    sim.user_says(0, question);
    sim.run_until(60_000);
    sim
}

/// The TCP handshake and a teardown: two participants, seven messages including a reply and a
/// message to itself. Heads stand in one row, messages go down in order, and the lifelines grow
/// in stretches that each start where the line ended (checked by the simulator, I6).
#[test]
fn a_sequence_diagram_grows_down_and_its_lifelines_follow() {
    let sim = run(
        concat!(
            r#"The <participant id="c" label="Client"/>client and the "#,
            r#"<participant id="s" label="Server"/>server shake hands. The client "#,
            r#"<message from="c" to="s" label="SYN"/>sends a SYN, the server "#,
            r#"<message from="s" to="c" label="SYN-ACK" style="return"/>answers, and the "#,
            r#"client <message from="c" to="s" label="ACK"/>acknowledges. Data "#,
            r#"<message from="c" to="s" label="data"/>flows, the server "#,
            r#"<message from="s" to="s" label="process"/>works on it, then "#,
            r#"<message from="s" to="c" label="FIN" style="return"/>closes, and the client "#,
            r#"<edge from="c" to="s" label="ACK"/>acknowledges once more."#,
        ),
        "Explain the TCP handshake as a sequence diagram",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let ops = ops(&sim);
    let heads: Vec<(Rect, i32)> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Participant {
                rect,
                lifeline_bottom,
                ..
            } => Some((*rect, *lifeline_bottom)),
            _ => None,
        })
        .collect();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads[0].0.y, heads[1].0.y, "participants stand in one row");
    assert!(heads[1].0.x > heads[0].0.right(), "left to right, apart");
    let messages: Vec<(&Vec<Point>, LineStyle, usize)> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Message {
                points,
                style,
                extend,
                ..
            } => Some((points, *style, extend.len())),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages.len(),
        7,
        "an edge between participants is a message"
    );
    let rows: Vec<i32> = messages.iter().map(|(points, ..)| points[0].y).collect();
    assert!(rows.windows(2).all(|pair| pair[1] > pair[0]), "{rows:?}");
    assert_eq!(messages[1].1, LineStyle::Dashed, "a return is dashed");
    assert_eq!(messages[4].0.len(), 4, "a message to itself loops");
    assert!(
        messages.iter().any(|(_, _, stretches)| *stretches == 2),
        "the lifelines grew together"
    );
    let left_x = heads[0].0.center().x;
    assert_eq!(messages[0].0[0].x, left_x, "messages start on the lifeline");
    let board = summary(sim.core().board());
    assert!(
        board.contains("sequence c \"Client\", s \"Server\": c -[SYN]→ s"),
        "{board}"
    );
}

/// An array drawn empty, filled one value at a time, one cell highlighted and one corrected.
#[test]
fn an_array_is_filled_and_a_cell_highlighted_and_struck() {
    let sim = run(
        concat!(
            r#"An <cells id="arr" count="5" label="nums"/>array of five slots starts empty. "#,
            r#"We <write target="arr.0" text="3"/>store three, then "#,
            r#"<write target="arr.1" text="1"/>one, then <write target="arr.2" text="4"/>four. "#,
            r#"Now <annotate target="arr.1" mark="highlight" text="i"/>slot one is the "#,
            r#"cursor, and we <strike target="arr.2" text="9"/>overwrite four with nine, and "#,
            r#"<write target="arr.0" text="5"/>five replaces three."#,
        ),
        "How does an array get filled?",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let ops = ops(&sim);
    let Some(DrawOp::Cells { cells, .. }) =
        ops.iter().find(|op| matches!(op, DrawOp::Cells { .. }))
    else {
        panic!("a row of cells: {ops:#?}");
    };
    assert_eq!(cells.len(), 5);
    assert!(cells.iter().all(|cell| cell.value.is_empty()));
    assert_eq!(cells[2].id.as_str(), "arr.2");
    assert_eq!(cells[0].index.as_deref(), Some("0"));
    assert!(
        cells
            .windows(2)
            .all(|pair| pair[0].rect.right() == pair[1].rect.x)
    );
    let writes: Vec<(&str, Rect)> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Write { target, rect, .. } => Some((target.as_str(), *rect)),
            _ => None,
        })
        .collect();
    assert_eq!(
        writes.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
        ["arr.0", "arr.1", "arr.2"],
        "an empty cell is written into"
    );
    assert_eq!(writes[0].1, cells[0].rect, "inside its own cell");
    assert!(ops.iter().any(|op| matches!(
        op,
        DrawOp::Annotate { mark: Mark::Highlight, fill: Some(Fill::Blush), rect, .. } if *rect == cells[1].rect
    )));
    let struck: Vec<&str> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Strike { target, label, .. } => Some((target.as_str(), label.as_str())),
            _ => None,
        })
        .map(|(target, _)| target)
        .collect();
    assert_eq!(struck, ["arr.2", "arr.0"], "a full cell is struck");
    let board = summary(sim.core().board());
    assert!(board.contains("arr[5|1|9|_|_] \"nums\""), "{board}");
}

/// A node's id, box, and the ends of the line from its parent.
type NodeBox = (String, Rect, Option<(Point, Point)>);

fn node_rects(ops: &[DrawOp]) -> Vec<NodeBox> {
    ops.iter()
        .filter_map(|op| match op {
            DrawOp::Node { id, rect, link, .. } => Some((
                id.as_str().to_owned(),
                *rect,
                link.map(|link| (link.start, link.end)),
            )),
            _ => None,
        })
        .collect()
}

/// A binary search tree built as 5, 3, 8, 1, 4, 9 are inserted: each child under its parent on
/// its side, joined by a line from the parent's bottom to its top, and no two nodes touching.
#[test]
fn a_binary_tree_grows_top_down_without_moving_anything() {
    let sim = run(
        concat!(
            r#"Insert <node id="n5" label="5" kind="circle"/>five as the root, "#,
            r#"<node id="n3" label="3" parent="n5" side="left"/>three to its left, "#,
            r#"<node id="n8" label="8" parent="n5" side="right"/>eight to its right, "#,
            r#"<node id="n1" label="1" parent="n3" side="left"/>one under three, "#,
            r#"<node id="n4" label="4" parent="n3" side="right"/>four beside it, and "#,
            r#"<node id="n9" label="9" parent="n8" side="right"/>nine under eight. "#,
            r#"Searching for four <trace path="n5>n3>n4"/>goes left, then right."#,
        ),
        "Draw a binary search tree as I insert 5, 3, 8, 1",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let ops = ops(&sim);
    let nodes = node_rects(&ops);
    assert_eq!(nodes.len(), 6);
    let rect = |name: &str| {
        nodes
            .iter()
            .find(|(id, ..)| id == name)
            .map(|(_, rect, _)| *rect)
            .expect("drawn")
    };
    assert_binary_layout(&nodes);
    let (_, child, link) = nodes.iter().find(|(id, ..)| id == "n4").expect("n4");
    let parent = rect("n3");
    assert_eq!(
        *link,
        Some((
            Point {
                x: parent.center().x,
                y: parent.bottom()
            },
            Point {
                x: child.center().x,
                y: child.y
            }
        )),
        "the line runs from the parent's bottom to the child's top"
    );
    assert!(
        ops.iter()
            .all(|op| !matches!(op, DrawOp::Node { shape, .. } if *shape != NodeShape::Circle)),
        "children take their parent's kind"
    );
    assert!(
        ops.iter().any(|op| matches!(op, DrawOp::Trace { .. })),
        "a trace runs along the tree's links"
    );
}

/// Each child under its parent on its side, a spread halving at each level, and no two nodes
/// touching.
fn assert_binary_layout(nodes: &[NodeBox]) {
    let rect = |name: &str| {
        nodes
            .iter()
            .find(|(id, ..)| id == name)
            .map(|(_, rect, _)| *rect)
            .expect("drawn")
    };
    for (child, parent, left) in [
        ("n3", "n5", true),
        ("n8", "n5", false),
        ("n1", "n3", true),
        ("n4", "n3", false),
        ("n9", "n8", false),
    ] {
        let (child_rect, parent_rect) = (rect(child), rect(parent));
        assert!(
            child_rect.y > parent_rect.bottom(),
            "{child} under {parent}"
        );
        assert_eq!(
            child_rect.center().x < parent_rect.center().x,
            left,
            "{child} on its side of {parent}"
        );
    }
    let spread = |child: &str, parent: &str| rect(child).center().x - rect(parent).center().x;
    assert_eq!(
        [
            spread("n3", "n5"),
            spread("n8", "n5"),
            spread("n1", "n3"),
            spread("n4", "n3")
        ],
        [-160, 160, -80, 80],
        "a binary tree spreads by half as much at each level"
    );
    assert_apart(nodes);
}

fn assert_apart(nodes: &[NodeBox]) {
    for (index, (name, a, _)) in nodes.iter().enumerate() {
        for (other, b, _) in &nodes[index + 1..] {
            assert!(!a.intersects(*b), "{name} and {other} overlap");
        }
    }
}

/// A hash table: four buckets in a column, an entry beside its bucket and a collision chained
/// beside the first entry, each lined up with its bucket.
#[test]
fn a_hash_table_chains_a_collision_beside_its_bucket() {
    let sim = run(
        concat!(
            r#"The table has <cells id="ht" count="4" vertical="true" label="buckets"/>four "#,
            r#"buckets. Apple hashes to <node id="apple" label="apple: 3" near="ht.2" "#,
            r#"dir="right"/><edge from="ht.2" to="apple"/>bucket two, and grape collides "#,
            r#"there, so it <node id="grape" label="grape: 7" near="apple" dir="right"/>"#,
            r#"<edge from="apple" to="grape"/>chains behind apple. Kiwi goes to "#,
            r#"<node id="kiwi" label="kiwi: 1" near="ht.0" dir="right"/>"#,
            r#"<edge from="ht.0" to="kiwi"/>bucket zero."#,
        ),
        "Show me how a hash map handles collisions",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let ops = ops(&sim);
    let Some(DrawOp::Cells {
        cells, vertical, ..
    }) = ops.iter().find(|op| matches!(op, DrawOp::Cells { .. }))
    else {
        panic!("buckets: {ops:#?}");
    };
    assert!(*vertical);
    let bucket = |index: usize| -> &CellBox { &cells[index] };
    assert!(bucket(0).rect.bottom() == bucket(1).rect.y, "a column");
    let nodes = node_rects(&ops);
    let rect = |name: &str| {
        nodes
            .iter()
            .find(|(id, ..)| id == name)
            .map(|(_, rect, _)| *rect)
            .expect("drawn")
    };
    let (apple, grape, kiwi) = (rect("apple"), rect("grape"), rect("kiwi"));
    assert_eq!(apple.center().y, bucket(2).rect.center().y, "lined up");
    assert!(apple.x > bucket(2).rect.right());
    assert_eq!(
        grape.center().y,
        apple.center().y,
        "the chain stays on its row"
    );
    assert!(grape.x > apple.right());
    assert_eq!(kiwi.center().y, bucket(0).rect.center().y);
    let edge = ops
        .iter()
        .find_map(|op| match op {
            DrawOp::Edge { from, path, .. } if from.as_str() == "ht.2" => Some(*path),
            _ => None,
        })
        .expect("an edge from the bucket");
    assert_eq!(edge.start.x, bucket(2).rect.right(), "from the cell's side");
    let board = summary(sim.core().board());
    assert!(board.contains("ht[_|_|_|_](column)"), "{board}");
    assert!(board.contains("apple"), "{board}");
}

/// An interruption drops a message that was laid out but not drawn; the next messages carry on
/// the lifelines from where the drawn line ended, with no gap and no overlap (I6).
#[test]
fn a_dropped_message_leaves_the_lifelines_where_they_were_drawn() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"The <participant id="a" label="Alice"/>caller and the "#,
        r#"<participant id="b" label="Bob"/>callee talk. Alice "#,
        r#"<message from="a" to="b" label="one"/>calls, Bob <message from="b" to="a" label="two"/>"#,
        r#"<message from="b" to="a" label="three"/><message from="b" to="a" label="four"/>answers "#,
        r#"three times in a row, and then a long while passes before anything else happens."#,
    )));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"Because Bob <message from="b" to="b" label="think"/>thinks first. Then "#,
        r#"<message from="a" to="b" label="five"/>Alice calls again and "#,
        r#"<message from="b" to="a" label="six"/>Bob answers."#,
    )));
    sim.user_says(0, "Show me a call between two people");
    sim.user_says(5_200, "wait, why does Bob answer three times?");
    sim.run_until(90_000);
    assert_clean(&sim);
    let drawn: Vec<String> = ops(&sim)
        .iter()
        .filter_map(|op| match op {
            DrawOp::Message { label, .. } => label.clone(),
            _ => None,
        })
        .collect();
    assert!(
        drawn.iter().any(|label| label == "one") && !drawn.iter().any(|label| label == "four"),
        "the interruption came before the last messages were drawn: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|label| label == "six"),
        "the answer drew its messages: {drawn:?}"
    );
}

/// A message naming a participant nobody drew draws it first, on a layer with a sequence: the
/// handshake isn't lost to a forgotten `participant` (live gemma4 runs).
#[test]
fn a_forgotten_participant_is_drawn_before_its_message() {
    let sim = run(
        concat!(
            r#"The <participant id="client" label="Client"/>client <message from="client" "#,
            r#"to="server" label="SYN"/>sends a SYN, the server <message from="server" "#,
            r#"to="client" label="SYN-ACK" style="return"/>answers, the <participant id="server" "#,
            r#"label="SERVER"/>server, and <message from="a" to="b"/>nothing else."#,
        ),
        "Explain the TCP handshake as a sequence diagram",
    );
    assert_clean(&sim);
    let drawn: Vec<String> = ops(&sim)
        .iter()
        .filter_map(|op| match op {
            DrawOp::Participant { id, .. } => Some(format!("participant {}", id.as_str())),
            DrawOp::Message { from, to, .. } => {
                Some(format!("message {}>{}", from.as_str(), to.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn,
        [
            "participant client",
            "participant server",
            "message client>server",
            "message server>client",
            "participant a",
            "participant b",
            "message a>b",
        ],
        "each forgotten end comes first, labelled with its id"
    );
    assert_eq!(dropped(&sim), 0);
    assert!(
        !ops(&sim)
            .iter()
            .any(|op| matches!(op, DrawOp::Strike { .. })),
        "naming it afterwards corrects nothing"
    );
    let board = summary(sim.core().board());
    assert!(board.contains("sequence client, server, a, b:"), "{board}");
    let server = sim
        .core()
        .board()
        .layer(LayerId::MAIN)
        .and_then(|layer| layer.element(&ElementId::parse("server").expect("valid")))
        .map(|element| element.label.clone());
    assert_eq!(server.as_deref(), Some("Server"), "labelled from its id");
}

/// A row of cells next to a cell (a linked list of two-cell boxes) lines up with that cell.
#[test]
fn cells_beside_a_cell_line_up_with_it() {
    let sim = run(
        concat!(
            r#"The list starts at <cells id="a" values="3|·" index="none"/>three, whose next "#,
            r#"pointer leads to <cells id="b" values="7|·" label="next" near="a.1"/>"#,
            r#"<edge from="a.1" to="b.0"/>seven."#,
        ),
        "Draw a linked list",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let rows: Vec<Vec<CellBox>> = ops(&sim)
        .into_iter()
        .filter_map(|op| match op {
            DrawOp::Cells { cells, .. } => Some(cells),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1].rect.center().y, rows[1][0].rect.center().y);
    assert!(rows[1][0].rect.x > rows[0][1].rect.right());
    assert!(rows[0].iter().all(|cell| cell.index.is_none()));
    assert!(
        rows[1].iter().all(|cell| cell.index.is_some()),
        "lined up by its cells, though its labels make it taller below"
    );
}

/// The main board's sequence and a detour's are separate: the detour starts its own row.
#[test]
fn a_detour_draws_its_own_sequence() {
    let sim = run(
        concat!(
            r#"The <participant id="c" label="Client"/>client <participant id="s" label="Server"/>"#,
            r#"<message from="c" to="s" label="GET"/>asks. <detour title="TLS"/>First, "#,
            r#"<participant id="c" label="Client"/>the client and <participant id="s" "#,
            r#"label="Server"/><message from="c" to="s" label="hello"/>server say hello. "#,
            r#"<resume/>So, back to the request."#,
        ),
        "What happens on a request?",
    );
    assert_clean(&sim);
    assert_eq!(dropped(&sim), 0);
    let detour = sim
        .core()
        .board()
        .layer(LayerId(1))
        .expect("the detour layer");
    assert_eq!(
        detour
            .elements()
            .iter()
            .filter(|element| element.id.as_str() == "c" || element.id.as_str() == "s")
            .count(),
        2
    );
}

/// Pointing at a cell names the cell, with its value, to the talker: a cell is an element of its
/// own (D-120), so "why is this one stale?" lands on `ht.2`, not on the whole column.
#[test]
fn pointing_at_a_cell_names_that_cell() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"The block has <cells id="blk" values="A|B|C" vertical="true" label="block"/>three "#,
        r#"pages, and <annotate target="blk.1" mark="highlight" text="stale"/>page one is stale."#,
    )));
    sim.talker
        .push(ScriptedReply::says("That page was overwritten elsewhere."));
    sim.user_says(0, "How does a flash block look?");
    sim.run_until(20_000);
    let target = PointerTarget::Element {
        layer: LayerId::MAIN,
        id: ElementId::parse("blk.1").expect("valid"),
    };
    sim.at(
        20_100,
        Input::Command(UiCommand::Pointer {
            target: Some(target),
        }),
    );
    sim.user_says(20_200, "why is this one stale?");
    sim.run_until(40_000);
    assert_clean(&sim);
    let asked = sim
        .core()
        .conversation()
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => Some(text.clone()),
                UserPart::ToolResult { .. } => None,
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .expect("the question");
    assert!(
        asked.contains(r#"[Pointing at: blk.1 "B" on the main board]"#),
        "{asked}"
    );
}
