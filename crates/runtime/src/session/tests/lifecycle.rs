//! The session's life (LLD §3.6): it opens with the store's context, ends after the idle
//! timeout, opens again on the next words, and ends when the app closes. A session nobody
//! spoke in leaves nothing; a muted one wakes only on an unmute.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::geometry::Rect;
use boardai_core::history::Message;
use boardai_core::ids::LayerId;
use boardai_core::protocol::{DrawAction, DrawnShape, MicAction, UiCommand, UiEvent};

use super::{ScriptedTalker, close, idle_after_speaking, run_until, start, start_with_store};
use crate::limits::{SESSION_IDLE_TIMEOUT_MS, SESSION_START_TIMEOUT_MS};
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
    assert!(link.voice_end(400, 0).await.is_ok());
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

/// You draw (artboard 13): finishing a drawing asks BoardAI something, so the session is kept
/// with its digest though nothing was said; resting, the pen opens a new session.
#[tokio::test(start_paused = true)]
async fn a_finished_drawing_keeps_the_session_and_the_pen_wakes_the_next() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("That's a client.");
    let harness = start(Arc::clone(&talker), false);
    let draw = |action: DrawAction| UiCommand::Draw { action };
    let rect = Rect {
        x: 0,
        y: 0,
        w: 120,
        h: 60,
    };
    let shape = || DrawAction::Shape {
        layer: LayerId::MAIN,
        drawn: DrawnShape::Box { rect },
    };
    assert!(harness.session.command(draw(DrawAction::Start)).is_ok());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(harness.session.command(draw(shape())).is_ok());
    assert!(harness.session.command(draw(DrawAction::Done)).is_ok());
    assert!(run_until(20_000, || talker.requests().len() == 1).await);
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    past_the_idle_timeout().await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    // Undo while resting opens nothing; a stroke does.
    assert!(harness.session.command(draw(DrawAction::Undo)).is_ok());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    assert!(harness.session.command(draw(shape())).is_ok());
    assert!(run_until(2_000, || recorder.lifecycle().len() == 3).await);
    let drawn = recorder
        .events()
        .iter()
        .filter(|event| matches!(event, UiEvent::UserShape { .. }))
        .count();
    assert_eq!(drawn, 2, "the stroke that woke the session landed in it");
    close(harness).await;
}

/// How long closing may take once the store's session start hangs: nothing else is waited on,
/// so it is far inside `SESSION_CLOSE_TIMEOUT_MS`.
const CLOSE_WHILE_OPENING_MAX_MS: u64 = 1_000;

/// Review finding 12: a store whose session start never answers (a hung network or external
/// volume, a stalled fsync) made quitting hang forever. The orchestrator awaited the start with
/// no bound and saw the close only at its loop, and the app blocks on `close` at exit.
#[tokio::test(start_paused = true)]
async fn closing_is_bounded_while_the_session_is_opening() {
    let talker = Arc::new(ScriptedTalker::default());
    let harness = start_with_store(talker, false, Some(0));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let bound = Duration::from_millis(CLOSE_WHILE_OPENING_MAX_MS);
    let closed = tokio::time::timeout(bound, close(harness)).await;
    let Ok((stats, recorder, _)) = closed else {
        panic!("close() did not return: quitting hangs");
    };
    assert_eq!(stats.sessions, 0, "no session opened");
    assert_eq!(
        recorder.lifecycle(),
        vec!["start"],
        "a session the store never opened is never ended there"
    );
}

/// The same while waking after an idle rest: the first words open a new session, and its start
/// hangs.
#[tokio::test(start_paused = true)]
async fn closing_is_bounded_while_a_new_session_opens() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    let harness = start_with_store(Arc::clone(&talker), false, Some(1));
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    past_the_idle_timeout().await;
    assert!(say_typed(&link, "where were we").await.is_ok());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let bound = Duration::from_millis(CLOSE_WHILE_OPENING_MAX_MS);
    let closed = tokio::time::timeout(bound, close(harness)).await;
    let Ok((stats, recorder, _)) = closed else {
        panic!("close() did not return: quitting hangs");
    };
    assert_eq!(stats.sessions, 1);
    assert_eq!(recorder.lifecycle(), vec!["start", "digest", "start"]);
}

/// A start the store doesn't answer in time still opens the session, without its context and
/// unrecorded: the user can talk, and the session is never ended in the store.
#[tokio::test(start_paused = true)]
async fn a_session_whose_start_stalls_still_opens() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    let harness = start_with_store(Arc::clone(&talker), false, Some(0));
    tokio::time::sleep(Duration::from_millis(SESSION_START_TIMEOUT_MS + 100)).await;
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    let first = talker
        .requests()
        .first()
        .map(|conversation| conversation.messages().to_vec())
        .unwrap_or_default();
    let context = first.iter().any(|message| {
        matches!(message, Message::System { text } if text.contains("Where you left off"))
    });
    assert!(!context, "no context from a store that didn't answer");
    let (stats, recorder, _) = close(harness).await;
    assert_eq!(stats.sessions, 1);
    assert_eq!(recorder.lifecycle(), vec!["start"]);
}

