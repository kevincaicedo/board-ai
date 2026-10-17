//! Simulator scenarios for asking the talker once more (D-178): a drawing was asked for and the
//! reply drew nothing, or a repo read was written as words instead of called (D-179). One retry
//! a turn, never more, and only when the reply really missed.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::protocol::UiEvent;
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::{DirEntry, ToolOutput};
use common::{assert_clean, config, count, op_starts};

const DESCRIBES: &str = "A binary search tree keeps smaller keys on the left and larger keys \
                         on the right, so each insert walks down from the root.";

const DRAWS: &str = concat!(
    r#"Five is the <node id="n5" label="5" kind="circle"/>root, three goes "#,
    r#"<node id="n3" label="3" parent="n5" side="left"/>left of it."#,
);

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

/// Every message of request `index`, as text.
fn request_text(sim: &Sim, index: usize) -> String {
    format!(
        "{:?}",
        sim.talker.requests.get(index).expect("a request").messages
    )
}

#[test]
fn a_drawing_asked_for_and_not_drawn_is_asked_for_once_more() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DESCRIBES));
    sim.talker.push(ScriptedReply::says(DRAWS));
    sim.user_says(0, "Draw a binary search tree as I insert 5, 3, 8, 1");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2, "one more request");
    let note = request_text(&sim, 1);
    assert!(note.contains("they asked for a drawing"), "{note}");
    assert!(
        note.contains(DESCRIBES),
        "what it said stays in the history: {note}"
    );
    assert_eq!(op_starts(&sim).len(), 2, "the second answer drew");
    let said = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, Effect::Synthesize { .. }))
        .count();
    assert!(said >= 3, "both answers were spoken");
}

/// The bound: a second answer that still draws nothing ends the turn; no loop.
#[test]
fn a_reply_that_still_draws_nothing_is_not_asked_again() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DESCRIBES));
    sim.talker
        .push(ScriptedReply::says("Each insert walks down from the root."));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Can you sketch a binary search tree?");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2);
    assert!(op_starts(&sim).is_empty());
    // The next turn starts with its own retry.
    sim.user_says(40_000, "Please draw it.");
    sim.run_until(80_000);
    assert_eq!(talks(&sim), 4, "a new turn may ask once more");
}

#[test]
fn no_retry_when_the_reply_drew_or_no_drawing_was_asked() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DRAWS));
    sim.user_says(0, "Draw a binary search tree as I insert 5, 3, 8, 1");
    sim.run_until(20_000);
    assert_eq!(talks(&sim), 1, "it drew");
    sim.talker.push(ScriptedReply::says(DESCRIBES));
    sim.user_says(20_000, "Explain how a request reaches the database");
    sim.run_until(40_000);
    assert_eq!(talks(&sim), 2, "nothing asked to be drawn");
    sim.talker
        .push(ScriptedReply::says("Don't worry, it's simple."));
    sim.user_says(40_000, "Explain Raft, but don't draw anything");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 3);
}

/// "Show me my notes" shows a view: that is showing, not a missed drawing.
#[test]
fn a_view_or_a_card_counts_as_showing() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Here they are.").calls("show_memory", "{}"));
    sim.talker.push(ScriptedReply::says("Three notes so far."));
    sim.user_says(0, "Show me my notes.");
    sim.run_until(20_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2, "the tool round only");
}

/// A detour answer that drew is never asked again, whatever the words were.
#[test]
fn a_detour_answer_that_drew_is_not_asked_again() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(common::DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>Reads try "#,
        r#"<node id="redis" label="Redis" kind="db"/>Redis first. <resume/>So, back."#,
    )));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| op_starts(sim).len() > 2));
    let at = sim.now().as_ms() + 50;
    sim.user_says_with_onset(at, "wait, show me the cache");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2);
}

/// gemma4's repo tour (2026-10-16): the call was spoken as words and nothing ran. Now it is
/// never said, the talker is told, and it is asked once to call the tool for real.
#[test]
fn a_repo_read_written_as_words_is_not_said_and_is_asked_for_as_a_call() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(r#"list_dir path ".""#));
    sim.talker
        .push(ScriptedReply::says("Let me look.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(
        r#"The code lives in <node id="crates" label="crates/" kind="group"/>crates."#,
    ));
    sim.tools.respond(
        "list_dir",
        ToolOutput::Listing {
            entries: vec![DirEntry {
                name: "crates".to_owned(),
                is_dir: true,
            }],
            more_entries: 0,
        },
    );
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    let spoken = sim.effects().any(
        |(_, effect)| matches!(effect, Effect::Synthesize { text, .. } if text.contains("list")),
    );
    assert!(!spoken, "the call is never read aloud");
    let captioned = sim.ui().any(
        |(_, event)| matches!(event, UiEvent::Caption { text, .. } if text.contains("list_dir")),
    );
    assert!(!captioned, "nor captioned");
    assert_eq!(talks(&sim), 3, "asked once, then the tool round");
    let asked = request_text(&sim, 1);
    assert!(asked.contains("wrote list_dir in its text"), "{asked}");
    assert!(
        asked.contains("They were not spoken and nothing ran"),
        "{asked}"
    );
    let ran = count(&sim, |effect| matches!(effect, Effect::RunTool { .. }));
    assert_eq!(ran, 1);
}
