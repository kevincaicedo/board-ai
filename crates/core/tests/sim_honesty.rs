//! Simulator scenario for honest module maps (D-180, law 6): in a repo session a node labelled
//! with a repo path no tool returned is drawn as a note marked "?", and the talker is told.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::history::Message;
use boardai_core::protocol::{DrawOp, NodeShape, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::{DirEntry, ToolOutput};
use common::{assert_clean, config};

/// gemma4's tour after one `list_dir` of the root (2026-10-16), in miniature.
const TOUR: &str = concat!(
    r#"The code lives in <node id="crates" label="crates/" kind="group"/>crates, with "#,
    r#"<node id="core" label="crates/core" near="crates"/>a core crate, an "#,
    r#"<node id="auth" label="auth/" near="core"/>auth module and the "#,
    r#"<node id="ui" label="UI shell" near="auth"/>interface, with an "#,
    r#"<node id="engine" label="engine" near="ui"/>engine and a "#,
    r#"<node id="docs" label="docs" near="engine"/>docs folder."#,
);

fn drawn(sim: &Sim) -> Vec<(String, String, NodeShape)> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                op: DrawOp::Node {
                    id, label, shape, ..
                },
                ..
            } = event
            {
                Some((id.as_str().to_owned(), label.clone(), *shape))
            } else {
                None
            }
        })
        .collect()
}

/// `list_dir` answers with these folders.
fn respond_listing(sim: &mut Sim, names: &[&str]) {
    sim.tools.respond(
        "list_dir",
        ToolOutput::Listing {
            entries: names
                .iter()
                .map(|name| DirEntry {
                    name: (*name).to_owned(),
                    is_dir: true,
                })
                .collect(),
            more_entries: 0,
        },
    );
}

#[test]
fn a_module_no_tool_returned_is_drawn_as_a_question() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Let me look.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(TOUR));
    sim.talker.push(ScriptedReply::says("Sure."));
    respond_listing(&mut sim, &["crates", "docs", "ui"]);
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    let nodes = drawn(&sim);
    let find = |id: &str| {
        nodes
            .iter()
            .find(|(drawn, _, _)| drawn == id)
            .cloned()
            .expect("drawn")
    };
    assert_eq!(find("crates").1, "crates/", "returned by list_dir");
    assert_eq!(
        find("crates").2,
        NodeShape::Service,
        "a group nothing goes in is a box"
    );
    assert_eq!(
        find("core").1,
        "crates/core ?",
        "crates was listed, not crates/core"
    );
    assert_eq!(find("core").2, NodeShape::Note);
    assert_eq!(find("auth").1, "auth/ ?");
    assert_eq!(find("ui").1, "UI shell", "a concept's name is left alone");
    assert_eq!(
        find("engine").1,
        "engine ?",
        "a module's name in a tour is checked too"
    );
    assert_eq!(find("docs").1, "docs", "listed");
    sim.user_says(40_000, "What is in core?");
    sim.run_until(50_000);
    let notes: String = sim
        .talker
        .requests
        .last()
        .expect("asked")
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System { text } => Some(text.as_str()),
            Message::User { .. } | Message::Assistant { .. } => None,
        })
        .collect();
    assert!(
        notes.contains("no repo tool returned crates/core"),
        "{notes}"
    );
    assert!(
        notes.contains("crates/core ?"),
        "the summary shows the mark: {notes}"
    );
}

/// With no repo tool used, a path-like label is general knowledge: nothing is marked.
#[test]
fn outside_a_repo_session_labels_are_left_alone() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"A Rust crate keeps its entry in <node id="lib" label="src/lib.rs"/>src/lib.rs, "#,
        r#"which a <node id="client" label="client" near="lib"/>client calls."#,
    )));
    sim.user_says(0, "How is a Rust crate laid out?");
    sim.run_until(20_000);
    assert_clean(&sim);
    let labels: Vec<String> = drawn(&sim).into_iter().map(|(_, label, _)| label).collect();
    assert_eq!(labels, vec!["src/lib.rs", "client"]);
}

/// gemma4 after the retry (2026-10-16): it wrote the read in its text twice and described a
/// listing it never got. Nothing was read, so the module it named is drawn as a question.
#[test]
fn a_tour_that_read_nothing_marks_every_module() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says(r#"list_dir{path:"."}"#));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"I see the top level now, like the internal directory "#,
        r#"<node id="internal" label="internal"/>internal and a "#,
        r#"<node id="root" label="Repository root" kind="note"/>root note. "#,
        r#"list_dir{path:"internal"}"#,
    )));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    let labels: Vec<String> = drawn(&sim).into_iter().map(|(_, label, _)| label).collect();
    assert_eq!(labels, vec!["internal ?", "Repository root"]);
    let talks = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, boardai_core::effect::Effect::Talk { .. }))
        .count();
    assert_eq!(talks, 2, "asked once, never twice");
}