fn position(events: &[UiEvent], matches: impl Fn(&UiEvent) -> bool) -> Option<usize> {
    events.iter().position(matches)
}

/// D-144, D-145: "we can stop here" ends the session once "Session saved." has played; the
/// stage clears, the library rests on it once the digest is written, and the next words open a
/// new session. `SessionEnd` never reaches the session's log.
#[tokio::test(start_paused = true)]
async fn ending_the_session_by_voice_rests_on_the_library() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    talker.says("Welcome back.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    assert!(say_typed(&link, "I think we can stop here.").await.is_ok());
    assert!(run_until(20_000, || recorder.lifecycle() == vec!["start", "digest"]).await);
    assert!(run_until(2_000, || recorder.events().iter().any(is_library)).await);
    let events = recorder.events();
    let saved = position(
        &events,
        |event| matches!(event, UiEvent::Caption { text, is_final: true, .. } if text == "Session saved."),
    );
    let ended = position(&events, |event| matches!(event, UiEvent::SessionEnd));
    let library = position(&events, is_library);
    assert!(
        saved < ended && ended < library && saved.is_some(),
        "{events:?}"
    );
    assert_eq!(
        talker.requests().len(),
        1,
        "the core answered the end itself"
    );
    assert!(
        !recorder
            .logged()
            .iter()
            .any(|event| matches!(event, UiEvent::SessionEnd | UiEvent::ViewShow { .. })),
        "the resting stage is never in the ended session's log"
    );

    assert!(say_typed(&link, "where were we").await.is_ok());
    assert!(run_until(20_000, || talker.requests().len() == 2).await);
    assert_eq!(recorder.lifecycle(), vec!["start", "digest", "start"]);
    assert_eq!(session_starts(&recorder.events()), 2);
    let (stats, _, _) = close(harness).await;
    assert_eq!(stats.sessions, 2);
}

fn is_library(event: &UiEvent) -> bool {
    matches!(
        event,
        UiEvent::ViewShow {
            view: boardai_core::views::View::Library { .. },
            ..
        }
    )
}

/// Esc on the resting library closes it without opening a session.
#[tokio::test(start_paused = true)]
async fn esc_closes_the_resting_library() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    assert!(say_typed(&link, "end the session").await.is_ok());
    assert!(run_until(20_000, || recorder.events().iter().any(is_library)).await);
    assert!(harness.session.command(UiCommand::CloseView).is_ok());
    assert!(
        run_until(2_000, || recorder
            .events()
            .iter()
            .any(|event| matches!(event, UiEvent::ViewDismiss)))
        .await
    );
    let (stats, recorder, _) = close(harness).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"]);
    assert_eq!(stats.sessions, 1, "Esc opens no session");
}

/// The idle rest clears the stage the same way.
#[tokio::test(start_paused = true)]
async fn an_idle_rest_clears_the_stage() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    past_the_idle_timeout().await;
    assert!(
        recorder
            .events()
            .iter()
            .any(|event| matches!(event, UiEvent::SessionEnd))
    );
    let (_, recorder, _) = close(harness).await;
    assert!(
        !recorder
            .logged()
            .iter()
            .any(|event| matches!(event, UiEvent::SessionEnd))
    );
}

/// Review 2, finding 9: the resting library was read on the orchestrator's loop, so a store
/// slow to answer held the next session's words for up to `SESSION_TOOL_TIMEOUT_MS`.
#[tokio::test(start_paused = true)]
async fn a_stalled_library_read_never_holds_the_next_session() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("Sure.");
    talker.says("Welcome back.");
    let harness = start(Arc::clone(&talker), false);
    if let Ok(mut stalls) = harness.recorder.library_stalls.lock() {
        *stalls = true;
    }
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "explain Raft").await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(20_000, || idle_after_speaking(&recorder)).await);
    assert!(say_typed(&link, "I think we can stop here.").await.is_ok());
    assert!(run_until(20_000, || recorder.lifecycle() == vec!["start", "digest"]).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(say_typed(&link, "where were we").await.is_ok());
    assert!(
        run_until(1_000, || recorder.lifecycle()
            == vec!["start", "digest", "start"])
        .await,
        "the next session waited on the library read: {:?}",
        recorder.lifecycle()
    );
    assert!(run_until(20_000, || talker.requests().len() == 2).await);
    assert!(
        !recorder.events().iter().any(is_library),
        "a library that never answered is never shown"
    );
    let (stats, _, _) = close(harness).await;
    assert_eq!(stats.sessions, 2);
}
