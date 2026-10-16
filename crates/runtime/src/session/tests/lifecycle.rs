//! The session's life (LLD §3.6): it opens with the store's context, ends after the idle
//! timeout, opens again on the next words, and ends when the app closes. A session nobody
//! spoke in leaves nothing; a muted one wakes only on an unmute.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::Message;
use boardai_core::protocol::{MicAction, UiCommand, UiEvent};

use super::{ScriptedTalker, close, idle_after_speaking, run_until, start};
use crate::limits::SESSION_IDLE_TIMEOUT_MS;
use crate::session::say_typed;

fn session_starts(events: &[UiEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, UiEvent::SessionStart { .. }))
        .count()
}

async fn past_the_idle_timeout() {
    tokio::time::sleep(Duration::from_millis(SESSION_IDLE_TIMEOUT_MS + 1_000)).await;
}

#[tokio::test(start_paused = true)]
async fn a_session_opens_first_and_the_talker_starts_with_its_context() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Picking up at log matching.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "pick it up from there").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    let events = recorder.events();
    assert!(
        matches!(events.first(), Some(UiEvent::SessionStart { session: Some(id) }) if id == "2026-10-15-143210"),
        "{events:?}"
    );
    let requests = talker.requests();
    let first = requests
        .first()
        .map(|conversation| conversation.messages().to_vec());
    let context = first.unwrap_or_default().iter().any(|message| {
        matches!(message, Message::System { text } if text.contains("Where you left off: Raft."))
    });
    assert!(context, "the store's context reaches the first request");
    let suggested = events.iter().any(|event| {
        matches!(event, UiEvent::Suggestion { text: Some(text) } if text == "Pick up Raft where we left off?")
    });
    assert!(
        suggested,
        "the last session's suggestion shows under the orb"
    );
    let (stats, recorder, _) = close(harness).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    assert_eq!(stats.sessions, 1);
}

#[tokio::test(start_paused = true)]
async fn an_idle_session_ends_and_the_next_words_open_a_new_one() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    talker.says("Welcome back.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    past_the_idle_timeout().await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    assert!(say_typed(&link, "where were we").await.is_ok());
    assert!(run_until(20_000, || talker.requests().len() == 2).await);
    assert_eq!(recorder.lifecycle(), vec!["start", "digest", "start"]);
    assert_eq!(session_starts(&recorder.events()), 2);
    let second = talker
        .requests()
        .last()
        .map(|conversation| conversation.messages().len());
    assert!(
        second.is_some_and(|messages| messages <= 3),
        "a new session starts a new conversation: {second:?}"
    );
    let (stats, recorder, _) = close(harness).await;
    assert_eq!(stats.sessions, 2);
    assert_eq!(
        recorder.lifecycle(),
        vec!["start", "digest", "start", "digest"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_session_nobody_spoke_in_leaves_nothing() {
    let talker = Arc::new(ScriptedTalker::default());
    let harness = start(Arc::clone(&talker), false);
    past_the_idle_timeout().await;
    let (stats, recorder, _) = close(harness).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "discard"]);
    assert_eq!(stats.sessions, 1);
}

#[tokio::test(start_paused = true)]
async fn a_muted_session_rests_and_wakes_only_on_unmute() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    let mute = UiCommand::Mic {
        action: MicAction::Mute,
    };
    assert!(harness.session.command(mute).is_ok());
    past_the_idle_timeout().await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    // Noise while muted and resting opens nothing.
    assert!(link.voice_start().await.is_ok());
    assert!(link.voice_end(400).await.is_ok());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    let unmute = UiCommand::Mic {
        action: MicAction::Unmute,
    };
    assert!(harness.session.command(unmute).is_ok());
    assert!(run_until(2_000, || recorder.lifecycle().len() == 3).await);
    let (stats, recorder, _) = close(harness).await;
    assert!(stats.resting_inputs >= 2, "{stats:?}");
    assert_eq!(
        recorder.lifecycle(),
        vec!["start", "digest", "start", "discard"]
    );
}
