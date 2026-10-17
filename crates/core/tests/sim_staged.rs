//! Staged diagrams in conversation (D-187): a whole diagram laid out before its first stroke and
//! revealed as the narration names it, in order or out of it, through an interruption and a page
//! turn, beside the user's shapes and past its limits. Every run also holds I1, I6, I11 and I16
//! (a staged part is drawn once, by its reveal, where it was laid out), which the simulator checks
//! on every step.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::effect::{Dropped, Effect, LogRecord};
use boardai_core::geometry::Rect;
use boardai_core::ids::{ElementId, LayerId};
use boardai_core::input::Input;
use boardai_core::limits::{DIAGRAM_PARTS_MAX, REVEAL_BUDGET_MAX_MS};
use boardai_core::protocol::{DrawAction, DrawOp, DrawnShape, StagedPart, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{assert_clean, config, count, transcript};

const INF_ALLOC: &str = include_str!("fixtures/inf_alloc.narration");
const ARENA_INSIDE: &str = include_str!("fixtures/inf_alloc_arena.narration");
const MAIN: LayerId = LayerId::MAIN;

/// Each reveal's parts, in order.
fn reveals(sim: &Sim) -> Vec<Vec<String>> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Reveal { parts, .. },
                ..
            } => Some(
                parts
                    .iter()
                    .map(|part| part.id().as_str().to_owned())
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn dropped(sim: &Sim, what: Dropped) -> u32 {
    sim.effects()
        .filter_map(|(_, effect)| match effect {
            Effect::Log(LogRecord::Dropped { what: kind, count }) if *kind == what => Some(*count),
            _ => None,
        })
        .sum()
}

fn run(replies: &[&str], question: &str, until_ms: u64) -> Sim {
    let mut sim = Sim::new(config());
    for reply in replies {
        sim.talker.push(ScriptedReply::says(reply));
    }
    sim.user_says(0, question);
    sim.run_until(until_ms);
    sim
}

fn shown(sim: &Sim, layer: LayerId, id: &str) -> bool {
    let id = ElementId::parse(id).expect("id");
    sim.core()
        .board()
        .layer(layer)
        .and_then(|layer| layer.element(&id))
        .is_some_and(|element| element.state.is_shown())
}

/// The map from the owner's inf-alloc Excalidraw, revealed in the order it is explained: each
/// section after its frame, connectors after both ends, the legend with the first part, the
/// header naming the part being explained, close-ups along the way and the whole map at the end.
#[test]
fn the_inf_alloc_map_is_staged_whole_and_revealed_in_order() {
    let sim = run(
        &[INF_ALLOC],
        "Walk me through how inf-alloc hands out memory.",
        120_000,
    );
    assert_clean(&sim);
    assert_eq!(
        count(&sim, |effect| matches!(
            effect,
            Effect::Log(LogRecord::Dropped { .. })
        )),
        0
    );
    let reveals = reveals(&sim);
    assert_eq!(
        reveals[0][0], "arena",
        "the first section's frame comes first"
    );
    assert!(
        reveals[0]
            .iter()
            .filter(|id| id.contains(":legend"))
            .count()
            == 4,
        "the whole legend comes with the first part: {:?}",
        reveals[0]
    );
    let order: Vec<&String> = reveals.iter().flatten().collect();
    let at = |id: &str| order.iter().position(|drawn| *drawn == id).expect(id);
    assert!(
        at("alloc_in>class") > at("class"),
        "a connector after both its ends"
    );
    assert!(
        at("region") > at("slots") && at("slots") > at("pool"),
        "in narration order"
    );
    let board = sim.core().board().layer(MAIN).expect("main");
    let drawn = board
        .elements()
        .iter()
        .filter(|element| element.state.is_shown())
        .count();
    assert!(drawn >= 150, "{drawn} elements drawn");
    assert_header_and_camera(&sim);
}

/// The header names the part being explained; the camera closes up on each reveal and shows the
/// whole map once at the end; every reveal's budget is bounded.
fn assert_header_and_camera(sim: &Sim) {
    let parts: Vec<u8> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Diagram { part, parts: 4, .. } => Some(*part),
            _ => None,
        })
        .collect();
    assert_eq!(parts, [0, 1, 2, 3, 4], "the header follows the parts");
    let frames: Vec<bool> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Camera { enlarge, .. } => Some(*enlarge),
            _ => None,
        })
        .collect();
    let close_ups = frames.iter().filter(|enlarge| **enlarge).count();
    assert!(
        close_ups * 2 >= frames.len(),
        "close-ups on small parts: {frames:?}"
    );
    assert_eq!(
        frames.last(),
        Some(&false),
        "the whole map, once, at the end"
    );
    for (_, event) in sim.ui() {
        if let UiEvent::OpStart {
            op: DrawOp::Reveal { .. },
            budget_ms,
            ..
        } = event
        {
            assert!(*budget_ms <= REVEAL_BUDGET_MAX_MS);
        }
    }
}

