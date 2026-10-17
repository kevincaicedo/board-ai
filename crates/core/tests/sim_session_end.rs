//! Simulator scenarios: ending the session by voice (D-144) where the review (round 2) found it
//! failing: joined to another control, said while BoardAI thinks, said while a change runs, and
//! a thanks over the tail of "Session saved.".
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Effect, ForgetScope};
use boardai_core::engine::text::{SESSION_ENDED_LINE, SESSION_SAVED_LINE};
use boardai_core::harness::HarnessKind;
use boardai_core::input::Input;
use boardai_core::protocol::UiEvent;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Entry, Sim};
use common::{DEMO, assert_clean, config, count, transcript};

fn ends(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::SessionEnd))
    })
}

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

fn said(sim: &Sim, line: &str) -> bool {
    sim.effects()
        .any(|(_, effect)| matches!(effect, Effect::Synthesize { text, .. } if text == line))
}

/// "Don't remember this session, we can stop here." joins two controls in one breath: the
/// session goes off the record first, then ends, and the talker hears none of it.
#[test]
fn forget_the_session_and_stop_in_one_breath_does_both() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says("This is never asked."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    sim.user_says(
        30_000,
        "Okay, don't remember this session, we can stop here.",
    );
    sim.run_until(45_000);
    assert_clean(&sim);
    let forgot = count(&sim, |effect| {
        matches!(
            effect,
            Effect::Forget {
                scope: ForgetScope::Session
            }
        )
    });
    assert_eq!(forgot, 1, "the user asked not to remember this session");
    assert!(
        said(&sim, SESSION_ENDED_LINE),
        "the off-record line ends it"
    );
    assert_eq!(ends(&sim), 1);
    assert_eq!(talks(&sim), 1, "the controls went to the talker");
}

/// A question with a control at its end stays a question: nothing is forgotten, nothing ends.
#[test]
fn a_question_ending_in_a_control_stays_a_question() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Paxos agrees on one value."));
    sim.user_says(0, "Explain Paxos briefly, then we can stop here.");
    sim.run_until(15_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 0);
    assert_eq!(talks(&sim), 1);
}

/// "End the session." said while BoardAI still thinks (nothing of the answer heard) is the cue,
/// matched against the new words alone: the question is dropped and the session ends.
#[test]
fn end_the_session_while_thinking_ends_it() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 3_000;
    sim.talker.push(ScriptedReply::says("This never plays."));
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, "How would you cache this?");
    assert!(sim.run_until_true(10_000, |sim| {
        *sim.core().state() == ConversationState::Thinking
    }));
    let at = sim.now().as_ms() + 50;
    sim.user_says(at, "End the session.");
    sim.run_until(at + 15_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 1, "{:#?}", transcript(&sim));
    assert_eq!(talks(&sim), 1, "the cue joined the question");
}

/// Runs a change the user said yes to; returns when BoardAI is quiet with the change running.
fn a_running_change(sim: &mut Sim) -> u64 {
    sim.harness.run_ms = 120_000;
    sim.talker
        .push(ScriptedReply::says("I can add jitter.").calls(
            "propose_change",
            r#"{"instruction":"Add jitter to the retry policy"}"#,
        ));
    sim.talker
        .push(ScriptedReply::says("It goes on its own branch."));
    sim.talker.push(ScriptedReply::says("Started."));
    sim.user_says(0, "can you add jitter to the retries");
    assert!(sim.run_until_true(60_000, |sim| {
        sim.ui().any(|(_, event)| {
            matches!(event, UiEvent::Caption { text, is_final: true, .. } if text.ends_with("Should I go ahead?"))
        }) && *sim.core().state() == ConversationState::Idle
    }));
    let asked = sim.now().as_ms();
    sim.user_says(asked + 500, "yes, go ahead");
    assert!(sim.run_until_true(asked + 20_000, |sim| {
        sim.effects().any(|(_, effect)| {
            matches!(effect, Effect::RunHarness { spec, .. } if matches!(spec.kind, HarnessKind::Change { .. }))
        })
    }));
    assert!(sim.run_until_true(asked + 30_000, |sim| {
        *sim.core().state() == ConversationState::Idle && !sim.speech.is_active()
    }));
    sim.now().as_ms() + 1_000
}

