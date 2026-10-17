//! Simulator scenarios: what you say while you draw (artboard 13), and what a drawing's turn
//! asks. Each was a review finding (round 2): a remark that finished the drawing or became a
//! label, words that never reached the talker, and a question that came back after Esc.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::geometry::Rect;
use boardai_core::history::{Message, UserPart};
use boardai_core::ids::LayerId;
use boardai_core::input::Input;
use boardai_core::protocol::{DrawAction, DrawnShape, UiCommand, UiEvent, UserShape};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{assert_clean, config};

fn draw(sim: &mut Sim, at_ms: u64, action: DrawAction) {
    sim.at(at_ms, Input::Command(UiCommand::Draw { action }));
}

fn a_box(sim: &mut Sim, at_ms: u64) {
    let drawn = DrawnShape::Box {
        rect: Rect {
            x: 40,
            y: 300,
            w: 120,
            h: 70,
        },
    };
    draw(
        sim,
        at_ms,
        DrawAction::Shape {
            layer: LayerId::MAIN,
            drawn,
        },
    );
}

/// The words of request `index`'s last user message.
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

fn last_echo(sim: &Sim) -> UserShape {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::UserShape { shape, .. } => Some(shape.clone()),
            _ => None,
        })
        .last()
        .expect("an echo")
}

/// "Wait, I'm not done." ends on "done", but it says the opposite: the drawing goes on.
#[test]
fn not_done_does_not_finish_the_drawing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Nice drawing."));
    draw(&mut sim, 0, DrawAction::Start);
    a_box(&mut sim, 500);
    sim.user_says(3_000, "Wait, I'm not done.");
    sim.user_says(6_000, "I'm almost done");
    sim.run_until(15_000);
    assert_clean(&sim);
    assert!(sim.core().is_drawing(), "the drawing finished");
    assert!(sim.talker.requests.is_empty(), "the talker was asked");
    assert_eq!(last_echo(&sim).label, None, "a remark became a label");
}

/// A stall ("Wait a second.") takes no label, so the label said next is the box's label, not
/// its note.
#[test]
fn a_stall_phrase_never_takes_the_label() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    a_box(&mut sim, 500);
    sim.user_says(3_000, "Wait a second.");
    sim.user_says(6_000, "This is the cache.");
    sim.run_until(10_000);
    assert_clean(&sim);
    let shape = last_echo(&sim);
    assert_eq!(shape.label.as_deref(), Some("cache"));
    assert_eq!(shape.note, None);
}

/// A bare label ("Redis") is replaced by a label said with a lead-in ("this is the cache"),
/// which is the clearer naming; a second lead-in label is the note, as before.
#[test]
fn a_label_said_with_a_lead_in_replaces_a_bare_one() {
    let mut sim = Sim::new(config());
    draw(&mut sim, 0, DrawAction::Start);
    a_box(&mut sim, 500);
    sim.user_says(3_000, "Redis.");
    sim.user_says(6_000, "Actually, this is the cache.");
    sim.user_says(9_000, "That's the source of truth.");
    sim.run_until(14_000);
    assert_clean(&sim);
    let shape = last_echo(&sim);
    assert_eq!(shape.label.as_deref(), Some("cache"));
    assert_eq!(shape.note.as_deref(), Some("source of truth"));
}

/// Whisper ends a trailing-off utterance with "…"; the words said after it still reach the
/// talker with the drawing.
#[test]
fn words_after_a_trailing_off_utterance_still_reach_the_talker() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Yes, it does."));
    draw(&mut sim, 0, DrawAction::Start);
    a_box(&mut sim, 4_000);
    sim.user_says(5_000, "so the request goes through the gateway and then…");
    sim.user_says(15_000, "Does this make sense?");
    sim.run_until(30_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 1);
    let words = asked(&sim, 0);
    assert!(words.contains("gateway and then…"), "{words}");
    assert!(words.contains("Does this make sense?"), "{words}");
}

/// LLD §2.2: nothing drawn and nothing said asks nothing, also when an earlier drawing left
/// shapes on the board.
#[test]
fn an_empty_second_drawing_asks_nothing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Nice drawing."));
    draw(&mut sim, 0, DrawAction::Start);
    a_box(&mut sim, 500);
    draw(&mut sim, 1_000, DrawAction::Done);
    sim.run_until(20_000);
    assert_eq!(sim.talker.requests.len(), 1, "the first drawing asks once");
    draw(&mut sim, 21_000, DrawAction::Start);
    draw(&mut sim, 25_000, DrawAction::Done);
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 1, "the empty drawing asked");
}

