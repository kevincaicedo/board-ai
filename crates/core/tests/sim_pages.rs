//! Simulator scenarios for pages (D-177): "clear the board" and `<page/>` start a fresh board
//! without moving anything drawn (I6), the camera starts over (I11), detours close first (I4),
//! and the earlier page stays in the session's record and export.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use std::collections::BTreeSet;

use boardai_core::effect::Effect;
use boardai_core::engine::pages::{PAGE_TURNED_LINE, PAGES_FULL_LINE};
use boardai_core::history::Message;
use boardai_core::ids::LayerId;
use boardai_core::limits::PAGES_PER_SESSION_MAX;
use boardai_core::protocol::{DrawOp, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{DEMO, assert_clean, config, count, op_starts};

/// DNS on a fresh page, reusing an id the first page drew (`client`).
const DNS: &str = concat!(
    r#"A <node id="client" label="Laptop" kind="actor"/>laptop asks a "#,
    r#"<node id="resolver" label="Resolver" near="client"/><edge from="client" to="resolver"/>"#,
    r#"resolver for the address."#,
);

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

fn turns(sim: &Sim) -> Vec<(u16, Option<String>)> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::PageTurn { page, title } = event {
                Some((*page, title.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn said(sim: &Sim, line: &str) -> bool {
    sim.effects()
        .any(|(_, effect)| matches!(effect, Effect::Synthesize { text, .. } if text == line))
}

/// The system notes of the last talker request, joined.
fn last_notes(sim: &Sim) -> String {
    sim.talker
        .requests
        .last()
        .map(|request| {
            request
                .messages
                .iter()
                .filter_map(|message| match message {
                    Message::System { text } => Some(text.as_str()),
                    Message::User { .. } | Message::Assistant { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Node ids drawn on the main layer after the `index`-th event of the run.
fn main_nodes_after(sim: &Sim, index: usize) -> Vec<String> {
    sim.ui()
        .skip(index)
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                layer: LayerId::MAIN,
                op: DrawOp::Node { id, .. },
                ..
            } = event
            {
                Some(id.as_str().to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn page_turn_index(sim: &Sim) -> usize {
    sim.ui()
        .position(|(_, event)| matches!(event, UiEvent::PageTurn { .. }))
        .expect("a page turn")
}

/// The owner's words: the core turns the page and answers in two words, without the talker.
#[test]
fn clear_the_board_alone_turns_the_page_and_the_core_answers() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(DNS));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    sim.user_says(30_000, "Clean the current board and start fresh.");
    sim.run_until(36_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 1, "no talker round trip");
    assert!(said(&sim, PAGE_TURNED_LINE));
    assert_eq!(turns(&sim), vec![(2, None)]);
    let board = sim.core().board();
    assert_eq!(board.page_number(), 2);
    assert_eq!(board.earlier_pages().len(), 1);
    let main = board.layer(LayerId::MAIN).expect("main");
    assert!(main.elements().is_empty(), "a fresh page");
    // The next question draws on it, from scratch: the old `client` id draws again (I6 holds,
    // and its first stroke brings a new camera frame, I11).
    sim.user_says(36_000, "How does DNS find an address?");
    sim.run_until(60_000);
    assert_clean(&sim);
    let turned = page_turn_index(&sim);
    assert_eq!(main_nodes_after(&sim, turned), vec!["client", "resolver"]);
    let notes = last_notes(&sim);
    assert!(notes.contains("fresh, empty page (page 2)"), "{notes}");
    assert!(notes.contains("page 2 (earlier pages are kept"), "{notes}");
    let camera_after = sim.ui().skip(turned).position(
        |(_, event)| matches!(event, UiEvent::Camera { layer, .. } if *layer == LayerId::MAIN),
    );
    assert!(camera_after.is_some(), "the new page is framed afresh");
}

/// "Clear the board and explain DNS": the page turns before the talker hears the words, and the
/// talker answers on the fresh page.
#[test]
fn clear_the_board_and_a_request_turns_the_page_first() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(DNS));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    sim.user_says(30_000, "clear the board and explain DNS");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2);
    assert!(!said(&sim, PAGE_TURNED_LINE), "the talker answers");
    assert_eq!(turns(&sim), vec![(2, None)]);
    let turn_at = sim
        .ui()
        .find(|(_, event)| matches!(event, UiEvent::PageTurn { .. }))
        .map(|(at, _)| at)
        .expect("turned");
    let asked_at = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .map(|(at, _)| at)
        .nth(1)
        .expect("asked");
    assert!(
        turn_at <= asked_at,
        "the page turns before the talker answers"
    );
    assert!(last_notes(&sim).contains("Answer the rest of their words"));
    assert_eq!(
        main_nodes_after(&sim, page_turn_index(&sim)),
        vec!["client", "resolver"]
    );
}

/// `<page title="DNS"/>` mid-reply: what came before stays on page 1, what follows draws on a
/// fresh page 2, and the plan of the old topic goes.
#[test]
fn a_page_tag_turns_the_page_when_its_words_play() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(&format!(
        r#"{DEMO} <page title="DNS"/>Now, names. {DNS}"#
    )));
    sim.user_says(0, "Walk me through a request, then DNS.");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(turns(&sim), vec![(2, Some("DNS".to_owned()))]);
    let turned = page_turn_index(&sim);
    let before: Vec<String> = main_nodes_after(&sim, 0)
        .into_iter()
        .take_while(|id| id != "resolver")
        .collect();
    assert!(before.contains(&"lb".to_owned()));
    assert_eq!(main_nodes_after(&sim, turned), vec!["client", "resolver"]);
    let thread_after = sim.ui().skip(turned).find_map(|(_, event)| {
        if let UiEvent::Thread { steps, .. } = event {
            Some(steps.clone())
        } else {
            None
        }
    });
    assert_eq!(thread_after, Some(Vec::new()), "the old plan goes");
    let earlier = sim.core().board().earlier_pages();
    assert_eq!(earlier.len(), 1);
    assert!(earlier[0].layer.element(&id("lb")).is_some(), "kept whole");
    assert_eq!(sim.core().board().page_title(), Some("DNS"));
}

fn id(text: &str) -> boardai_core::ids::ElementId {
    boardai_core::ids::ElementId::parse(text).expect("valid id")
}

/// A page turn closes every open detour first (I4), and the frozen stroke of the old page
/// stays there: the next question isn't told to finish it.
#[test]
fn a_page_turn_closes_open_detours_first() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>That's the cache-aside pattern. "#,
        r#"<node id="redis" label="Redis" kind="db"/>Reads go to Redis first."#,
    )));
    sim.talker.push(ScriptedReply::says(DNS));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| op_starts(sim).len() > 2));
    let at = sim.now().as_ms() + 50;
    sim.user_says_with_onset(at, "wait, why Redis first?");
    sim.run_until(at + 30_000);
    assert_eq!(sim.core().thread().depth(), 1);
    sim.user_says(at + 30_000, "start fresh with DNS please");
    sim.run_until(at + 60_000);
    assert_clean(&sim);
    assert_eq!(sim.core().thread().depth(), 0);
    let pop = sim
        .ui()
        .position(|(_, event)| matches!(event, UiEvent::LayerPop { .. }))
        .expect("the detour closed");
    assert!(pop < page_turn_index(&sim), "before the page turns");
    let resumed = sim.ui().skip(pop).any(
        |(_, event)| matches!(event, UiEvent::Resume { layer, .. } if *layer == LayerId::MAIN),
    );
    assert!(!resumed, "the old page's frozen stroke stays where it was");
    let asked = sim.talker.requests.last().expect("asked");
    let words = format!("{:?}", asked.messages.last());
    assert!(!words.contains("Drawing stopped"), "{words}");
}

