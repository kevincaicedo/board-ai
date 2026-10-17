//! Simulator scenarios: you draw too (artboard 13). You ask to draw by voice or with the pen,
//! BoardAI stops and watches with no talker request (I14), what you say labels your shapes, and
//! "done" describes the drawing to the talker, which draws beside it and never over it (I6).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Dropped, Effect, LogRecord};
use boardai_core::engine::text::DRAW_GO_AHEAD_LINE;
use boardai_core::geometry::{Point, Rect};
use boardai_core::history::{Message, UserPart};
use boardai_core::ids::{ElementId, LayerId};
use boardai_core::input::Input;
use boardai_core::protocol::{
    DrawAction, DrawOp, DrawnShape, MicAction, OrbState, UiCommand, UiEvent, UserShape,
    UserShapeKind,
};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::{CoreConfig, MicMode};
use common::{DEMO, assert_clean, config, count, op_starts, orbs, transcript};

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

fn pen(points: &[(i32, i32)]) -> DrawnShape {
    DrawnShape::Pen {
        points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
    }
}

fn id(text: &str) -> ElementId {
    ElementId::parse(text).expect("a valid id")
}

/// Your shapes as last echoed, by id.
fn yours(sim: &Sim) -> Vec<UserShape> {
    let mut shapes: Vec<UserShape> = Vec::new();
    for (_, event) in sim.ui() {
        match event {
            UiEvent::UserShape { shape, .. } => {
                shapes.retain(|kept| kept.id != shape.id);
                shapes.push(shape.clone());
            }
            UiEvent::UserShapeRemove { id, .. } => shapes.retain(|kept| kept.id != *id),
            _ => {}
        }
    }
    shapes.sort_by(|a, b| a.id.cmp(&b.id));
    shapes
}

fn modes(sim: &Sim) -> Vec<bool> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::DrawMode { drawing } => Some(*drawing),
            _ => None,
        })
        .collect()
}

fn said_line(sim: &Sim) -> usize {
    count(
        sim,
        |effect| matches!(effect, Effect::Synthesize { text, .. } if text.starts_with("Go ahead.")),
    )
}

/// The text of the last user message of request `index`.
fn asked(sim: &Sim, index: usize) -> String {
    let request = sim.talker.requests.get(index).expect("that request");
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => Some(text.clone()),
                UserPart::ToolResult { .. } => None,
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .expect("a user message")
}

fn dropped_shapes(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(
            effect,
            Effect::Log(LogRecord::Dropped {
                what: Dropped::UserShape | Dropped::DrawCommand,
                ..
            })
        )
    })
}

/// Draws artboard 13 from `at`: Client, an arrow, API, an arrow, Postgres with the pen (body
/// and lid), and a cache under the API, labelling them by voice. Returns when it is done.
fn draw_the_artboard(sim: &mut Sim, at: u64) -> u64 {
    shape(sim, at, boxed(40, 300, 120, 70));
    sim.user_says(at + 500, "This is the client.");
    shape(sim, at + 2_500, arrow(160, 335, 290, 335));
    shape(sim, at + 3_000, boxed(300, 300, 140, 80));
    sim.user_says(at + 3_500, "And this one is the API.");
    shape(sim, at + 6_000, arrow(440, 340, 590, 340));
    let body = [(600, 290), (690, 290), (690, 390), (600, 390), (600, 292)];
    shape(sim, at + 6_500, pen(&body));
    shape(sim, at + 6_700, pen(&[(600, 300), (645, 312), (690, 300)]));
    sim.user_says(at + 7_000, "Postgres.");
    sim.user_says(at + 8_000, "source of truth");
    shape(sim, at + 9_500, boxed(310, 480, 120, 60));
    sim.user_says(at + 10_000, "and this one is the cache");
    at + 12_500
}

const BLOCK: &str = "[The user drew on the board: u1 \"client\" → u3 \"API\" → u5 \"Postgres\" \
                     (source of truth); u6 \"cache\" under u3, not connected yet]";

const ANSWER: &str = concat!(
    r#"Right, the <annotate target="u3" mark="circle" text="hot spot"/>API is where it all "#,
    r#"meets, and the <edge from="u6" to="u3" style="dashed"/>cache would sit beside it."#
);

