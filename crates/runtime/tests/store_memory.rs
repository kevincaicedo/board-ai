//! Memory writes as the policy decided them, proposals kept only after a yes, "forget that",
//! and "don't remember this session" leaving the vault exactly as it was (I10).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::Speaker;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::store::{
    ForgottenKind, MemoryFileName, Refusal, Remembered, SessionEnded, SessionFinished, Store,
    StoreError,
};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, at, caption, close, good_answer, keep, memory_file, node,
    recall_text, snapshot, task, write,
};
use support::TempFolder;

use MemoryAction::{Keep, OnRequest, Propose};
use MemoryKind::{Inferred, LearningSignal, Note, SessionRecord, Stated};

const FACTS_HEADER: &str = "# Facts\n\nWhat you told BoardAI about your systems. Edit freely; \
keep the ^ids at the ends of lines.\n\n";

async fn finish(store: &Store, scribe: Option<&FakeScribe>) -> String {
    let scribe = scribe.map(|scribe| scribe as &dyn LlmProvider);
    let finished = store
        .finish_session(at(60_000), scribe, None, CancelToken::new())
        .await
        .expect("finishes");
    let SessionFinished::Written { session, .. } = finished else {
        panic!("written: {finished:?}");
    };
    session.folder
}

#[tokio::test]
async fn memories_go_to_the_file_of_their_subject() {
    let folder = TempFolder::new("store-memory-files");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    let fact = keep(&store, Stated, Keep, "We use Kafka for the event bus").await;
    let preference = keep(&store, Stated, Keep, "I prefer Go examples").await;
    let signal = keep(
        &store,
        LearningSignal,
        Keep,
        "Interrupted 3 times on log matching",
    )
    .await;
    assert_eq!(memory_file(&fact), Some(MemoryFileName::Facts));
    assert_eq!(memory_file(&preference), Some(MemoryFileName::AboutYou));
    assert_eq!(memory_file(&signal), Some(MemoryFileName::AboutYou));
    assert_eq!(
        test.read("memory/facts.md"),
        format!("{FACTS_HEADER}- We use Kafka for the event bus ^m1\n")
    );
    let about = test.read("memory/about-you.md");
    assert!(
        about.ends_with("- I prefer Go examples ^m2\n- Interrupted 3 times on log matching ^m3\n"),
        "{about}"
    );
    let answer = recall_text(store.recall("kafka event bus").await);
    assert!(
        answer
            .contains("[memory] facts.md (memory/facts.md): We use «Kafka» for the «event» «bus»"),
        "{answer}"
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn notes_and_tasks_are_files_with_links_back_to_the_session() {
    let folder = TempFolder::new("store-memory-notes-tasks");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let id = store
        .start_session(None)
        .await
        .expect("starts")
        .id
        .to_string();
    let mut note = write(
        Note,
        OnRequest,
        "Cache-aside: read through Redis\nTTL is 5 minutes",
    );
    note.links = vec!["src/cache.rs:42".into()];
    let Remembered::Note { path, .. } = store.remember(note).await.expect("note") else {
        panic!("a note");
    };
    assert_eq!(path, "notes/cache-aside-read-through-redis.md");
    assert_eq!(
        test.read(&path),
        format!(
            "---\nid: n1\ncreated: 2026-10-15T14:32:10Z\nsession: {id}\nlinks: [\"src/cache.rs:42\"]\n\
             ---\nCache-aside: read through Redis\nTTL is 5 minutes\n"
        )
    );
    let added = task(
        "Check Redis TTLs",
        Some("Harness project"),
        &["src/cache.rs:42"],
    );
    let Remembered::Task { path, project, .. } = store.remember(added).await.expect("task") else {
        panic!("a task");
    };
    assert_eq!(
        (path.as_str(), project.as_str()),
        ("projects/harness-project.md", "Harness project")
    );
    assert_eq!(
        test.read(&path),
        format!(
            "# Harness project\n\n- [ ] Check Redis TTLs [[session:{id}]] [[src/cache.rs:42]] ^t1\n"
        )
    );
    let inbox = store
        .remember(task("Read the Raft paper", None, &[]))
        .await
        .expect("task");
    assert!(matches!(inbox, Remembered::Task { ref path, .. } if path == "projects/inbox.md"));

    let now = store.now_list(None).await.expect("now");
    let titles: Vec<&str> = now.tasks.iter().map(|task| task.title.as_str()).collect();
    assert_eq!(titles, vec!["Check Redis TTLs", "Read the Raft paper"]);
    let harness = store
        .now_list(Some("harness project".into()))
        .await
        .expect("now");
    assert_eq!(harness.tasks.len(), 1);
    assert_eq!(harness.tasks[0].session.as_deref(), Some(id.as_str()));

    assert_eq!(
        keep(&store, SessionRecord, Keep, "Paused at retries").await,
        Remembered::SessionRecord
    );
    let digest = test.read(&format!("{}/digest.md", finish(&store, None).await));
    assert!(
        digest.contains("- Task: Check Redis TTLs (projects/harness-project.md ^t1)"),
        "{digest}"
    );
    assert!(digest.contains("- Noted: Paused at retries"), "{digest}");
    close(store, task_handle).await;
}

#[tokio::test]
async fn what_must_never_be_stored_is_refused_with_its_reason() {
    let folder = TempFolder::new("store-memory-refused");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let no_session = store.remember(write(Stated, Keep, "We use Kafka")).await;
    assert_eq!(no_session, Err(StoreError::NoSession));
    store.start_session(None).await.expect("starts");
    let before = snapshot(&test.vault_root());
    for (kind, action, reason) in [
        (Stated, MemoryAction::Never, Refusal::NeverAction),
        (MemoryKind::RawAudio, Keep, Refusal::RawAudio),
        (MemoryKind::RawAudio, OnRequest, Refusal::RawAudio),
        (MemoryKind::Secret, Keep, Refusal::Secret),
        (MemoryKind::Secret, Propose, Refusal::Secret),
    ] {
        let result = store.remember(write(kind, action, "anything")).await;
        assert_eq!(
            result,
            Err(StoreError::Refused { reason }),
            "{kind:?} {action:?}"
        );
    }
    assert_eq!(snapshot(&test.vault_root()), before, "nothing was written");
    close(store, task_handle).await;
}

#[tokio::test]
async fn proposals_are_kept_only_after_a_yes() {
    let folder = TempFolder::new("store-memory-proposals");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    let Remembered::Proposed { id } = keep(&store, Inferred, Propose, "Likes a slower pace").await
    else {
        panic!("proposed");
    };
    assert!(
        !test.vault().join("memory/about-you.md").exists(),
        "not kept yet"
    );
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Request path")));
    let digest_path = format!("{}/digest.md", finish(&store, Some(&scribe)).await);
    let digest = test.read(&digest_path);
    assert!(
        digest.contains(&format!("- [ ] About you: Likes a slower pace ^{id}")),
        "{digest}"
    );
    assert!(
        digest.contains("- [ ] About your systems: The user API uses cache-aside with Redis ^p3"),
        "{digest}"
    );
    assert!(
        !test.vault().join("memory/facts.md").exists(),
        "scribe proposals are not kept"
    );

    let kept = store.accept_proposal(&id.to_string()).await.expect("kept");
    assert_eq!(memory_file(&kept), Some(MemoryFileName::AboutYou));
    assert!(
        test.read("memory/about-you.md")
            .contains("- Likes a slower pace ^m1")
    );
    assert!(
        test.read(&digest_path)
            .contains(&format!("- [x] About you: Likes a slower pace ^{id}"))
    );
    let again = store.accept_proposal(&id.to_string()).await;
    assert!(
        matches!(again, Err(StoreError::NotFound { .. })),
        "{again:?}"
    );
    let not_a_proposal = store.accept_proposal("t1").await;
    assert!(matches!(not_a_proposal, Err(StoreError::NotFound { .. })));
    close(store, task_handle).await;
}

#[tokio::test]
async fn an_earlier_sessions_proposal_can_be_kept_later() {
    let folder = TempFolder::new("store-memory-past-proposal");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Request path")));
    let digest_path = format!("{}/digest.md", finish(&store, Some(&scribe)).await);
    store.start_session(None).await.expect("next session");
    let kept = store
        .accept_proposal("p2")
        .await
        .expect("kept from the index");
    assert_eq!(memory_file(&kept), Some(MemoryFileName::Facts));
    assert!(
        test.read("memory/facts.md")
            .contains("- The user API uses cache-aside with Redis ^m1")
    );
    assert!(
        test.read(&digest_path)
            .contains("- [x] About your systems: The user API uses cache-aside with Redis ^p2")
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn forget_that_removes_the_last_capture_from_the_file_and_the_index() {
    let folder = TempFolder::new("store-memory-forget-last");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    assert_eq!(store.forget_last().await, Ok(None));
    keep(&store, Stated, Keep, "We use Kafka").await;
    keep(&store, Stated, Keep, "Our cluster runs Zookeeper").await;
    keep(&store, Note, OnRequest, "Quorum reads cost a round trip").await;

    let forgotten = store
        .forget_last()
        .await
        .expect("forgets")
        .expect("something");
    assert_eq!(forgotten.what, ForgottenKind::Note);
    assert!(
        !test
            .vault()
            .join("notes/quorum-reads-cost-a-round-trip.md")
            .exists()
    );
    let forgotten = store
        .forget_last()
        .await
        .expect("forgets")
        .expect("something");
    assert_eq!(
        (forgotten.what, forgotten.text.as_str()),
        (ForgottenKind::Memory, "Our cluster runs Zookeeper")
    );
    let facts = test.read("memory/facts.md");
    assert!(
        facts.contains("We use Kafka") && !facts.contains("Zookeeper"),
        "{facts}"
    );
    let answer = recall_text(store.recall("zookeeper quorum").await);
    assert!(answer.starts_with("Nothing"), "{answer}");
    assert!(recall_text(store.recall("kafka").await).contains("We use «Kafka»"));
    close(store, task_handle).await;
}

/// An earlier session (a digest with proposals p1 and p2), facts written by hand without a
/// final newline, and an existing project.
async fn earlier_session(test: &TestStore, store: &Store) -> String {
    store.start_session(None).await.expect("starts");
    store.log_ui(
        at(100),
        vec![caption(
            Speaker::User,
            "How does the load balancer pick a pod?",
        )],
    );
    store
        .remember(task("Check pod readiness", Some("Harness"), &[]))
        .await
        .expect("task");
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Load balancing")));
    let folder = finish(store, Some(&scribe)).await;
    std::fs::write(
        test.vault().join("memory/facts.md"),
        "# Facts\n\n- We use Kafka ^m40\n- hand-written line without an id",
    )
    .expect("hand edit");
    test.clock.advance_ms(3_600_000);
    folder
}

/// A session that writes every kind of thing, including accepting an earlier proposal.
async fn busy_session(store: &Store) {
    store
        .start_session(Some("Retries".into()))
        .await
        .expect("starts");
    store.log_ui(
        at(10),
        vec![caption(
            Speaker::User,
            "Walk me through retries with jitter",
        )],
    );
    store.log_ui(
        at(20),
        vec![
            node(0, "api", "API"),
            caption(Speaker::Assistant, "Retries back off."),
        ],
    );
    keep(store, Stated, Keep, "Our retries use full jitter").await;
    keep(store, Stated, Keep, "I prefer diagrams before code").await;
    keep(store, Note, OnRequest, "Jitter spreads the herd").await;
    store
        .remember(task(
            "Add jitter to the client",
            Some("Harness"),
            &["client.rs:12"],
        ))
        .await
        .expect("task");
    store
        .remember(task("Benchmark backoff", Some("Resilience"), &[]))
        .await
        .expect("task");
    keep(store, SessionRecord, Keep, "Stopped at circuit breakers").await;
    let Remembered::Proposed { id } = keep(store, Inferred, Propose, "Likes worked examples").await
    else {
        panic!("proposed");
    };
    store
        .accept_proposal(&id.to_string())
        .await
        .expect("accepted");
    store
        .accept_proposal("p2")
        .await
        .expect("an earlier session's proposal");
    store.flush().await.expect("flush");
}

#[tokio::test]
async fn dont_remember_this_session_leaves_the_vault_exactly_as_it_was() {
    let folder = TempFolder::new("store-memory-forget-session");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let earlier = earlier_session(&test, &store).await;
    let library_before = store.library().await.expect("library");
    let now_before = store.now_list(None).await.expect("now");
    let before = snapshot(&test.vault_root());

    busy_session(&store).await;
    assert_ne!(
        snapshot(&test.vault_root()),
        before,
        "the session wrote things"
    );
    store.forget_session().await.expect("forgets");
    assert_eq!(
        snapshot(&test.vault_root()),
        before,
        "nothing of the session is left"
    );
    assert!(
        test.read(&format!("{earlier}/digest.md"))
            .contains("- [ ] About your systems")
    );
    assert_eq!(store.library().await.expect("library"), library_before);
    assert_eq!(store.now_list(None).await.expect("now"), now_before);
    for words in [
        "jitter",
        "circuit breakers",
        "worked examples",
        "diagrams before code",
    ] {
        let answer = recall_text(store.recall(words).await);
        assert!(answer.starts_with("Nothing"), "{words}: {answer}");
    }

    // Nothing more is written for it.
    let after = store.remember(write(Stated, Keep, "We use Postgres")).await;
    assert_eq!(after, Err(StoreError::SessionForgotten));
    store.log_ui(at(30), vec![caption(Speaker::User, "still talking")]);
    assert_eq!(store.end_session(at(40)).await, Ok(SessionEnded::Forgotten));
    assert_eq!(snapshot(&test.vault_root()), before);
    close(store, task_handle).await;
}

#[tokio::test]
async fn a_session_forgotten_after_its_digest_leaves_nothing_either() {
    let folder = TempFolder::new("store-memory-forget-digested");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    earlier_session(&test, &store).await;
    let before = snapshot(&test.vault_root());
    busy_session(&store).await;
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Retries with jitter")));
    let session_folder = finish(&store, Some(&scribe)).await;
    assert!(
        session_folder.ends_with("-retries-with-jitter"),
        "{session_folder}"
    );
    store.forget_session().await.expect("forgets");
    assert_eq!(snapshot(&test.vault_root()), before);
    assert_eq!(store.library().await.expect("library").sessions.len(), 1);
    close(store, task_handle).await;
}