/// "Clear the board" on a clean board changes nothing but still answers.
#[test]
fn clear_the_board_on_an_empty_board_changes_nothing() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "clear the board");
    sim.run_until(6_000);
    assert_clean(&sim);
    assert!(turns(&sim).is_empty());
    assert!(said(&sim, PAGE_TURNED_LINE));
    assert_eq!(talks(&sim), 0);
    assert_eq!(sim.core().board().page_number(), 1);
}

/// Past `PAGES_PER_SESSION_MAX` a page turn is refused: by tag it is dropped with a notice, by
/// voice BoardAI says so; the board stays as it is.
#[test]
fn pages_are_bounded_per_session() {
    let mut sim = Sim::new(config());
    let one_page = r#"<node id="a" label="A"/>Alpha. <page/>"#;
    let turns_max = usize::from(PAGES_PER_SESSION_MAX);
    sim.talker
        .push(ScriptedReply::says(&one_page.repeat(turns_max + 1)));
    sim.user_says(0, "Draw many pages.");
    sim.run_until(240_000);
    assert_clean(&sim);
    assert_eq!(turns(&sim).len(), turns_max - 1);
    assert_eq!(sim.core().board().page_number(), PAGES_PER_SESSION_MAX);
    sim.user_says(240_000, "clear the board");
    sim.run_until(250_000);
    assert!(said(&sim, PAGES_FULL_LINE));
    assert_eq!(turns(&sim).len(), turns_max - 1);
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(250_000, "What now?");
    sim.run_until(260_000);
    assert!(last_notes(&sim).contains("has used all"));
}

