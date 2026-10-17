//! Larger, truer maps (T-035): a walkthrough of a whole thing asks the talker for a staged
//! map with breadth, as a request for a map does (D-239, D-247), while a sketch question doesn't;
//! a drawing tag naming a staged part that is already drawn points at it instead of rewriting it
//! (D-251); what a reply's diagram still holds when everything was said is revealed then (D-237,
//! D-250); and a staged repo map is held to what was read (D-180, D-252).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::history::{Message, UserPart};
use boardai_core::protocol::{DrawOp, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::{DirEntry, ToolOutput};
use boardai_core::{CoreConfig, RepoReads};
use common::{assert_clean, config, op_starts};

/// What the stage hint asks of a whole map.
const HINT_ASKS: &str = "Aim for 40 to 100 parts in all";

/// The user's message of request `index`: the last message on the user's side with words.
fn user_words(sim: &Sim, index: usize) -> String {
    let request = sim.talker.requests.get(index).expect("a request");
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => Some(text.clone()),
                UserPart::ToolResult { .. } => None,
            }),
            Message::System { .. } | Message::Assistant { .. } => None,
        })
        .expect("the user's words")
}

fn first_request(question: &str) -> String {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, question);
    sim.run_until(20_000);
    assert_clean(&sim);
    user_words(&sim, 0)
}

#[test]
fn a_walkthrough_of_a_whole_thing_asks_for_a_map_with_breadth() {
    for question in [
        "Walk me through how a B-tree storage engine works.",
        "Draw me a detailed map of the coding harness in crates/runtime/src/harness.",
        "Walk me through this repo.",
    ] {
        let words = first_request(question);
        assert!(words.starts_with(question), "{words}");
        assert!(words.contains(HINT_ASKS), "{words}");
        assert!(
            words.contains("draw only what your reads returned"),
            "{words}"
        );
    }
    let words = first_request("More detail on the buffer pool.");
    assert!(words.contains("deeper than the map"), "{words}");
    assert!(
        !words.contains(HINT_ASKS),
        "a drill-down's own hint: {words}"
    );
    for question in [
        "How does a request get from the browser to our database? Walk me through the CDN, the \
         load balancer, the API pods, Redis and Postgres.",
        "Show me how a hash map handles collisions.",
        "Explain the TCP handshake as a sequence diagram.",
    ] {
        let words = first_request(question);
        assert!(!words.contains(HINT_ASKS), "a sketch question: {words}");
    }
}

