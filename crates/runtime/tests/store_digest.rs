//! The session digest: the scribe's when its answer is good, the minimal one otherwise, and
//! the session is never lost either way.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::Speaker;
use boardai_core::tools::ToolOutput;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::store::{
    FallbackReason, SCRIBE_SYSTEM_PROMPT, SessionFinished, Store, WrittenBy, scribe_config,
};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, at, caption, close, good_answer, node, task,
};
use support::TempFolder;

const BOARD_EXPORT: &str = r#"{"type":"excalidraw","version":2,"elements":[]}"#;

async fn session_with_talk(store: &Store) {
    store
        .start_session(Some("Request path".into()))
        .await
        .expect("starts");
    store.log_ui(
        at(1_000),
        vec![caption(
            Speaker::User,
            "Walk me through the request path for our user API.",
        )],
    );
    store.log_ui(
        at(2_000),
        vec![
            node(0, "cdn", "CDN"),
            caption(
                Speaker::Assistant,
                "A request starts at the client and hits the CDN.",
            ),
        ],
    );
    store.log_ui(
        at(9_000),
        vec![caption(Speaker::User, "Why does the CDN cache errors?")],
    );
    store
        .remember(task("Check Redis TTLs", None, &[]))
        .await
        .expect("task");
}

async fn finish(store: &Store, scribe: &FakeScribe) -> (String, WrittenBy) {
    let finished = store
        .finish_session(
            at(61_000),
            Some(scribe as &dyn LlmProvider),
            Some(BOARD_EXPORT.into()),
            CancelToken::new(),
        )
        .await
        .expect("finished");
    let SessionFinished::Written {
        session,
        written_by,
    } = finished
    else {
        panic!("written")
    };
    (session.folder, written_by)
}