/// A part named before its container brings the container's frame first; the rest of the
/// container comes later, and connectors wait for both their ends.
#[test]
fn a_part_revealed_out_of_order_brings_its_containers_first() {
    let narration = concat!(
        "Look at the slot first. <diagram id=\"m\" title=\"Arena\">",
        "<node id=\"arena\" kind=\"group\" label=\"Arena\"/>",
        "<node id=\"inner\" kind=\"group\" label=\"Chunk\" in=\"arena\"/>",
        "<node id=\"slot\" label=\"slot\" in=\"inner\"/>",
        "<node id=\"class\" label=\"class\" in=\"arena\"/>",
        "<edge from=\"class\" to=\"slot\" label=\"pick\"/>",
        "</diagram>",
        "<reveal id=\"slot\"/>A slot holds one record. <reveal id=\"class\"/>A class picks it, ",
        "<reveal id=\"m\" deep=\"true\"/>and that is the whole arena."
    );
    let sim = run(&[narration], "Show me the arena, the slot first.", 60_000);
    assert_clean(&sim);
    let reveals = reveals(&sim);
    assert_eq!(reveals[0], ["arena", "inner", "slot"]);
    assert_eq!(reveals[1], ["class", "class>slot"]);
    assert_eq!(
        reveals.len(),
        2,
        "the deep reveal found everything drawn: {reveals:?}"
    );
}

/// A reveal naming nothing staged is dropped and counted, the rest goes on, and the talker is
/// told what is staged and still undrawn.
#[test]
fn a_reveal_of_an_unknown_id_is_dropped_and_the_talker_told() {
    let narration = concat!(
        "<diagram id=\"m\" title=\"Map\"><node id=\"a\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\"/></diagram>",
        "First <reveal id=\"nope\"/>nothing, then <reveal id=\"a\"/>A."
    );
    let mut sim = run(&[narration, "B is next."], "Map it.", 30_000);
    sim.user_says(30_000, "And then?");
    sim.run_until(50_000);
    assert_clean(&sim);
    assert_eq!(dropped(&sim, Dropped::Op), 1);
    assert_eq!(
        reveals(&sim),
        [vec!["a".to_owned()], vec!["b".to_owned()]],
        "B, left planned, is revealed when the reply ends (D-237, D-250)"
    );
    let told = transcript(&sim).join("\n");
    assert!(
        told.contains("<reveal id=\"nope\"/> named nothing staged"),
        "{told}"
    );
    assert!(
        told.contains("Diagram m is staged; its parts not drawn yet: a, b"),
        "{told}"
    );
    assert!(
        told.contains("diagram m \"Map\""),
        "the board summary lists it: {told}"
    );
}

/// A barge-in mid-reveal freezes the reveal and drops the reveals still waiting; the staged parts
/// they would have drawn stay planned. The detour answers, `<resume/>` finishes the frozen reveal,
/// and the next reply reveals the rest by id (I5, I7).
#[test]
fn a_barge_in_mid_reveal_freezes_it_and_revealing_resumes_after() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(INF_ALLOC));
    sim.talker.push(ScriptedReply::says(concat!(
        "<detour title=\"Size classes\"/>Each <node id=\"c\" label=\"class\"/>class is a power ",
        "of two. <resume/>So, back to the map: <reveal id=\"pool\" deep=\"true\"/>the pool, ",
        "<reveal id=\"slots\" deep=\"true\"/>the slots and <reveal id=\"region\" deep=\"true\"/>",
        "the region."
    )));
    sim.user_says(0, "Walk me through how inf-alloc hands out memory.");
    assert!(sim.run_until_true(60_000, |sim| reveals(sim).len() >= 3));
    let at = sim.now().as_ms() + 300;
    sim.user_says(at, "wait, why powers of two?");
    sim.run_until(at + 60_000);
    assert_clean(&sim);
    let events: Vec<&UiEvent> = sim.ui().map(|(_, event)| event).collect();
    let freeze = events
        .iter()
        .position(|event| matches!(event, UiEvent::Freeze { layer } if *layer == MAIN))
        .expect("the reveal froze");
    let resume = events
        .iter()
        .position(|event| matches!(event, UiEvent::Resume { layer, .. } if *layer == MAIN))
        .expect("and resumed");
    assert!(resume > freeze);
    for part in ["pool", "slots", "region", "region_s"] {
        assert!(
            shown(&sim, MAIN, part),
            "{part} was revealed after the detour"
        );
    }
    let summary_before = transcript(&sim).join("\n");
    assert!(
        summary_before.contains("planned:"),
        "the talker saw what was planned"
    );
}

