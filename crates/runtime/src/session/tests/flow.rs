//! Whole turns through the orchestrator.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use boardai_core::effect::Milestone;
use boardai_core::history::{Message, UserPart};
use boardai_core::ids::ToolCallId;
use boardai_core::input::{Input, StopReason};
use boardai_core::protocol::{OrbState, UiEvent};
use boardai_core::tools::{ToolCall, ToolInvocation};

use super::{DEMO, ScriptedTalker, close, idle_after_speaking, run_until, start};
use crate::limits::SESSION_INBOX_CAPACITY;
use crate::llm::ChatEvent;
use crate::session::{InboxRefused, SessionChannels, say_typed};

fn tool_call(name: &str, input_json: &str) -> ChatEvent {
    let Some(id) = ToolCallId::parse("call_1") else {
        panic!("valid id");
    };
    ChatEvent::ToolCall(ToolInvocation {
        id,
        name: name.to_owned(),
        input_json: input_json.to_owned(),
        call: ToolCall::parse(name, input_json),
    })
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User { parts } => parts.first().map(|part| match part {
                UserPart::Text { text } => text.clone(),
                UserPart::ToolResult { content, .. } => format!("<result>{content}"),
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .collect()
}

fn count(events: &[UiEvent], matches: impl Fn(&UiEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

/// The steel thread in miniature: a question becomes a spoken answer with strokes in sync,
/// and the session goes idle. The 600 ms end of turn is a `WakeAt` turned into a `Tick`.
#[tokio::test(start_paused = true)]
async fn a_typed_question_is_answered_with_voice_and_strokes_then_idle() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(
        say_typed(&link, "How does a request reach our API?")
            .await
            .is_ok()
    );
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    let (stats, recorder, talker) = close(harness).await;
    let events = recorder.events();
    let ops = count(&events, |event| matches!(event, UiEvent::OpStart { .. }));
    assert_eq!(ops, 3, "the browser, the CDN and the edge between them");
    assert!(events.iter().any(|event| matches!(
        event,
        UiEvent::Caption { text, is_final: true, .. } if text.ends_with("CDN at the edge.")
    )));
    let marks = recorder.marks();
    for mark in [
        Milestone::VoiceStart,
        Milestone::FinalTranscript,
        Milestone::TalkerRequested,
        Milestone::TalkerFirstToken,
        Milestone::FirstAudio,
        Milestone::FirstOp,
    ] {
        assert!(marks.contains(&mark), "{mark:?} in {marks:?}");
    }
    assert_eq!(talker.requests().len(), 1);
    assert_eq!(stats.stale_talker_inputs, 0);
    assert!(recorder.flushed.lock().is_ok_and(|flushed| *flushed));
}

/// D-034: once a stream is cancelled, nothing it sent reaches the core, even what was already
/// in the inbox.
#[tokio::test(start_paused = true)]
async fn inputs_from_a_cancelled_stream_never_reach_the_core() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.pending();
    talker.says("Both sit in front of the API.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "Explain the CDN.").await.is_ok());
    assert!(run_until(5_000, || talker.requests().len() == 1).await);
    // More words while BoardAI is still thinking: the request is cancelled and they join it.
    assert!(say_typed(&link, "and the load balancer too").await.is_ok());
    assert!(run_until(5_000, || talker.requests().len() == 2).await);
    assert_eq!(
        talker.dropped.load(Ordering::SeqCst),
        1,
        "the first stream was dropped"
    );
    let stale = Input::TalkerDelta {
        text: "STALE <node id=\"x\" label=\"Stale\"/>".to_owned(),
    };
    assert!(harness.session.inbox.send_talker(0, stale).await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    let (stats, recorder, talker) = close(harness).await;
    assert_eq!(stats.stale_talker_inputs, 1);
    let reached = recorder
        .inputs()
        .iter()
        .any(|input| matches!(input, Input::TalkerDelta { text } if text.contains("STALE")));
    assert!(!reached, "a stale delta reached the core");
    let second = talker.requests()[1].messages().to_vec();
    assert_eq!(
        user_texts(&second),
        vec!["Explain the CDN. and the load balancer too".to_owned()]
    );
}

/// A voice that refuses every clause: the orchestrator answers each with `SynthesisFailed`,
/// the user reads them as problems, nothing is drawn, and the turn still ends.
#[tokio::test(start_paused = true)]
async fn clauses_the_voice_refuses_are_answered_and_the_turn_ends() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(r#"Sure. It starts in the <node id="client" label="Browser"/>browser."#);
    let harness = start(Arc::clone(&talker), true);
    assert!(
        say_typed(&harness.session.speech_in(), "Where does it start?")
            .await
            .is_ok()
    );
    let recorder = Arc::clone(&harness.recorder);
    let finished = || {
        let events = recorder.events();
        count(&events, |event| matches!(event, UiEvent::Problem { .. })) == 2
            && recorder.orb() == Some(OrbState::Idle)
    };
    assert!(run_until(30_000, finished).await);
    let (_, recorder, _) = close(harness).await;
    let events = recorder.events();
    assert_eq!(
        count(&events, |event| matches!(event, UiEvent::OpStart { .. })),
        0
    );
    let refused = recorder
        .inputs()
        .iter()
        .filter(|input| matches!(input, Input::SynthesisFailed { .. }))
        .count();
    assert_eq!(refused, 2);
}

/// A barge-in mid-answer: the voice ducks, stops and answers `StopVoice` with where it was,
/// and the interruption becomes the next request.
#[tokio::test(start_paused = true)]
async fn a_barge_in_stops_the_voice_and_the_interruption_is_answered() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    talker.says("Because the cache answers first.");
    let harness = start(Arc::clone(&talker), false);
    let link = harness.session.speech_in();
    assert!(
        say_typed(&link, "How does a request reach our API?")
            .await
            .is_ok()
    );
    let recorder = Arc::clone(&harness.recorder);
    let drawing = || {
        let events = recorder.events();
        count(&events, |event| matches!(event, UiEvent::OpStart { .. })) >= 1
    };
    assert!(run_until(30_000, drawing).await);
    assert!(say_typed(&link, "wait, why Redis first?").await.is_ok());
    assert!(run_until(30_000, || talker.requests().len() == 2).await);
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    let (_, recorder, talker) = close(harness).await;
    let stopped = recorder
        .inputs()
        .iter()
        .any(|input| matches!(input, Input::VoiceStopped { last: Some(_) }));
    assert!(stopped, "the voice answered StopVoice with a position");
    let marks = recorder.marks();
    assert!(marks.contains(&Milestone::BargeInDuck) && marks.contains(&Milestone::BargeInStop));
    let events = recorder.events();
    assert_eq!(
        count(&events, |event| matches!(event, UiEvent::Freeze { .. })),
        1
    );
    // The answer was fully streamed before the cut, so the user's words follow a note saying
    // where they stopped hearing it (LLD §2.7 case b).
    let users = user_texts(talker.requests()[1].messages());
    let last = users.last().cloned().unwrap_or_default();
    assert!(last.starts_with("[You were interrupted."), "{users:#?}");
    assert!(last.contains("\nwait, why Redis first?\n"), "{users:#?}");
}

/// Tools: `recall` goes to the store, and a harness task with no harness configured fails out
/// loud; both answers come back to the talker.
#[tokio::test(start_paused = true)]
async fn tool_results_and_harness_failures_reach_the_talker() {
    let talker = Arc::new(ScriptedTalker::default());
    let recall = tool_call("recall", r#"{"query":"kafka topics"}"#);
    talker.push(vec![(30, recall)], StopReason::ToolUse, "");
    let research = tool_call(
        "research",
        r#"{"question":"where is the TTL","depth":"quick"}"#,
    );
    talker.push(vec![(30, research)], StopReason::ToolUse, "");
    talker.says("I'm looking into it.");
    let harness = start(Arc::clone(&talker), false);
    assert!(
        say_typed(&harness.session.speech_in(), "What do we know about Kafka?")
            .await
            .is_ok()
    );
    assert!(run_until(60_000, || talker.requests().len() == 3).await);
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    let (_, _, talker) = close(harness).await;
    let requests = talker.requests();
    let second = user_texts(requests[1].messages());
    assert!(
        second
            .iter()
            .any(|text| text.starts_with("<result>Nothing in this session's memory")),
        "{second:#?}"
    );
    // The failure came back before the next request, so that request carries it, fenced as
    // untrusted task output in a user turn (never a system message).
    let reported = user_texts(requests[2].messages())
        .iter()
        .any(|text| text.contains("no coding harness is installed") && text.contains("untrusted"));
    assert!(reported, "the harness failure is reported to the talker");
    assert_eq!(requests.len(), 3);
}

/// Closing mid-request drops the stream and flushes the store.
#[tokio::test(start_paused = true)]
async fn closing_cancels_the_stream_and_flushes_the_store() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.pending();
    let harness = start(Arc::clone(&talker), false);
    assert!(
        say_typed(&harness.session.speech_in(), "Explain Raft.")
            .await
            .is_ok()
    );
    assert!(run_until(5_000, || talker.requests().len() == 1).await);
    let (_, recorder, talker) = close(harness).await;
    assert_eq!(talker.dropped.load(Ordering::SeqCst), 1);
    assert!(recorder.flushed.lock().is_ok_and(|flushed| *flushed));
}

#[tokio::test]
async fn a_full_inbox_refuses_inputs_that_cannot_wait() {
    let channels = SessionChannels::new();
    for _ in 0..SESSION_INBOX_CAPACITY {
        assert!(channels.inbox.try_send(Input::Tick).is_ok());
    }
    assert_eq!(
        channels.inbox.try_send(Input::Tick),
        Err(InboxRefused::Full)
    );
}