/// The whole flow: "wait, let me draw" mid-explanation is an interruption; BoardAI says its
/// line and waits with no request while you draw and label; your question finishes the drawing
/// and the talker gets it described, then marks your API and joins your cache to it.
#[test]
fn draw_label_and_ask_and_boardai_answers_beside_your_shapes() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(ANSWER));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let cut = sim.now().as_ms() + 200;
    sim.user_says(cut, "Wait, let me draw what I mean.");
    assert!(sim.run_until_true(cut + 5_000, |sim| sim.core().is_drawing()));
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 1);
    assert!(
        sim.ui()
            .any(|(_, event)| *event == UiEvent::Freeze { layer: MAIN })
    );
    let heard = transcript(&sim);
    assert!(
        heard
            .iter()
            .any(|line| line.starts_with("assistant(heard): ")),
        "what was heard is committed: {heard:#?}"
    );
    assert_eq!(said_line(&sim), 1);
    let start = sim.now().as_ms() + 5_000;
    let finished = draw_the_artboard(&mut sim, start);
    sim.run_until(finished);
    assert_eq!(sim.talker.requests.len(), 1, "no request while you draw");
    assert!(sim.core().is_drawing());
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Listening));
    sim.user_says(finished, "Does this make sense?");
    sim.run_until(finished + 40_000);
    assert_clean(&sim);
    assert_eq!(modes(&sim), vec![true, false]);
    assert_eq!(sim.talker.requests.len(), 2);
    let message = asked(&sim, 1);
    assert!(message.starts_with(BLOCK), "{message}");
    assert!(
        message.contains("Wait, let me draw what I mean. Does this make sense?"),
        "{message}"
    );
    let labels: Vec<(String, Option<String>, Option<String>)> = yours(&sim)
        .into_iter()
        .map(|shape| (shape.id.as_str().to_owned(), shape.label, shape.note))
        .collect();
    assert_eq!(labels.len(), 6, "{labels:?}");
    let marked = op_starts(&sim).iter().any(|(_, event)| {
        matches!(event, UiEvent::OpStart { op: DrawOp::Annotate { target, .. }, .. } if *target == id("u3"))
    });
    let joined = op_starts(&sim).iter().any(|(_, event)| {
        matches!(event, UiEvent::OpStart { op: DrawOp::Edge { from, to, .. }, .. } if *from == id("u6") && *to == id("u3"))
    });
    assert!(marked && joined, "{:#?}", op_starts(&sim));
    assert_eq!(*sim.core().state(), ConversationState::Idle);
}

/// The pen pressed while BoardAI speaks is an interruption with no words: the voice and the
/// stream stop, the stroke in flight freezes, what was heard is committed, and BoardAI says its
/// line and waits.
#[test]
fn the_pen_while_boardai_speaks_stops_it_like_an_interruption() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let at = sim.now().as_ms() + 100;
    draw(&mut sim, at, DrawAction::Start);
    sim.run_until(at + 8_000);
    assert_clean(&sim);
    assert!(sim.core().is_drawing());
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 1);
    assert!(count(&sim, |effect| *effect == Effect::CancelTalk) >= 1);
    assert!(
        sim.ui()
            .any(|(_, event)| *event == UiEvent::Freeze { layer: MAIN })
    );
    assert!(
        transcript(&sim)
            .iter()
            .any(|line| line.starts_with("assistant(heard): "))
    );
    assert_eq!(said_line(&sim), 1);
    assert_eq!(sim.talker.requests.len(), 1);
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Listening));
}

/// While you say a label, a hint names the shape it will land on; it clears once the label
/// lands.
#[test]
fn a_label_being_said_shows_the_hint_under_the_newest_shape() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 5_000, boxed(0, 0, 120, 60));
    sim.user_says(6_000, "and this one is the cache");
    sim.run_until(10_000);
    assert_clean(&sim);
    let hints: Vec<Option<String>> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::LabelHint { hint } => {
                Some(hint.as_ref().map(|hint| hint.target.as_str().to_owned()))
            }
            _ => None,
        })
        .collect();
    assert!(
        hints
            .first()
            .is_some_and(|hint| hint.as_deref() == Some("u1")),
        "{hints:?}"
    );
    assert_eq!(hints.last(), Some(&None));
    assert_eq!(yours(&sim)[0].label.as_deref(), Some("cache"));
    assert_eq!(sim.talker.requests.len(), 0);
}