/// D while BoardAI thinks, then Esc once the go-ahead line has played: the question nothing
/// was heard of is dropped, as Esc drops it while the line plays, and never joins the next one.
#[test]
fn a_question_abandoned_by_cancelling_a_drawing_does_not_join_the_next_question() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 3_000;
    sim.talker.push(ScriptedReply::says("This never plays."));
    sim.talker.push(ScriptedReply::says("TCP is a stream."));
    sim.user_says(0, "How does DNS resolution work?");
    assert!(sim.run_until_true(10_000, |sim| {
        *sim.core().state() == ConversationState::Thinking
    }));
    let at = sim.now().as_ms() + 50;
    draw(&mut sim, at, DrawAction::Start);
    draw(&mut sim, at + 9_000, DrawAction::Cancel);
    sim.user_says(at + 12_000, "What about TCP?");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 2);
    let words = asked(&sim, 1);
    assert!(
        !words.contains("DNS"),
        "the dropped question came back: {words}"
    );
}

/// "Let me draw it" said while BoardAI thinks is the drawing cue, not words joined to the
/// question; the question waits for the drawing's turn, which asks it.
#[test]
fn let_me_draw_said_while_boardai_thinks_starts_drawing() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 3_000;
    sim.talker.push(ScriptedReply::says("This never plays."));
    sim.talker.push(ScriptedReply::says("Sure, like that."));
    sim.user_says(0, "How would you cache this?");
    assert!(sim.run_until_true(10_000, |sim| {
        *sim.core().state() == ConversationState::Thinking
    }));
    let at = sim.now().as_ms() + 50;
    sim.user_says(at, "Let me draw it.");
    sim.run_until(at + 8_000);
    assert!(sim.core().is_drawing(), "not drawing");
    assert_eq!(sim.talker.requests.len(), 1, "the cue went to the talker");
    a_box(&mut sim, at + 9_000);
    draw(&mut sim, at + 10_000, DrawAction::Done);
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 2);
    let words = asked(&sim, 1);
    assert!(words.contains("How would you cache this?"), "{words}");
    assert!(words.contains("[The user drew on the board"), "{words}");
}

/// "Cancel that" while drawing leaves the drawing and never cancels a running task; a cancel
/// that names its task stops it, and its words still reach the talker when the drawing ends.
#[test]
fn cancel_that_while_drawing_leaves_the_drawing_and_spares_the_research() {
    use boardai_core::effect::Effect;
    use common::count;
    let mut sim = Sim::new(config());
    sim.harness.run_ms = 120_000;
    sim.talker
        .push(ScriptedReply::says("I'll look into it.").calls(
            "research",
            r#"{"question":"where is the TTL","depth":"quick"}"#,
        ));
    sim.talker.push(ScriptedReply::says("It's running."));
    sim.talker
        .push(ScriptedReply::says("Stopped, and nice drawing."));
    sim.user_says(0, "Find where the TTL is set.");
    assert!(sim.run_until_true(30_000, |sim| {
        *sim.core().state() == ConversationState::Idle && sim.talker.requests.len() == 2
    }));
    let at = sim.now().as_ms() + 500;
    let cancels = |sim: &Sim| count(sim, |effect| matches!(effect, Effect::CancelHarness { .. }));
    draw(&mut sim, at, DrawAction::Start);
    sim.user_says(at + 6_000, "Cancel that.");
    sim.run_until(at + 9_000);
    assert!(!sim.core().is_drawing(), "\"cancel that\" left the drawing");
    assert_eq!(cancels(&sim), 0, "the research was cancelled");
    draw(&mut sim, at + 10_000, DrawAction::Start);
    a_box(&mut sim, at + 14_000);
    sim.user_says(at + 15_000, "Stop that research.");
    sim.run_until(at + 18_000);
    assert_eq!(cancels(&sim), 1);
    assert!(sim.core().is_drawing());
    draw(&mut sim, at + 19_000, DrawAction::Done);
    sim.run_until(at + 40_000);
    assert_clean(&sim);
    let words = asked(&sim, 2);
    assert!(words.contains("Stop that research."), "{words}");
}