/// The session export keeps every page: a frame per page, then the detours opened on it.
#[test]
fn the_export_keeps_every_page() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(&format!(
        r#"{DEMO} <page title="DNS"/>Now, names. {DNS}"#
    )));
    sim.user_says(0, "Walk me through a request, then DNS.");
    sim.run_until(60_000);
    let text = sim.core().export_session_excalidraw();
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let elements = document["elements"].as_array().expect("elements");
    let names: Vec<&str> = elements
        .iter()
        .filter(|element| element["type"] == "frame")
        .filter_map(|frame| frame["name"].as_str())
        .collect();
    assert_eq!(names, vec!["Page 1", "Page 2: DNS"]);
    let ids: BTreeSet<&str> = elements
        .iter()
        .filter_map(|element| element["id"].as_str())
        .collect();
    assert_eq!(ids.len(), elements.len(), "ids stay unique across pages");
}

/// The user draws one box on the main board at (`x`, `y`), then puts the pen down (Esc).
fn user_draws_a_box(sim: &mut Sim, at: u64, (x, y): (i32, i32)) {
    use boardai_core::geometry::Rect;
    use boardai_core::input::Input;
    use boardai_core::protocol::{DrawAction, DrawnShape, UiCommand};
    let rect = Rect {
        x,
        y,
        w: 200,
        h: 120,
    };
    let actions = [
        DrawAction::Start,
        DrawAction::Shape {
            layer: LayerId::MAIN,
            drawn: DrawnShape::Box { rect },
        },
        DrawAction::Cancel,
    ];
    for (step, action) in (0..).zip(actions) {
        sim.at(at + step * 500, Input::Command(UiCommand::Draw { action }));
    }
    sim.run_until(at + 2_000);
}

fn on_main(sim: &Sim, target: &str) -> bool {
    sim.core()
        .board()
        .layer(LayerId::MAIN)
        .and_then(|main| main.element(&id(target)))
        .is_some()
}

/// The user's own shapes (artboard 13) turn with the page: kept in the earlier page with
/// BoardAI's drawing and in the export, never silently lost, and off the board the talker can
/// name. A later shape of the user's keeps counting ids (`u2`), and `u1` names nothing now.
#[test]
fn the_users_shapes_turn_with_the_page() {
    use boardai_core::board::ElementKind;
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    // Where the next page's first node will go: once the page turns, BoardAI may draw there.
    user_draws_a_box(&mut sim, 30_000, (0, 0));
    assert!(on_main(&sim, "u1"), "the user drew u1 on the main board");
    sim.user_says(32_000, "clear the board");
    sim.run_until(38_000);
    assert_eq!(turns(&sim), vec![(2, None)]);
    assert!(!on_main(&sim, "u1"), "off the board in front of the user");
    let kept = sim.core().board().earlier_pages()[0]
        .layer
        .element(&id("u1"));
    assert!(
        kept.is_some_and(|element| matches!(element.kind, ElementKind::User { .. })),
        "kept with the earlier page"
    );
    // The talker can name only what is on the current page: u1 draws nothing now.
    sim.talker.push(ScriptedReply::says(concat!(
        r#"Your <annotate target="u1" mark="circle"/>box is gone, so "#,
        r#"<node id="api" label="API"/>the API stands alone."#,
    )));
    sim.user_says(38_000, "What about my box?");
    sim.run_until(60_000);
    for (at, event) in sim.ui() {
        if matches!(
            event,
            UiEvent::UserShape { .. } | UiEvent::PageTurn { .. } | UiEvent::DrawMode { .. }
        ) || matches!(
            event,
            UiEvent::OpStart {
                op: DrawOp::Node { .. },
                ..
            }
        ) {
            eprintln!("DBG {} {:?}", at.as_ms(), event);
        }
    }
    assert_clean(&sim);
    let marked_u1 = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::OpStart { op: DrawOp::Annotate { target, .. }, .. }
            if target.as_str() == "u1")
    });
    assert!(!marked_u1);
    assert!(
        !last_notes(&sim).contains("u1"),
        "the summary shows only this page"
    );
    user_draws_a_box(&mut sim, 60_000, (900, 400));
    assert!(on_main(&sim, "u2"), "a new shape keeps counting");
    let export = sim.core().export_session_excalidraw();
    assert!(
        export.contains("\"Page 1\""),
        "the export keeps the earlier page"
    );
    assert!(export.contains(":u1"), "with the user's shape on it");
    assert_clean(&sim);
}