/// Undo by button and by voice takes the newest shape of this drawing first; nothing to undo is
/// counted, not an error.
#[test]
fn undo_takes_back_the_newest_shapes() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 120, 60));
    shape(&mut sim, 4_100, boxed(300, 0, 120, 60));
    shape(&mut sim, 4_200, boxed(600, 0, 120, 60));
    draw(&mut sim, 4_300, DrawAction::Undo);
    sim.user_says(5_000, "undo that");
    sim.run_until(8_000);
    draw(&mut sim, 8_000, DrawAction::Undo);
    draw(&mut sim, 8_100, DrawAction::Undo);
    sim.run_until(9_000);
    assert_clean(&sim);
    assert!(yours(&sim).is_empty());
    assert_eq!(dropped_shapes(&sim), 1, "the fourth undo had nothing left");
    assert_eq!(sim.talker.requests.len(), 0);
}

/// Mute while drawing: the drawing stays and your pen still works; Done can't be answered while
/// muted, and after unmuting "done" asks BoardAI.
#[test]
fn mute_keeps_the_drawing_and_done_waits_for_the_microphone() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("That's a client and an API."));
    draw(&mut sim, 0, DrawAction::Start);
    sim.mic(4_000, MicAction::Mute);
    shape(&mut sim, 4_500, boxed(0, 0, 120, 60));
    draw(&mut sim, 5_000, DrawAction::Done);
    sim.run_until(6_000);
    assert!(sim.core().is_drawing());
    assert_eq!(*sim.core().state(), ConversationState::Muted);
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Muted));
    assert_eq!(dropped_shapes(&sim), 1, "Done while muted");
    sim.mic(6_000, MicAction::Unmute);
    shape(&mut sim, 6_500, boxed(300, 0, 120, 60));
    sim.user_says(7_000, "Done.");
    sim.run_until(20_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 1);
    let message = asked(&sim, 0);
    assert!(
        message.starts_with(
            "[The user drew on the board: u1 (a box, no label) left of u2, not connected yet; u2"
        ),
        "{message}"
    );
}

/// Muting while BoardAI says its line stops it; the drawing goes on.
#[test]
fn mute_during_the_line_stops_it_and_keeps_drawing() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    assert!(sim.run_until_true(5_000, |sim| *sim.core().state()
        == ConversationState::Speaking));
    let at = sim.now().as_ms() + 50;
    sim.mic(at, MicAction::Mute);
    sim.run_until(at + 2_000);
    assert_clean(&sim);
    assert!(sim.core().is_drawing());
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 1);
}

/// Esc (Cancel) or "never mind" leaves drawing without asking; your shapes stay, and the talker
/// sees them in the board summary with a note that it wasn't asked.
#[test]
fn esc_leaves_drawing_without_asking_and_the_shapes_stay() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Sure."));
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 300, 120, 60));
    sim.user_says(4_500, "the queue");
    draw(&mut sim, 7_000, DrawAction::Cancel);
    sim.run_until(8_000);
    assert!(!sim.core().is_drawing());
    assert_eq!(sim.talker.requests.len(), 0);
    draw(&mut sim, 8_000, DrawAction::Start);
    sim.user_says(12_000, "never mind");
    sim.run_until(14_000);
    assert!(!sim.core().is_drawing());
    sim.user_says(15_000, "What's a good queue for this?");
    sim.run_until(25_000);
    assert_clean(&sim);
    assert_eq!(modes(&sim), vec![true, false, true, false]);
    let told = sim.talker.requests[0].messages.iter().any(|message| {
        matches!(message, Message::System { text }
            if text.contains("drawn by the user: u1 \"queue\"")
                && text.contains("stopped drawing without asking"))
    });
    assert!(told, "{:#?}", sim.talker.requests[0].messages);
    assert_eq!(yours(&sim).len(), 1);
}

