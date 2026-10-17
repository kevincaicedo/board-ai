//! The talker's warm-up at session open (D-154): it starts when a session opens, a real
//! request drops it at once, closing, resting or a spoken session end drops it, and nothing it
//! does reaches the core or the conversation.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use boardai_core::history::{Message, UserPart};
use boardai_core::protocol::UiEvent;

use super::{ScriptedTalker, close, idle_after_speaking, run_until, start};
use crate::limits::SESSION_IDLE_TIMEOUT_MS;
use crate::session::say_typed;

fn warming_talker() -> Arc<ScriptedTalker> {
    Arc::new(ScriptedTalker {
        warms_up: true,
        ..ScriptedTalker::default()
    })
}

#[tokio::test(start_paused = true)]
async fn a_question_drops_the_warm_up_and_is_answered_from_a_clean_history() {
    let talker = warming_talker();
    talker.says("Sure.");
    let harness = start(Arc::clone(&talker), false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(talker.warm_ups.load(Ordering::SeqCst), 1);
    assert_eq!(talker.warm_ups_dropped.load(Ordering::SeqCst), 0);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    assert_eq!(
        talker.warm_ups_dropped.load(Ordering::SeqCst),
        1,
        "the real request never waits behind the warm-up"
    );
    let requests = talker.requests();
    assert_eq!(
        requests.len(),
        1,
        "the warm-up is not a conversation request"
    );
    let users: Vec<&Message> = requests[0]
        .messages()
        .iter()
        .filter(|message| matches!(message, Message::User { .. }))
        .collect();
    assert!(
        matches!(users.as_slice(), [Message::User { parts }] if matches!(parts.as_slice(), [UserPart::Text { text }] if text == "explain Raft")),
        "{users:?}"
    );
    let (_, recorder, _) = close(harness).await;
    assert!(
        recorder
            .inputs()
            .iter()
            .all(|input| !format!("{input:?}").contains("Hi.")),
        "nothing from the warm-up reaches the core"
    );
}

#[tokio::test(start_paused = true)]
async fn closing_drops_a_warm_up_still_running() {
    let talker = warming_talker();
    let harness = start(Arc::clone(&talker), false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = close(harness).await;
    assert_eq!(talker.warm_ups.load(Ordering::SeqCst), 1);
    assert_eq!(talker.warm_ups_dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_rest_drops_the_warm_up_and_waking_warms_up_again() {
    let talker = warming_talker();
    talker.says("Welcome back.");
    let harness = start(Arc::clone(&talker), false);
    tokio::time::sleep(Duration::from_millis(SESSION_IDLE_TIMEOUT_MS + 1_000)).await;
    assert_eq!(talker.warm_ups_dropped.load(Ordering::SeqCst), 1);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "where were we").await.is_ok());
    assert!(run_until(20_000, || talker.requests().len() == 1).await);
    assert_eq!(
        talker.warm_ups.load(Ordering::SeqCst),
        2,
        "the new session warmed up"
    );
    let _ = close(harness).await;
}

/// "End the session" is answered without the talker, so no request drops the warm-up; the
/// session's end does (D-144's rest).
#[tokio::test(start_paused = true)]
async fn a_spoken_session_end_drops_the_warm_up() {
    let talker = warming_talker();
    let harness = start(Arc::clone(&talker), false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "end the session").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(
        run_until(20_000, || recorder
            .events()
            .iter()
            .any(|event| matches!(event, UiEvent::SessionEnd)))
        .await
    );
    assert!(talker.requests().is_empty(), "the end needs no talker");
    assert_eq!(talker.warm_ups.load(Ordering::SeqCst), 1);
    assert_eq!(talker.warm_ups_dropped.load(Ordering::SeqCst), 1);
    let _ = close(harness).await;
}