/// A page turn retires the staged diagram with its page: a later reveal of it names nothing on
/// the fresh page, and a diagram staged there starts at the page's corner.
#[test]
fn a_page_turn_takes_the_staged_diagram_with_its_page() {
    let narration = concat!(
        "<diagram id=\"m\" title=\"Map\"><node id=\"a\" kind=\"group\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\" in=\"a\"/><node id=\"c\" label=\"C\"/></diagram>",
        "<reveal id=\"a\"/>A first. <page title=\"DNS\"/>Now DNS: <reveal id=\"c\"/>C is gone, ",
        "<diagram id=\"d\" title=\"DNS\"><node id=\"r\" label=\"Resolver\"/></diagram>",
        "<reveal id=\"r\"/>the resolver asks."
    );
    let sim = run(&[narration], "Map it, then DNS.", 40_000);
    assert_clean(&sim);
    assert_eq!(
        count(&sim, |effect| matches!(
            effect,
            Effect::Ui(UiEvent::PageTurn { .. })
        )),
        1
    );
    assert_eq!(dropped(&sim, Dropped::Op), 1, "C is on the page that left");
    let staged = sim.core().board().staged(MAIN);
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].id.as_str(), "d");
    assert_eq!((staged[0].bounds.x, staged[0].bounds.y), (0, 0));
    assert!(shown(&sim, MAIN, "r"));
}

/// "More detail on the arena" opens a detail layer anchored to it at once; the talker fills it;
/// "zoom into" a part of that goes a level deeper; "back out" and Esc come back up, one level
/// each, and the frozen strokes finish. Pushes and pops balance (I4).
#[test]
fn drill_down_two_levels_and_back_out() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(INF_ALLOC));
    sim.talker.push(ScriptedReply::says(ARENA_INSIDE));
    sim.talker.push(ScriptedReply::says(concat!(
        "<diagram id=\"chunk\" title=\"One chunk\"><node id=\"h\" label=\"header\"/>",
        "<node id=\"f\" label=\"free list\"/></diagram><reveal id=\"chunk\" deep=\"true\"/>",
        "A chunk is a header and a free list."
    )));
    sim.user_says(0, "Walk me through how inf-alloc hands out memory.");
    sim.user_says(56_000, "More detail on the arena.");
    sim.user_says(90_000, "Zoom into one chunk.");
    sim.user_says(110_000, "Back out.");
    sim.at(120_000, Input::Command(UiCommand::Stop));
    sim.run_until(125_000);
    assert_clean(&sim);
    let pushes: Vec<(LayerId, String, Option<String>)> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::LayerPush {
                layer,
                title,
                anchor,
                ..
            } => Some((
                *layer,
                title.clone(),
                anchor
                    .as_ref()
                    .map(|anchor| anchor.element.as_str().to_owned()),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        pushes,
        [
            (LayerId(1), "Arena".to_owned(), Some("arena".to_owned())),
            (
                LayerId(2),
                "one chunk, 64 KiB class".to_owned(),
                Some("one".to_owned())
            ),
        ]
    );
    let pops: Vec<LayerId> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::LayerPop { layer } => Some(*layer),
            _ => None,
        })
        .collect();
    assert_eq!(pops, [LayerId(2), LayerId(1)], "back out, then Esc");
    assert_eq!(sim.core().thread().depth(), 0);
    assert!(
        shown(&sim, LayerId(1), "s1"),
        "the talker filled the first detail"
    );
    assert!(shown(&sim, LayerId(2), "h"), "and the second");
}