/// gpt-oss-120b's repo map (T-035), cut to two parts: each reveal followed by a bare node tag
/// naming the part just revealed. A bare node's label is its id, so each struck the part's real
/// label and wrote "mod ?" over it (D-180). Now it points at the part and changes nothing.
#[test]
fn a_node_tag_naming_a_revealed_part_changes_nothing() {
    let mut sim = Sim::new(CoreConfig {
        repo_reads: RepoReads::Offered,
        ..config()
    });
    sim.tools.respond(
        "list_dir",
        ToolOutput::Listing {
            entries: ["mod.rs", "claude.rs"]
                .iter()
                .map(|name| DirEntry {
                    name: (*name).to_owned(),
                    is_dir: false,
                })
                .collect(),
            more_entries: 0,
        },
    );
    sim.talker.push(
        ScriptedReply::says("Listing it.")
            .calls("list_dir", r#"{"path":"crates/runtime/src/harness"}"#),
    );
    sim.talker.push(ScriptedReply::says(concat!(
        r#"Here is the harness. <diagram id="harness" title="Coding Harness">"#,
        r#"<node id="mod" label="harness/mod.rs" kind="group"/>"#,
        r#"<node id="claude" label="claude.rs" kind="group" in="mod"/>"#,
        r#"<node id="harness_trait" label="CodingHarness trait" kind="pill" in="mod"/>"#,
        "</diagram>",
        r#"<reveal id="mod"/>The top-level <node id="mod"/>module re-exports the public API. "#,
        r#"<reveal id="claude"/>The <node id="claude"/>module runs Claude Code, and the "#,
        r#"<node id="harness_trait"/>trait is what each harness implements."#,
    )));
    sim.user_says(0, "Draw me a detailed map of the coding harness");
    sim.run_until(60_000);
    assert_clean(&sim);
    let drawn = op_starts(&sim);
    let struck = drawn.iter().any(|(_, event)| {
        matches!(
            event,
            UiEvent::OpStart {
                op: DrawOp::Strike { .. },
                ..
            }
        )
    });
    assert!(!struck, "{drawn:#?}");
    let marked = drawn
        .iter()
        .any(|(_, event)| format!("{event:?}").contains(" ?\""));
    assert!(!marked, "{drawn:#?}");
    let board = sim.core().board().layer(boardai_core::ids::LayerId::MAIN);
    let label = |id: &str| {
        let id = boardai_core::ids::ElementId::parse(id).expect("id");
        board
            .and_then(|layer| layer.element(&id))
            .map(|element| element.label.clone())
    };
    assert_eq!(label("mod").as_deref(), Some("harness/mod.rs"));
    assert_eq!(label("claude").as_deref(), Some("claude.rs"));
    assert_eq!(
        label("harness_trait").as_deref(),
        Some("CodingHarness trait"),
        "a bare tag naming a planned part still reveals it"
    );
}

const PARTLY_REVEALED: &str = concat!(
    r#"Here it is. <diagram id="d" title="Engine"><node id="a" kind="group" label="Pager"/>"#,
    r#"<node id="a1" label="Page cache" in="a"/><record id="a2" label="struct Page" "#,
    r#"fields="id|lsn|bytes" in="a"/><node id="b" kind="group" label="Log"/>"#,
    r#"<node id="b1" label="Log writer" in="b"/><edge from="a1" to="b1" label="flush"/>"#,
    r#"</diagram><reveal id="a"/>First the pager, which holds the pages in memory. Then the "#,
    r#"<reveal id="b"/>log, which makes writes durable before the pages move."#,
);

fn states(sim: &Sim) -> Vec<(String, bool)> {
    let board = sim
        .core()
        .board()
        .layer(boardai_core::ids::LayerId::MAIN)
        .expect("the main layer");
    board
        .elements()
        .iter()
        .map(|element| (element.id.as_str().to_owned(), element.state.is_shown()))
        .collect()
}

/// D-237, D-250: the parts a reply's diagram still holds once everything was said are revealed then,
/// in a hand's order and at a pace per part. Live models revealed each section's frame and named
/// a few parts; the rest stayed planned (DeepSeek's repo maps showed 19 of 31 and 22 of 36, T-035).
#[test]
fn the_parts_left_planned_are_revealed_when_the_reply_ends() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(PARTLY_REVEALED));
    sim.user_says(0, "Walk me through how a storage engine works");
    sim.run_until(60_000);
    assert_clean(&sim);
    let hidden: Vec<String> = states(&sim)
        .into_iter()
        .filter(|(_, shown)| !shown)
        .map(|(id, _)| id)
        .collect();
    assert!(hidden.is_empty(), "still planned: {hidden:?}");
    let reveals: Vec<(u32, String)> = op_starts(&sim)
        .iter()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Reveal { parts, .. },
                budget_ms,
                ..
            } => Some((
                *budget_ms,
                parts
                    .iter()
                    .map(|part| part.id().as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        reveals.len(),
        3,
        "the two named, then the rest: {reveals:?}"
    );
    let (budget_ms, last) = &reveals[2];
    assert!(last.starts_with("a1 a2"), "in the order written: {last}");
    assert!(last.contains("a1>b1"), "{last}");
    let parts = u32::try_from(last.split(' ').count()).expect("a count");
    assert_eq!(
        *budget_ms,
        parts * boardai_core::limits::LEFTOVER_REVEAL_MS_PER_PART,
        "{reveals:?}"
    );
}

/// An interruption isn't the reply's end: what it didn't get to stays planned, for the talker to
/// reveal when it goes on.
#[test]
fn an_interrupted_reply_leaves_its_parts_planned() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(PARTLY_REVEALED));
    sim.talker
        .push(ScriptedReply::says("Because the pager batches them."));
    sim.user_says(0, "Walk me through how a storage engine works");
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let at = sim.now().as_ms() + 100;
    sim.user_says_with_onset(at, "wait, why a log?");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    let planned = states(&sim)
        .into_iter()
        .any(|(id, shown)| id == "a1" && !shown);
    assert!(planned, "{:?}", states(&sim));
}

/// D-180 holds a staged diagram's labels too (D-252). gemma4 staged a repo map before reading
/// anything, with files the repo doesn't have (T-035): they are drawn marked "?" where they stand
/// and the talker is told; a name a listing returned, and labels in words, are left alone.
#[test]
fn a_staged_repo_map_marks_what_no_read_returned() {
    let mut sim = Sim::new(CoreConfig {
        repo_reads: RepoReads::Offered,
        ..config()
    });
    sim.tools.respond(
        "list_dir",
        ToolOutput::Listing {
            entries: vec![DirEntry {
                name: "claude.rs".to_owned(),
                is_dir: false,
            }],
            more_entries: 0,
        },
    );
    sim.talker.push(
        ScriptedReply::says("Listing it.")
            .calls("list_dir", r#"{"path":"crates/runtime/src/harness"}"#),
    );
    sim.talker.push(ScriptedReply::says(concat!(
        r#"Here is the harness. <diagram id="h" title="Harness">"#,
        r#"<node id="run" kind="group" label="Running a task"/>"#,
        r#"<node id="claude" label="claude.rs" in="run"/>"#,
        r#"<node id="exec" label="executor.rs" in="run"/>"#,
        r#"<node id="api" label="Harness API" in="run"/>"#,
        r#"</diagram><reveal id="run" deep="true"/>A task runs through these parts."#,
    )));
    sim.user_says(0, "Draw me a detailed map of the coding harness");
    sim.run_until(60_000);
    assert_clean(&sim);
    let labels: Vec<String> = op_starts(&sim)
        .iter()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Reveal { parts, .. },
                ..
            } => Some(format!("{parts:?}")),
            _ => None,
        })
        .collect();
    let drawn = labels.join("\n");
    assert!(drawn.contains("\"executor.rs ?\""), "{drawn}");
    assert!(drawn.contains("\"claude.rs\""), "{drawn}");
    assert!(drawn.contains("\"Harness API\""), "{drawn}");
    // The talker hears it with its next request.
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(60_000, "And then?");
    sim.run_until(80_000);
    assert_clean(&sim);
    let told = common::transcript(&sim).join("\n");
    assert!(
        told.contains("1 part was marked \"?\": no repo tool returned executor.rs"),
        "{told}"
    );
}