/// Push-to-talk: labels are spoken while the key is held; voice without it is ignored.
#[test]
fn push_to_talk_labels_while_the_key_is_held() {
    let mut sim = Sim::new(CoreConfig {
        mic: MicMode::PushToTalk,
        ..config()
    });
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 120, 60));
    sim.user_says(4_500, "the cache");
    sim.mic(6_000, MicAction::TalkDown);
    sim.user_says(6_100, "this is the API");
    sim.mic(7_500, MicAction::TalkUp);
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(yours(&sim)[0].label.as_deref(), Some("API"));
    assert!(sim.core().is_drawing());
}

/// The pen pressed while BoardAI thinks: the request is cancelled, and the question, of which
/// nothing was heard, comes back with the drawing.
#[test]
fn drawing_while_boardai_thinks_keeps_the_question_for_the_drawing() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 3_000;
    sim.talker.push(ScriptedReply::says("This never plays."));
    sim.talker
        .push(ScriptedReply::says("A cache in front, yes."));
    sim.user_says(0, "How would you cache this?");
    assert!(sim.run_until_true(10_000, |sim| *sim.core().state()
        == ConversationState::Thinking));
    let at = sim.now().as_ms() + 50;
    draw(&mut sim, at, DrawAction::Start);
    shape(&mut sim, at + 4_000, boxed(0, 0, 120, 60));
    draw(&mut sim, at + 5_000, DrawAction::Done);
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 2);
    let message = asked(&sim, 1);
    assert!(
        message.starts_with("[The user drew on the board: u1 (a box, no label)")
            && message.contains("How would you cache this?"),
        "{message}"
    );
    let users = transcript(&sim)
        .iter()
        .filter(|line| line.starts_with("user: "))
        .count();
    assert_eq!(users, 1, "the question is one turn with the drawing");
}

/// A change waiting for a yes is closed by drawing (I9): a later yes runs nothing.
#[test]
fn drawing_closes_a_change_waiting_for_a_yes() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("I can add jitter.").calls(
            "propose_change",
            r#"{"instruction":"Add jitter to the retries"}"#,
        ));
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.talker.push(ScriptedReply::says("Yes."));
    sim.user_says(0, "Can you add jitter to the retries?");
    assert!(sim.run_until_true(60_000, |sim| {
        *sim.core().state() == ConversationState::Idle && sim.talker.requests.len() == 2
    }));
    let at = sim.now().as_ms() + 500;
    draw(&mut sim, at, DrawAction::Start);
    draw(&mut sim, at + 4_000, DrawAction::Cancel);
    sim.user_says(at + 5_000, "yes");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    let ran = count(&sim, |effect| matches!(effect, Effect::RunHarness { .. }));
    assert_eq!(ran, 0);
    let told = sim.talker.requests.iter().any(|request| {
        request.messages.iter().any(|message| {
            matches!(message, Message::System { text } if text.contains("started drawing instead of answering"))
        })
    });
    assert!(told);
}

/// A background task's report waits while you draw, and goes out once you finish.
#[test]
fn a_harness_report_waits_until_the_drawing_is_done() {
    let mut sim = Sim::new(config());
    sim.harness.run_ms = 6_000;
    sim.talker
        .push(ScriptedReply::says("I'll look into it.").calls(
            "research",
            r#"{"question":"Where is the TTL?","depth":"quick"}"#,
        ));
    sim.talker.push(ScriptedReply::says("It's running."));
    sim.talker.push(ScriptedReply::says("Nice drawing."));
    sim.talker
        .push(ScriptedReply::says("The research is back."));
    sim.user_says(0, "Where do we set the TTL?");
    sim.run_until(4_000);
    let at = sim.now().as_ms();
    draw(&mut sim, at, DrawAction::Start);
    shape(&mut sim, at + 3_000, boxed(0, 0, 120, 60));
    sim.run_until(at + 15_000);
    let while_drawing = sim.talker.requests.len();
    assert!(
        sim.ui()
            .any(|(_, event)| matches!(event, UiEvent::DrawMode { drawing: true })),
        "drawing started"
    );
    draw(&mut sim, at + 15_000, DrawAction::Done);
    sim.run_until(at + 60_000);
    assert_clean(&sim);
    assert!(
        sim.talker.requests.len() > while_drawing,
        "the report went out after"
    );
    let reported_at = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .map(|(at, _)| at.as_ms())
        .last()
        .expect("a report turn");
    assert!(reported_at >= at + 15_000);
}