/// While the user draws, "more detail on the arena" opens nothing and asks no talker (I14): the
/// words wait for the drawing's turn.
#[test]
fn drill_down_while_drawing_waits_for_the_drawing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(INF_ALLOC));
    sim.talker
        .push(ScriptedReply::says("Sure, here is the arena."));
    sim.user_says(0, "Walk me through how inf-alloc hands out memory.");
    sim.run_until(56_000);
    sim.at(
        56_000,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.user_says(58_000, "More detail on the arena.");
    sim.run_until(64_000);
    let pushes = count(&sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::LayerPush { .. }))
    });
    assert_eq!(pushes, 0, "no detail layer opens over a drawing");
    sim.at(
        65_000,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Done,
        }),
    );
    sim.run_until(80_000);
    assert_clean(&sim);
    let asked = transcript(&sim).join("\n");
    assert!(
        asked.contains("More detail on the arena."),
        "the words reached the talker: {asked}"
    );
}

/// The user's shapes are obstacles: a diagram is staged clear of a box they drew.
#[test]
fn the_users_shapes_are_obstacles() {
    let narration = concat!(
        "<diagram id=\"m\" title=\"Map\"><node id=\"a\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\"/><edge from=\"a\" to=\"b\"/></diagram>",
        "Here <reveal id=\"a\"/>is A, and <reveal id=\"b\"/>B after it."
    );
    let mut sim = Sim::new(config());
    sim.at(
        0,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    let yours = Rect {
        x: 0,
        y: 0,
        w: 300,
        h: 200,
    };
    sim.at(
        500,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Shape {
                layer: MAIN,
                drawn: DrawnShape::Box { rect: yours },
            },
        }),
    );
    sim.at(
        1_000,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Done,
        }),
    );
    sim.talker.push(ScriptedReply::says(narration));
    sim.run_until(30_000);
    assert_clean(&sim);
    let staged = sim.core().board().staged(MAIN);
    assert!(
        !staged[0].bounds.intersects(yours),
        "staged clear of the box"
    );
    assert!(shown(&sim, MAIN, "b"));
}

/// A box the user draws over a planned part after staging: the part stays planned rather than
/// land on it (I6 for the user's shapes), counted, and so does its connector.
#[test]
fn a_part_under_a_shape_drawn_after_staging_stays_planned() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        "<diagram id=\"m\" title=\"Map\"><node id=\"a\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\"/><edge from=\"a\" to=\"b\"/></diagram>",
        "Here <reveal id=\"a\"/>is A, and after a long pause, a pause long enough to draw in, ",
        "<reveal id=\"b\"/>B."
    )));
    sim.user_says(0, "Map it.");
    assert!(sim.run_until_true(30_000, |sim| !reveals(sim).is_empty()));
    let b = sim.core().board().staged(MAIN)[0]
        .entries
        .iter()
        .find(|entry| entry.id.as_str() == "b")
        .map(|entry| match entry.draw {
            StagedPart::Node { rect, .. } => rect,
            _ => Rect::default(),
        })
        .expect("b");
    let at = sim.now().as_ms() + 50;
    sim.at(
        at,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Shape {
                layer: MAIN,
                drawn: DrawnShape::Box { rect: b },
            },
        }),
    );
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert!(!shown(&sim, MAIN, "b"), "B never lands on the user's box");
    assert!(!shown(&sim, MAIN, "a>b"));
    assert_eq!(dropped(&sim, Dropped::DiagramPart), 1);
}

