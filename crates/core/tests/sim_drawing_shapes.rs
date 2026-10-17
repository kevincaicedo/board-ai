//! Simulator scenarios: your shapes on the board (LLD §2.5 "Your shapes"). Each was a review
//! finding (round 2): undo that stopped working, a member of BoardAI's group drawn over your box
//! (I6), and a box too big for the board's grid.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Dropped, Effect, LogRecord};
use boardai_core::geometry::{Point, Rect};
use boardai_core::ids::LayerId;
use boardai_core::input::Input;
use boardai_core::limits::USER_SHAPE_SPAN_MAX;
use boardai_core::protocol::{DrawAction, DrawnShape, UiCommand, UiEvent, UserShapeKind};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{assert_clean, config, count};

const MAIN: LayerId = LayerId::MAIN;

fn draw(sim: &mut Sim, at_ms: u64, action: DrawAction) {
    sim.at(at_ms, Input::Command(UiCommand::Draw { action }));
}

fn shape(sim: &mut Sim, at_ms: u64, drawn: DrawnShape) {
    draw(sim, at_ms, DrawAction::Shape { layer: MAIN, drawn });
}

fn boxed(x: i32, y: i32, w: u32, h: u32) -> DrawnShape {
    DrawnShape::Box {
        rect: Rect { x, y, w, h },
    }
}

fn arrow(x1: i32, y1: i32, x2: i32, y2: i32) -> DrawnShape {
    DrawnShape::Arrow {
        start: Point { x: x1, y: y1 },
        end: Point { x: x2, y: y2 },
    }
}

/// An arrow drawn before its target connects to it when the target is drawn; undoing the target
/// then disconnects the arrow (re-sent with its end loose, where it was drawn) instead of being
/// refused forever, and undo goes on to the arrow and the first box.
#[test]
fn undo_works_after_an_arrow_drawn_before_its_target() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 120, 70));
    // From the first box to empty space: a loose end.
    shape(&mut sim, 4_500, arrow(120, 35, 300, 35));
    // The target, drawn after: the loose end lands on it and connects.
    shape(&mut sim, 5_000, boxed(290, 0, 120, 70));
    draw(&mut sim, 6_000, DrawAction::Undo);
    sim.run_until(6_100);
    let loose = sim.ui().filter_map(|(_, event)| match event {
        UiEvent::UserShape { shape, .. } => match &shape.kind {
            UserShapeKind::Arrow { to, end, .. } => Some((to.clone(), *end)),
            UserShapeKind::Box { .. } | UserShapeKind::Ink { .. } => None,
        },
        _ => None,
    });
    let last = loose.last().expect("the arrow was echoed");
    assert_eq!(
        last,
        (None, Point { x: 300, y: 35 }),
        "re-sent loose, as drawn"
    );
    draw(&mut sim, 6_500, DrawAction::Undo);
    draw(&mut sim, 7_000, DrawAction::Undo);
    sim.run_until(8_000);
    assert_clean(&sim);
    let removed = sim
        .ui()
        .filter(|(_, event)| matches!(event, UiEvent::UserShapeRemove { .. }))
        .count();
    assert_eq!(removed, 3, "three undos removed {removed} shapes");
}

/// I6: a group keeps every cell its frame covers for its members. A box you draw on such a cell
/// takes it, so the talker's next member is placed beside your box, never on it.
#[test]
fn a_group_member_never_lands_on_a_box_you_drew_inside_the_group() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<node id="vpc" label="VPC" kind="group"/>The VPC holds "#,
        r#"<node id="api" label="API" in="vpc"/>the API."#,
    )));
    sim.talker.push(ScriptedReply::says(
        r#"Right, the <node id="db" label="DB" in="vpc"/>database sits inside it too."#,
    ));
    sim.user_says(0, "What is in our VPC?");
    assert!(sim.run_until_true(30_000, |sim| {
        *sim.core().state() == ConversationState::Idle && sim.talker.requests.len() == 1
    }));
    let at = sim.now().as_ms() + 500;
    draw(&mut sim, at, DrawAction::Start);
    // The group's second cell (x 200 to 400, y 0 to 140), still kept for a member.
    shape(&mut sim, at + 4_000, boxed(210, 10, 180, 120));
    draw(&mut sim, at + 5_000, DrawAction::Done);
    sim.run_until(at + 40_000);
    assert_eq!(sim.talker.requests.len(), 2);
    assert_clean(&sim);
}

/// A box past `USER_SHAPE_SPAN_MAX` is refused where it enters, with no echo, and counted: the
/// grid entries it would take grow with its area.
#[test]
fn a_huge_box_is_refused_at_the_boundary() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    let span = USER_SHAPE_SPAN_MAX;
    shape(
        &mut sim,
        500,
        boxed(-1_000_000, -1_000_000, 2_000_000, 2_000_000),
    );
    shape(&mut sim, 1_000, boxed(0, 0, span + 1, 70));
    shape(&mut sim, 1_500, boxed(0, 0, 70, span + 1));
    shape(
        &mut sim,
        2_000,
        arrow(0, 0, i32::try_from(span).unwrap_or(0) + 1, 0),
    );
    sim.run_until(3_000);
    let echoes = sim
        .ui()
        .filter(|(_, event)| matches!(event, UiEvent::UserShape { .. }))
        .count();
    assert_eq!(echoes, 0, "a shape past the span was kept");
    let refused = count(&sim, |effect| {
        matches!(
            effect,
            Effect::Log(LogRecord::Dropped {
                what: Dropped::UserShape,
                ..
            })
        )
    });
    assert_eq!(refused, 4);
    // At the bound it is a shape like any other.
    shape(&mut sim, 3_500, boxed(0, 0, span, 70));
    sim.run_until(4_000);
    assert_clean(&sim);
    let echoes = sim
        .ui()
        .filter(|(_, event)| matches!(event, UiEvent::UserShape { .. }))
        .count();
    assert_eq!(echoes, 1);
}
