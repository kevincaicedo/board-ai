//! A request started on an early final (D-262), through the orchestrator: it goes out at once,
//! its stream is dropped when speech resumes (nothing it sent reaches the screen), and the
//! request of the finished early final answers the turn once the final confirms it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use boardai_core::RequestStart;
use boardai_core::effect::Milestone;
use boardai_core::history::{Message, UserPart};
use boardai_core::protocol::UiEvent;

use super::{DEMO, ScriptedTalker, close, idle_after_speaking, run_until, start_configured};

fn last_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        Message::User { parts } => parts.first().and_then(|part| match part {
            UserPart::Text { text } => Some(text.clone()),
            UserPart::ToolResult { .. } => None,
        }),
        Message::Assistant { .. } | Message::System { .. } => None,
    })
}

#[tokio::test(start_paused = true)]
async fn a_resume_drops_the_early_stream_and_the_confirmed_one_answers() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("UNHEARD, a reply to half a question.");
    talker.says(DEMO);
    let harness = start_configured(Arc::clone(&talker), false, None, |parts| {
        parts.core.request_start = RequestStart::OnEarlyFinal;
    });
    let link = harness.session.speech_in();
    assert!(link.voice_start().await.is_ok());
    assert!(link.early_final("Why does it fail?".into()).await.is_ok());
    assert!(run_until(1_000, || talker.requests().len() == 1).await);
    // The speaker goes on 40 ms later: the early stream may have sent its first delta already.
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert!(link.voice_resumed().await.is_ok());
    assert!(run_until(1_000, || talker.dropped.load(Ordering::SeqCst) == 1).await);
    let whole = "Why does it fail on the second write?";
    assert!(link.early_final(whole.into()).await.is_ok());
    assert!(run_until(1_000, || talker.requests().len() == 2).await);
    // The detector calls the end ~110 ms after the early final: the answer's first deltas are in.
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(link.voice_end(1_800, 256).await.is_ok());
    assert!(link.transcript(whole.into(), true).await.is_ok());
    let recorder = Arc::clone(&harness.recorder);
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    let (_stats, recorder, talker) = close(harness).await;
    let requests = talker.requests();
    assert_eq!(requests.len(), 2, "the confirmed turn asked nothing again");
    assert_eq!(
        last_user_text(requests[1].messages()).as_deref(),
        Some(whole)
    );
    let shown_unheard = recorder
        .events()
        .iter()
        .any(|event| matches!(event, UiEvent::Caption { text, .. } if text.contains("UNHEARD")));
    assert!(!shown_unheard, "nothing of the dropped stream was shown");
    // The whole answer, its first deltas held until the turn was confirmed.
    let ops = recorder
        .events()
        .iter()
        .filter(|event| matches!(event, UiEvent::OpStart { .. }))
        .count();
    assert_eq!(ops, 3, "the browser, the CDN and the edge between them");
    assert!(recorder.events().iter().any(|event| matches!(
        event,
        UiEvent::Caption { text, is_final: true, .. } if text.starts_with("Sure.")
    )));
    let marks = recorder.marks();
    let early = marks
        .iter()
        .filter(|mark| **mark == Milestone::SpeculativeRequest)
        .count();
    assert_eq!(early, 2);
    assert!(marks.contains(&Milestone::FirstAudio));
}
