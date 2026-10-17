//! A source's diagram belongs to the reply that reads it (D-270). A figure or a held brief's
//! diagram waits for that reply's tool round: when the reply stops first, it never lands under
//! the next question. A staging that would open a layer or turn the page waits until the
//! reply's earlier words have drawn, so their drawing stays on the board they were written for.
//! Review 3, findings 15 and 16: each test was red at 2b76809.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::ids::LayerId;
use boardai_core::input::Input;
use boardai_core::protocol::{DrawOp, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::{BriefTopic, SourceInput};
use common::documents::{DIGEST, opened, paper, read_event};
use common::{assert_clean, config, request_text};

fn session_with_paper() -> Sim {
    let mut sim = Sim::new(config());
    sim.at(0, opened());
    sim.share(100, &paper(), false);
    sim.at(600, read_event(DIGEST));
    sim.run_until(1_000);
    sim
}

/// Finding 15: a figure's diagram waited for the tool round of the reply that read it; Esc
/// before that round left it waiting, and it was staged (and its leftovers revealed) under the
/// user's next, unrelated question.
#[test]
fn a_figure_read_by_a_stopped_reply_never_lands_under_the_next_question() {
    let mut sim = session_with_paper();
    sim.talker.push(
        ScriptedReply::says("Let me look.")
            .calls("read_file", r#"{"path":"layered-store.pdf#Figure 1"}"#),
    );
    sim.talker.push(ScriptedReply::says(
        "A B-tree keeps its keys sorted in wide nodes.",
    ));
    sim.user_says(2_000, "Explain Figure 1 of the layered store paper.");
    let called = sim.run_until_true(30_000, |sim| {
        sim.trace().iter().any(|(_, entry)| {
            matches!(
                entry,
                boardai_core::sim::Entry::Input(Input::TalkerToolCall { .. })
            )
        })
    });
    assert!(called, "the figure is read");
    let at = sim.now().as_ms();
    // Esc before the stream's end: the reply goes, its tool round never happens.
    sim.at(at, Input::Command(UiCommand::Stop));
    sim.run_until(at + 3_000);
    let staged_before = sim
        .ui()
        .filter(|(_, event)| matches!(event, UiEvent::Diagram { part: 0, .. }))
        .count();
    assert_eq!(staged_before, 0, "nothing staged for the stopped reply");
    sim.user_says(at + 4_000, "Unrelated: what is a B-tree?");
    sim.run_until(at + 40_000);
    let last = sim.talker.requests.len() - 1;
    let asked = request_text(&sim, last);
    let staged: Vec<String> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Diagram { part: 0, title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    let revealed = sim
        .ui()
        .filter(|(_, event)| {
            matches!(
                event,
                UiEvent::OpStart {
                    op: boardai_core::protocol::DrawOp::Reveal { .. },
                    ..
                }
            )
        })
        .count();
    assert!(
        staged.is_empty() && !asked.contains("The diagram of Figure 1"),
        "the stopped reply's figure was staged under the B-tree question: {staged:?}, \
         reveals drawn: {revealed}"
    );
    assert_clean(&sim);
}

const OVERVIEW: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas.
## Components
- arena · Arena · type · src/arena.rs — Owns pages and hands out slots
## Citations
- src/arena.rs:40-44
";

const ARENA: &str = "# Brief: the arena
## Overview
The arena keeps a free list per page and bumps a pointer inside the page.
## Components
- page · Page · type · src/arena.rs — 64 KiB of slots
- bump · Bump pointer · field · src/arena.rs — Next free byte
## Diagram
<diagram title=\"Arena\">
<node id=\"page\" label=\"Page\" kind=\"service\"/>
<node id=\"bump\" label=\"Bump pointer\" kind=\"service\"/>
<edge from=\"bump\" to=\"page\" label=\"points into\"/>
</diagram>
## Citations
- src/arena.rs:60-62
";

const INF_ALLOC: &str = "~/…/infinitydb/crates/inf-alloc";

/// Finding 16: a component brief's diagram was staged when the tool round's request went out,
/// while the voice still played the first stream; staging opened the part's detail layer at
/// once, and the first stream's drawing, whose words hadn't played yet, landed on it.
/// A code folder shared, its arena brief in the cache, and the arena drawn on the board.
fn arena_on_the_board() -> Sim {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.sources.cache.insert(
        (INF_ALLOC.to_owned(), BriefTopic::component("Arena").key()),
        ARENA.to_owned(),
    );
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(6_000);
    // An earlier answer drew the arena on the board.
    sim.talker.push(ScriptedReply::says(
        "<node id=\"arena\" label=\"Arena\" kind=\"service\"/>The arena holds the pages.",
    ));
    sim.user_says(7_000, "Draw the allocator for me.");
    sim.run_until(20_000);
    sim
}

/// The layers `event` names, when it is the kind `pick` keeps.
fn layers(sim: &Sim, pick: impl Fn(&UiEvent) -> Option<LayerId>) -> Vec<LayerId> {
    sim.ui().filter_map(|(_, event)| pick(event)).collect()
}

#[test]
fn a_component_briefs_staging_waits_for_the_first_streams_drawing() {
    let mut sim = arena_on_the_board();
    // Now a question about it: the first stream draws on the board and asks for the arena's brief.
    sim.talker.push(
        ScriptedReply::says(
            "It keeps a free list for each page it owns, and every allocation it serves takes the \
             first slot on that <node id=\"free-list\" label=\"Free list\" near=\"arena\"/>free \
             list.",
        )
        .calls(
            "research",
            r#"{"question":"How does the arena work?","depth":"quick"}"#,
        ),
    );
    sim.talker.push(ScriptedReply::says(
        "<reveal id=\"page\"/>Each page is 64 KiB.",
    ));
    sim.user_says(21_000, "How does the arena work?");
    sim.run_until(60_000);
    let free_list = layers(&sim, |event| match event {
        UiEvent::OpStart {
            layer,
            op: DrawOp::Node { id, .. },
            ..
        } if id.as_str() == "free-list" => Some(*layer),
        _ => None,
    });
    let pushed = layers(&sim, |event| match event {
        UiEvent::LayerPush { layer, .. } => Some(*layer),
        _ => None,
    });
    assert_eq!(
        free_list,
        vec![LayerId::MAIN],
        "the first stream's node belongs on the board it was written for (layers pushed: \
         {pushed:?})"
    );
    assert!(
        pushed.len() == 1 && pushed[0] != LayerId::MAIN,
        "the detail layer opened after: {pushed:?}"
    );
    let staged = layers(&sim, |event| match event {
        UiEvent::Diagram { layer, part: 0, .. } => Some(*layer),
        _ => None,
    });
    assert_eq!(staged, pushed, "the brief's diagram is staged on it");
    let revealed = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::OpStart { op: DrawOp::Reveal { .. }, layer, .. } if *layer != LayerId::MAIN)
    });
    assert!(revealed, "and revealed there");
    assert!(
        request_text(&sim, 2).contains("will be staged on a detail layer anchored to arena"),
        "the talker hears where it goes"
    );
    assert_clean(&sim);
}