fn reported(sim: &Sim) -> bool {
    sim.trace()
        .iter()
        .any(|(_, entry)| matches!(entry, Entry::Input(Input::Harness { .. })))
}

fn warned(sim: &Sim) -> usize {
    count(
        sim,
        |effect| matches!(effect, Effect::Synthesize { text, .. } if text.contains("change is still running")),
    )
}

/// "We can stop here" while a change runs: BoardAI says the change would stop partway and asks
/// for the words again; said again, the change is cancelled and the session ends, and the line
/// says so for the digest.
#[test]
fn ending_the_session_while_a_change_runs_asks_first() {
    let mut sim = Sim::new(config());
    let at = a_running_change(&mut sim);
    sim.user_says(at, "Okay, we can stop here.");
    sim.run_until(at + 10_000);
    assert!(!reported(&sim), "the change is still running");
    assert_eq!(warned(&sim), 1, "BoardAI said nothing about the change");
    assert_eq!(ends(&sim), 0, "the session ended while the change ran");
    let cancels = |sim: &Sim| count(sim, |effect| matches!(effect, Effect::CancelHarness { .. }));
    assert_eq!(cancels(&sim), 0);
    sim.user_says(at + 10_000, "End the session.");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert_eq!(
        cancels(&sim),
        1,
        "the change is stopped, not left to the rest"
    );
    assert_eq!(ends(&sim), 1);
    let stopped = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::Caption { text, is_final: true, .. }
            if text.contains("stopped") && text.contains(SESSION_SAVED_LINE))
    });
    assert!(stopped, "the line doesn't record the stop");
}

/// Anything else after the warning keeps the session: the next "end the session" warns again.
#[test]
fn anything_else_after_the_warning_keeps_the_session() {
    let mut sim = Sim::new(config());
    let at = a_running_change(&mut sim);
    sim.talker
        .push(ScriptedReply::says("It retries three times."));
    sim.user_says(at, "End the session.");
    sim.run_until(at + 10_000);
    sim.user_says(at + 10_000, "How many retries are there?");
    sim.run_until(at + 25_000);
    sim.user_says(at + 25_000, "End the session.");
    sim.run_until(at + 35_000);
    assert_clean(&sim);
    assert_eq!(warned(&sim), 2);
    assert_eq!(ends(&sim), 0);
}

/// "Session saved." plays to its end while "thanks, bye" overlaps its last word: the line
/// played, so the session ends, and a goodbye is not a question for the talker.
#[test]
fn a_thanks_over_the_tail_of_session_saved_ends_the_session() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("You're welcome!"));
    sim.user_says(0, "End the session.");
    assert!(sim.run_until_true(10_000, |sim| {
        sim.speech.is_active() && !sim.speech.word_starts().is_empty()
    }));
    let started = sim.now().as_ms();
    // "Session saved." is 14 bytes at 60 ms a byte: 840 ms. The user starts 600 ms in.
    sim.user_says_with_onset(started + 600, "Thanks, bye.");
    sim.run_until(started + 20_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 0, "'thanks, bye' went to the talker");
    assert_eq!(
        ends(&sim),
        1,
        "the line played to its end, yet the session did not end"
    );
}

/// The same overlap with a real question keeps the session, and the talker answers it.
#[test]
fn a_question_over_the_tail_of_session_saved_keeps_the_session() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Raft elects a leader."));
    sim.user_says(0, "End the session.");
    assert!(sim.run_until_true(10_000, |sim| {
        sim.speech.is_active() && !sim.speech.word_starts().is_empty()
    }));
    let started = sim.now().as_ms();
    sim.user_says_with_onset(
        started + 600,
        "Actually, one more: how does Raft elect a leader?",
    );
    sim.run_until(started + 30_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 0);
    assert_eq!(talks(&sim), 1);
}