/// The talker can point at your shapes and draw beside them, never redraw, strike or rewrite
/// one: those ops are dropped and counted.
#[test]
fn the_talker_cannot_redraw_or_strike_your_shapes() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"Your <node id="u1" label="Gateway"/>box, <strike target="u1" text="Gateway"/>"#,
        r#"renamed, <write target="u1" text="x"/>and <node id="db" label="Postgres" "#,
        r#"kind="db" near="u1" dir="right"/>a database beside it."#
    )));
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 120, 60));
    draw(&mut sim, 5_000, DrawAction::Done);
    sim.run_until(30_000);
    assert_clean(&sim);
    let drawn: Vec<String> = op_starts(&sim)
        .iter()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Node { id, .. },
                ..
            } => Some(id.as_str().to_owned()),
            UiEvent::OpStart {
                op: DrawOp::Strike { .. } | DrawOp::Write { .. },
                ..
            } => Some("rewrite".to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(drawn, vec!["db".to_owned()]);
    assert!(common::dropped(&sim) >= 3);
}

/// Shapes the core refuses (a click, a stroke past its point bound) get no echo; one finished
/// just after "done" still lands.
#[test]
fn refused_shapes_get_no_echo_and_a_late_one_still_lands() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Okay."));
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 2, 2));
    let long: Vec<(i32, i32)> = (0..600).map(|x| (x, 0)).collect();
    shape(&mut sim, 4_100, pen(&long));
    draw(&mut sim, 4_200, DrawAction::Done);
    shape(&mut sim, 4_300, boxed(0, 0, 120, 60));
    sim.run_until(15_000);
    assert_clean(&sim);
    assert_eq!(dropped_shapes(&sim), 2);
    let shapes = yours(&sim);
    assert_eq!(shapes.len(), 1);
    assert!(matches!(shapes[0].kind, UserShapeKind::Box { .. }));
    assert_eq!(
        sim.talker.requests.len(),
        0,
        "nothing was drawn when done came"
    );
}

/// "Let me draw" by voice while idle says the line once; asking again while drawing is nothing.
#[test]
fn asking_to_draw_twice_says_the_line_once() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "Let me draw it.");
    sim.run_until(8_000);
    sim.user_says(8_000, "let me draw it");
    sim.run_until(12_000);
    assert_clean(&sim);
    assert_eq!(said_line(&sim), 1);
    assert_eq!(modes(&sim), vec![true]);
    let spoken = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::Caption { text, is_final: true, .. } if text == DRAW_GO_AHEAD_LINE)
    });
    assert!(spoken);
    assert_eq!(sim.talker.requests.len(), 0);
}

/// A view never hides your drawing: starting to draw closes it first.
#[test]
fn starting_to_draw_closes_an_open_view() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Here are your sessions.").calls("show_library", "{}"));
    sim.talker
        .push(ScriptedReply::says("Raft is where you left off."));
    sim.user_says(0, "show me my sessions");
    sim.run_until(10_000);
    assert!(sim.core().view().is_some());
    draw(&mut sim, 10_000, DrawAction::Start);
    sim.run_until(12_000);
    assert_clean(&sim);
    assert_eq!(sim.core().view(), None);
    let order: Vec<&str> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::ViewDismiss => Some("dismiss"),
            UiEvent::DrawMode { drawing: true } => Some("draw"),
            _ => None,
        })
        .collect();
    assert_eq!(order, vec!["dismiss", "draw"]);
}

/// Ending the session, opening settings and going off the record mean the same while you draw;
/// they never become labels, and none asks the talker (I14).
#[test]
fn session_controls_still_work_while_drawing() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    shape(&mut sim, 4_000, boxed(0, 0, 120, 60));
    sim.user_says(5_000, "open settings");
    sim.user_says(8_000, "don't remember this session");
    sim.run_until(12_000);
    assert!(yours(&sim)[0].label.is_none());
    assert!(sim.ui().any(|(_, event)| *event == UiEvent::OpenSettings));
    assert!(sim.core().is_off_record());
    sim.user_says(12_000, "end the session");
    sim.run_until(20_000);
    assert_clean(&sim);
    assert!(sim.ui().any(|(_, event)| *event == UiEvent::SessionEnd));
    assert!(yours(&sim)[0].label.is_none());
    assert_eq!(sim.talker.requests.len(), 0);
}
