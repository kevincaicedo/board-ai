//! Simulator scenarios for what live models leave out of trees and sequence diagrams (D-181):
//! a circle placed below a circle is its child, and a reply along an open call is a return.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::protocol::{DrawOp, LineStyle, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{assert_clean, config};

fn run(text: &str) -> Sim {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(text));
    sim.user_says(0, "Go on.");
    sim.run_until(40_000);
    assert_clean(&sim);
    sim
}

/// Each node drawn, and whether it came with the line from a parent.
fn linked(sim: &Sim) -> Vec<(String, bool)> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                op: DrawOp::Node { id, link, .. },
                ..
            } = event
            {
                Some((id.as_str().to_owned(), link.is_some()))
            } else {
                None
            }
        })
        .collect()
}

fn styles(sim: &Sim) -> Vec<(String, LineStyle)> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                op: DrawOp::Message { label, style, .. },
                ..
            } = event
            {
                Some((label.clone().unwrap_or_default(), *style))
            } else {
                None
            }
        })
        .collect()
}

/// gemma4 drew trees with `near` and never `parent`; below a circle, a circle is its child.
#[test]
fn a_circle_below_a_circle_is_its_child() {
    let sim = run(concat!(
        r#"Seven is the <node id="n7" label="7" kind="circle"/>root, "#,
        r#"<node id="n4" label="4" kind="circle" near="n7" dir="below"/>four under it, "#,
        r#"<node id="n9" label="9" near="n7" dir="down"/>nine beside four, and "#,
        r#"<node id="s2" label="Idle" kind="circle" near="n9"/>a state to the right."#,
    ));
    assert_eq!(
        linked(&sim),
        vec![
            ("n7".to_owned(), false),
            ("n4".to_owned(), true),
            ("n9".to_owned(), true),
            ("s2".to_owned(), false),
        ]
    );
    let board = sim.core().board();
    let main = board.layer(boardai_core::ids::LayerId::MAIN).expect("main");
    let parent = |id: &str| {
        main.element(&boardai_core::ids::ElementId::parse(id).expect("id"))
            .and_then(|element| {
                if let boardai_core::board::ElementKind::Node { parent, .. } = &element.kind {
                    parent.clone()
                } else {
                    None
                }
            })
            .map(|parent| parent.as_str().to_owned())
    };
    assert_eq!(parent("n4").as_deref(), Some("n7"));
    assert_eq!(parent("n9").as_deref(), Some("n7"));
}

/// A box below a circle, or a circle below a box, is not a tree.
#[test]
fn only_circles_make_a_tree() {
    let sim = run(concat!(
        r#"The <node id="a" label="A" kind="circle"/>root and "#,
        r#"<node id="b" label="B" kind="service" near="a" dir="below"/>a box under it, "#,
        r#"<node id="c" label="C" kind="circle" near="b" dir="below"/>then a circle."#,
    ));
    assert!(linked(&sim).iter().all(|(_, link)| !link));
}

/// Replies without `style="return"`: a message back along an open call is its return; the
/// next call is a call again; an explicit style always wins.
#[test]
fn a_reply_along_an_open_call_is_a_return() {
    let sim = run(concat!(
        r#"The <participant id="c" label="Client"/>client <participant id="s" label="Server"/>"#,
        r#"<message from="c" to="s" label="SYN"/>sends a SYN, the server "#,
        r#"<message from="s" to="c" label="SYN-ACK"/>answers, the client "#,
        r#"<message from="c" to="s" label="ACK"/>acknowledges, then "#,
        r#"<message from="c" to="s" label="GET"/>asks, the <participant id="d" label="DB"/>"#,
        r#"<message from="s" to="d" label="query"/>server queries, the "#,
        r#"<message from="d" to="s" label="rows"/>database returns rows, the server "#,
        r#"<message from="s" to="c" label="200" style="solid"/>replies, and "#,
        r#"<message from="s" to="s" label="log"/>logs it."#,
    ));
    assert_eq!(
        styles(&sim),
        vec![
            ("SYN".to_owned(), LineStyle::Solid),
            ("SYN-ACK".to_owned(), LineStyle::Dashed),
            ("ACK".to_owned(), LineStyle::Solid),
            ("GET".to_owned(), LineStyle::Solid),
            ("query".to_owned(), LineStyle::Solid),
            ("rows".to_owned(), LineStyle::Dashed),
            ("200".to_owned(), LineStyle::Solid),
            ("log".to_owned(), LineStyle::Solid),
        ]
    );
}

fn shapes(sim: &Sim) -> Vec<(String, boardai_core::protocol::NodeShape)> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                op: DrawOp::Node { id, shape, .. },
                ..
            } = event
            {
                Some((id.as_str().to_owned(), *shape))
            } else {
                None
            }
        })
        .collect()
}

/// gpt-oss-120b drew a group per folder and filled none: a group nothing goes in is a box,
/// and one with members stays a group.
#[test]
fn a_group_nothing_goes_in_is_drawn_as_a_box() {
    use boardai_core::protocol::NodeShape;
    let sim = run(concat!(
        r#"The code is in <node id="crates" label="crates/" kind="group"/>crates and "#,
        r#"<node id="docs" label="docs/" kind="group"/>docs. The "#,
        r#"<node id="vpc" label="VPC" kind="group"/>VPC holds the "#,
        r#"<node id="db" label="Postgres" kind="db" in="vpc"/>database."#,
    ));
    let drawn = shapes(&sim);
    let shape = |id: &str| {
        drawn
            .iter()
            .find(|(drawn, _)| drawn == id)
            .map(|(_, shape)| *shape)
    };
    assert_eq!(shape("crates"), Some(NodeShape::Service));
    assert_eq!(shape("docs"), Some(NodeShape::Service));
    assert_eq!(shape("vpc"), Some(NodeShape::Group));
}
