//! Simulator scenarios for controlling BoardAI without the talker's help (PRODUCT §6.2, §8.9):
//! the Esc stop, a spoken cancel of a running harness task, and the idle suggestion.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::CoreConfig;
use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::history::Message;
use boardai_core::ids::LayerId;
use boardai_core::input::Input;
use boardai_core::limits::SUGGESTION_MAX_BYTES;
use boardai_core::protocol::{OrbState, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::check::strip_tags;
use boardai_core::sim::talker::ScriptedReply;
use common::{DEMO, assert_clean, config, count, op_starts, orbs, transcript};

fn stop(sim: &mut Sim, at_ms: u64) {
    sim.at(at_ms, Input::Command(UiCommand::Stop));
}

/// Esc while BoardAI speaks: the voice stops, the stroke in flight freezes, the words heard so
/// far are committed, and the floor goes idle with no new request.
#[test]
fn stop_while_speaking_freezes_commits_what_was_heard_and_goes_idle() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let at = sim.now().as_ms() + 100;
    stop(&mut sim, at);
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert_eq!(*sim.core().state(), ConversationState::Idle);
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 1);
    assert!(count(&sim, |effect| *effect == Effect::CancelTalk) >= 1);
    assert_eq!(sim.talker.requests.len(), 1, "a stop asks nothing new");
    let froze = sim.ui().any(|(_, event)| {
        *event
            == UiEvent::Freeze {
                layer: LayerId::MAIN,
            }
    });
    assert!(froze);
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Idle));
    let history = transcript(&sim);
    let heard = history
        .iter()
        .find_map(|line| line.strip_prefix("assistant(heard): "))
        .expect("what was heard is committed");
    let spoken = strip_tags(heard);
    assert!(!spoken.is_empty() && strip_tags(DEMO).starts_with(spoken.trim_end()));
}

/// Esc while BoardAI is still thinking: the request is cancelled and the question, of which
/// nothing was heard, is dropped (as with mute), so the next question stands alone.
#[test]
fn stop_while_thinking_cancels_and_drops_the_unheard_question() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 3_000;
    sim.talker.push(ScriptedReply::says("This never plays."));
    sim.talker
        .push(ScriptedReply::says("Raft elects a leader."));
    sim.user_says(0, "Explain Paxos.");
    let thinking = sim.run_until_true(10_000, |sim| {
        *sim.core().state() == ConversationState::Thinking
    });
    assert!(thinking);
    let at = sim.now().as_ms() + 50;
    stop(&mut sim, at);
    sim.user_says(at + 1_000, "Explain Raft instead.");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    let users: Vec<String> = transcript(&sim)
        .into_iter()
        .filter(|line| line.starts_with("user: "))
        .collect();
    assert_eq!(users.len(), 1, "{users:#?}");
    assert!(
        users[0].starts_with("user: Explain Raft instead."),
        "{users:#?}"
    );
}

/// Esc with nothing to stop changes nothing.
#[test]
fn stop_when_idle_or_listening_changes_nothing() {
    let mut sim = Sim::new(config());
    stop(&mut sim, 100);
    sim.at(500, Input::VoiceStart);
    stop(&mut sim, 600);
    sim.run_until(700);
    assert!(matches!(
        sim.core().state(),
        ConversationState::Listening(_)
    ));
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 0);
    assert_eq!(count(&sim, |effect| *effect == Effect::CancelTalk), 0);
}

/// "Stop that research" while a research task runs cancels it at once; the talker hears the
/// words with a note that it is done, and the task's late result is never reported.
#[test]
fn stop_that_research_cancels_the_running_task() {
    let mut sim = Sim::new(config());
    sim.harness.run_ms = 10_000;
    sim.talker
        .push(ScriptedReply::says("I'll dig into that.").calls(
            "research",
            r#"{"question":"Where is the TTL set?","depth":"quick"}"#,
        ));
    sim.talker.push(ScriptedReply::says(
        "It's running; I'll tell you when it's back.",
    ));
    sim.talker.push(ScriptedReply::says("Stopped."));
    sim.user_says(0, "where do we set the TTL?");
    sim.run_until(6_000);
    sim.user_says(6_000, "stop that research");
    sim.run_until(30_000);
    assert_clean(&sim);
    let cancelled: Vec<u32> = sim
        .effects()
        .filter_map(|(_, effect)| match effect {
            Effect::CancelHarness { task } => Some(task.0),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, vec![0]);
    assert_eq!(
        sim.talker.requests.len(),
        3,
        "no report turn for a cancelled task"
    );
    let last = &sim.talker.requests[2].messages;
    let noted = last.iter().any(|message| {
        matches!(message, Message::System { text } if text.contains("Research task 0 was cancelled"))
    });
    assert!(noted, "{last:#?}");
    let reported = sim.talker.requests.iter().any(|request| {
        request.messages.iter().any(|message| {
            matches!(message, Message::System { text } if text.contains("task 0 finished"))
        })
    });
    assert!(!reported);
}

/// "Cancel that" with nothing running is just words for the talker.
#[test]
fn cancel_that_with_no_task_running_cancels_nothing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.user_says(0, "cancel that");
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(
        count(&sim, |effect| matches!(
            effect,
            Effect::CancelHarness { .. }
        )),
        0
    );
    assert_eq!(sim.talker.requests.len(), 1);
}

/// Frame 1: the suggestion shows at start and clears the first time the user speaks; a long one
/// is cut, and a session without one shows nothing.
#[test]
fn the_idle_suggestion_shows_until_the_user_speaks() {
    let suggestion = "Pick up Raft log compaction where we left off?";
    let mut sim = Sim::new(CoreConfig {
        suggestion: Some(suggestion.to_owned()),
        ..config()
    });
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(2_000, "Yes, let's do that.");
    sim.user_says(9_000, "And snapshots?");
    sim.run_until(20_000);
    assert_clean(&sim);
    let suggestions: Vec<(u64, Option<String>)> = sim
        .ui()
        .filter_map(|(at, event)| match event {
            UiEvent::Suggestion { text } => Some((at.as_ms(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        suggestions,
        vec![(0, Some(suggestion.to_owned())), (2_000, None)]
    );

    let long = Sim::new(CoreConfig {
        suggestion: Some("word ".repeat(100)),
        ..config()
    });
    let shown = long.ui().find_map(|(_, event)| match event {
        UiEvent::Suggestion { text } => text.clone(),
        _ => None,
    });
    assert!(shown.is_some_and(|text| text.len() <= SUGGESTION_MAX_BYTES && text.ends_with('…')));

    let plain = Sim::new(config());
    assert!(
        !plain
            .ui()
            .any(|(_, event)| matches!(event, UiEvent::Suggestion { .. }))
    );
}