#[tokio::test]
async fn a_good_scribe_answer_becomes_the_digest_and_the_graph() {
    let folder = TempFolder::new("store-digest-good");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    session_with_talk(&store).await;
    let scribe = FakeScribe::new(ScribeAnswer::Text(format!(
        "```json\n{}\n```",
        good_answer("Request path")
    )));
    let (session_folder, written_by) = finish(&store, &scribe).await;
    assert_eq!(written_by, WrittenBy::Scribe);
    assert_eq!(
        session_folder, "sessions/2026-10-15-request-path",
        "renamed after the title"
    );
    assert!(!test.vault().join("sessions/2026-10-15-143210").exists());

    let digest = test.read(&format!("{session_folder}/digest.md"));
    for expected in [
        "---\nid: 2026-10-15-143210\ntitle: \"Request path\"\nstarted: 2026-10-15T14:32:10Z\nduration_ms: 61000\nwritten_by: scribe\n---\n# Request path\n",
        "## Summary\n\nWe traced a request from the client",
        "## Key diagrams\n\n- [Request path](board.excalidraw)\n",
        "## Open questions\n\n- Why does the CDN cache 404s?\n",
        "## Captured\n\n- Task: Check Redis TTLs (projects/inbox.md ^t1)\n",
        "## Proposed memories\n\nNothing here is kept until you say yes.\n\n- [ ] About you: Likes a slower pace on networking ^p1\n",
        "## Graph\n\n- Redis (system)\n- Load balancer (concept)\n- API `depends_on` Redis\n- you `struggled_with` Load balancer\n",
    ] {
        assert!(
            digest.contains(expected),
            "missing {expected:?} in\n{digest}"
        );
    }
    assert_eq!(
        test.read(&format!("{session_folder}/board.excalidraw")),
        BOARD_EXPORT
    );

    let prompts = scribe.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("[00:01] You: Walk me through the request path for our user API."));
    assert!(
        prompts[0].contains("[00:02] BoardAI: A request starts at the client and hits the CDN.")
    );
    assert!(prompts[0].contains("- Task: Check Redis TTLs"));
    assert_eq!(scribe_config().system_prompt, SCRIBE_SYSTEM_PROMPT);

    let ToolOutput::Text { text } = store.recall("redis").await else {
        panic!("recall")
    };
    assert!(
        text.contains("[digest] Request path (session 2026-10-15-143210, 2026-10-15)"),
        "{text}"
    );
    let opened = store
        .open_session("2026-10-15-143210")
        .await
        .expect("the id survives the rename");
    assert!(
        opened
            .events_path
            .ends_with("sessions/2026-10-15-request-path/events.jsonl")
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn junk_failures_and_refusals_give_the_minimal_digest() {
    for (label, answer, reason) in [
        (
            "junk",
            ScribeAnswer::Text("Sure! Here's a summary of the session.".into()),
            "bad_output",
        ),
        ("fails", ScribeAnswer::Fails, "provider_failed"),
        ("refuses", ScribeAnswer::Refuses, "incomplete"),
    ] {
        let folder = TempFolder::new(&format!("store-digest-{label}"));
        let test = TestStore::new(&folder.path);
        let (store, task_handle, _) = test.open().await;
        session_with_talk(&store).await;
        let scribe = FakeScribe::new(answer);
        let (session_folder, written_by) = finish(&store, &scribe).await;
        let WrittenBy::Fallback { reason: fallback } = &written_by else {
            panic!("{label}: {written_by:?}")
        };
        assert!(
            serde_json::to_string(fallback)
                .unwrap_or_default()
                .contains(reason),
            "{label}: {fallback:?}"
        );
        let digest = test.read(&format!("{session_folder}/digest.md"));
        assert!(digest.contains("written_by: fallback"), "{label}");
        assert!(
            digest.contains("# Request path\n"),
            "{label}: the title hint"
        );
        assert!(
            digest.contains("You asked: \"Walk me through the request path for our user API.\". 2 exchange(s) over about 2 min."),
            "{label}: {digest}"
        );
        assert!(
            digest.contains("- Task: Check Redis TTLs (projects/inbox.md ^t1)"),
            "{label}"
        );
        assert!(digest.contains("- [Board](board.excalidraw)"), "{label}");
        assert_eq!(
            store.library().await.expect("library").sessions.len(),
            1,
            "{label}"
        );
        close(store, task_handle).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_scribe_that_never_answers_times_out_into_the_minimal_digest() {
    let folder = TempFolder::new("store-digest-hang");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    session_with_talk(&store).await;
    let scribe = FakeScribe::new(ScribeAnswer::Hangs);
    let (_, written_by) = finish(&store, &scribe).await;
    assert_eq!(
        written_by,
        WrittenBy::Fallback {
            reason: FallbackReason::TimedOut
        }
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn without_a_scribe_or_a_transcript_the_session_is_still_kept() {
    let folder = TempFolder::new("store-digest-empty");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    store
        .remember(store_support::write(
            MemoryKind::Inferred,
            MemoryAction::Propose,
            "Likes short answers",
        ))
        .await
        .expect("proposed");
    let finished = store
        .finish_session(at(30_000), None, None, CancelToken::new())
        .await
        .expect("finished");
    let SessionFinished::Written {
        session,
        written_by,
    } = finished
    else {
        panic!("written")
    };
    assert_eq!(
        written_by,
        WrittenBy::Fallback {
            reason: FallbackReason::NoScribe
        }
    );
    assert_eq!(session.folder, "sessions/2026-10-15-untitled-session");
    let digest = test.read(&format!("{}/digest.md", session.folder));
    assert!(
        digest.contains("A session of about 1 min with no transcript."),
        "{digest}"
    );
    assert!(digest.contains("- [ ] About you: Likes short answers ^p1"));
    assert!(
        !digest.contains("## Key diagrams"),
        "no board, no diagrams section"
    );
    let recap = store.recap().await.expect("recap").expect("one digest");
    assert_eq!(
        recap,
        "Last time (2026-10-15), \"Untitled session\": A session of about 1 min with no transcript."
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn a_session_cut_short_still_gets_a_digest() {
    let folder = TempFolder::new("store-digest-interrupted");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    // Ended without a digest, then the next session starts.
    session_with_talk(&store).await;
    store.end_session(at(10_000)).await.expect("ends");
    test.clock.advance_ms(60_000);
    store.start_session(None).await.expect("the next session");
    // And the app quits mid-session.
    store.log_ui(
        at(500),
        vec![caption(Speaker::User, "Explain consistent hashing")],
    );
    close(store, task_handle).await;

    let (store, task_handle, _) = test.open().await;
    let library = store.library().await.expect("library");
    let titles: Vec<&str> = library
        .sessions
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Explain consistent hashing", "Request path"]);
    for entry in &library.sessions {
        let opened = store.open_session(entry.id.as_str()).await.expect("opens");
        let folder = opened.events_path.parent().expect("folder").to_path_buf();
        let digest = std::fs::read_to_string(folder.join("digest.md")).expect("a digest");
        assert!(digest.contains("written_by: fallback"), "{digest}");
        let log = std::fs::read_to_string(&opened.events_path).expect("log");
        assert!(
            log.trim_end().ends_with(r#""k":"end"}"#),
            "the log was closed cleanly"
        );
    }
    close(store, task_handle).await;
}