/// Past its limits a diagram drops what doesn't fit and counts it: parts past
/// `DIAGRAM_PARTS_MAX`, a container nested too deep (what it held moves up), a fifth category,
/// a connector naming nothing. The talker is told.
#[test]
fn a_diagram_past_its_limits_drops_and_counts() {
    let mut narration = String::from("<diagram id=\"m\" title=\"Big\">");
    for depth in 0..8 {
        let parent = if depth == 0 {
            String::new()
        } else {
            format!(" in=\"g{}\"", depth - 1)
        };
        narration.push_str(&format!(
            "<node id=\"g{depth}\" kind=\"group\" label=\"level {depth}\"{parent}/>"
        ));
    }
    for (index, name) in ["one", "two", "three", "four", "five"].iter().enumerate() {
        narration.push_str(&format!(
            "<node id=\"c{index}\" kind=\"group\" label=\"{name}\" cat=\"{name}\"/>"
        ));
    }
    narration.push_str("<edge from=\"c0\" to=\"ghost\"/>");
    for index in 0..DIAGRAM_PARTS_MAX {
        narration.push_str(&format!("<node id=\"n{index}\" label=\"n\" in=\"g7\"/>"));
    }
    narration.push_str("</diagram><reveal id=\"m\" deep=\"true\"/>That is the whole thing.");
    let mut sim = run(&[&narration, "Yes."], "Draw it all.", 60_000);
    sim.user_says(60_000, "Is that all of it?");
    sim.run_until(80_000);
    assert_clean(&sim);
    let parts_dropped = dropped(&sim, Dropped::DiagramPart);
    // 14 parts past the limit in the block, then in staging: two containers too deep, a fifth
    // category, the connector to nothing.
    assert!(parts_dropped >= 18, "{parts_dropped}");
    let told = transcript(&sim).join("\n");
    assert!(
        told.contains("parts of your diagram \"Big\" were dropped"),
        "{told}"
    );
    let staged = &sim.core().board().staged(MAIN)[0];
    let deepest = staged
        .entries
        .iter()
        .position(|entry| entry.id.as_str() == "n0")
        .map(|at| staged.ancestors(at).len())
        .expect("n0 kept");
    assert_eq!(
        deepest,
        boardai_core::limits::DIAGRAM_NESTING_MAX,
        "moved up into g5"
    );
}

/// A diagram nothing in its reply reveals is revealed whole as its words play, and a drawing tag
/// naming a staged part reveals it: a staged diagram never stays empty.
#[test]
fn a_diagram_nobody_reveals_is_drawn_whole_and_a_node_tag_reveals_its_part() {
    let unrevealed = concat!(
        "Here is the map. <diagram id=\"m\" title=\"Map\"><node id=\"a\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\"/></diagram>It has two parts."
    );
    let sim = run(&[unrevealed], "Map it.", 30_000);
    assert_clean(&sim);
    assert_eq!(reveals(&sim), [vec!["a".to_owned(), "b".to_owned()]]);
    let named = concat!(
        "<diagram id=\"m\" title=\"Map\"><node id=\"a\" label=\"A\"/>",
        "<node id=\"b\" label=\"B\"/></diagram>First <node id=\"b\"/>B, then ",
        "<node id=\"a\" label=\"A\"/>A."
    );
    let sim = run(&[named], "Map it.", 30_000);
    assert_clean(&sim);
    assert_eq!(reveals(&sim), [vec!["b".to_owned()], vec!["a".to_owned()]]);
}

const _: () = assert!(
    DIAGRAM_PARTS_MAX >= 150,
    "the inf-alloc map fits one diagram"
);

/// The replay fixture the UI is checked against (artboards 15 and 15b): the inf-alloc map
/// revealed, then "more detail on the arena", then "back out". `BOARDAI_WRITE_FIXTURES=1`
/// rewrites it; the test fails while the committed copy is stale.
#[test]
fn the_staged_replay_fixture_is_current() {
    use boardai_core::protocol::{UiLogKind, UiLogLine};
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(INF_ALLOC));
    sim.talker.push(ScriptedReply::says(ARENA_INSIDE));
    sim.user_says(0, "Walk me through how inf-alloc hands out memory.");
    sim.user_says(56_000, "More detail on the arena.");
    sim.user_says(90_000, "Back out.");
    sim.run_until(100_000);
    assert_clean(&sim);
    let mut jsonl = String::new();
    for (at, event) in sim.ui() {
        let line = UiLogLine {
            v: 1,
            t: at,
            k: UiLogKind::Ui,
            event: event.clone(),
        };
        jsonl.push_str(&serde_json::to_string(&line).expect("serializes"));
        jsonl.push('\n');
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/public/fixtures/staged.events.jsonl"
    );
    if std::env::var_os("BOARDAI_WRITE_FIXTURES").is_some() {
        std::fs::write(path, &jsonl).expect("writes the fixture");
        return;
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == jsonl,
        "ui/public/fixtures/staged.events.jsonl is stale: run BOARDAI_WRITE_FIXTURES=1 cargo test \
         -p boardai-core --test sim_staged"
    );
}
