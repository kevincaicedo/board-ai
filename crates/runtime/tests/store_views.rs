//! The store's reads for the views (LLD §3.7) and the session's end: library covers, the Now
//! list with where each task came from, memory, a replay's log by id only, a bounded digest
//! when quitting, and the minimal digest a crash leaves for the next start (LLD §3.6).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::Speaker;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::store::{
    FallbackReason, SessionFinished, SessionId, Store, StoreError, WrittenBy,
};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, at, caption, close, keep, keep_you, node, task,
};
use support::TempFolder;

const BOARD: &str = r#"{"type":"excalidraw","version":2,"elements":[
    {"id":"l0:lb","type":"rectangle","x":0,"y":0,"width":160,"height":64,"strokeStyle":"solid"},
    {"id":"l0:db","type":"ellipse","x":240,"y":0,"width":120,"height":80,"strokeStyle":"solid"}]}"#;

fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create");
    for entry in std::fs::read_dir(from).expect("readable").flatten() {
        std::fs::copy(entry.path(), to.join(entry.file_name())).expect("copy");
    }
}

/// A finished session with a memory about you, a fact, a note, a task and a two-node board.
async fn seed(store: &Store) {
    let title = Some("Request path".to_owned());
    store.start_session(title).await.expect("starts");
    keep_you(
        store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "Examples in Go",
    )
    .await;
    keep(
        store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "We use Kafka",
    )
    .await;
    let note = "Cache-aside: Redis first";
    keep(store, MemoryKind::Note, MemoryAction::OnRequest, note).await;
    let ttl = task("Check Redis TTLs", Some("Harness"), &["service.go:50"]);
    store.remember(ttl).await.expect("task");
    let board = Some(BOARD.to_owned());
    let end = at(18 * 60_000);
    let finished = store.finish_session(end, None, board, CancelToken::new());
    finished.await.expect("finished");
}

#[tokio::test]
async fn library_now_list_and_memory_read_the_vault() {
    let folder = TempFolder::new("store-views-read");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    seed(&store).await;
    let library = store.library().await.expect("library");
    let entry = &library.sessions[0];
    assert_eq!(entry.title, "Request path");
    assert_eq!(entry.date, "2026-10-15");
    assert_eq!(
        entry.shapes.len(),
        2,
        "the board's two nodes make the cover"
    );

    let now = store.now_list(Some("harness".into())).await.expect("now");
    assert_eq!(now.tasks[0].title, "Check Redis TTLs");
    assert_eq!(now.tasks[0].session_title.as_deref(), Some("Request path"));
    assert_eq!(now.tasks[0].session_date.as_deref(), Some("2026-10-15"));

    let memory = store.memory_view().await.expect("memory");
    let texts = |rows: &[boardai_core::views::MemoryRow]| -> Vec<String> {
        rows.iter().map(|row| row.text.clone()).collect()
    };
    assert_eq!(texts(&memory.about_you), vec!["Examples in Go"]);
    assert_eq!(texts(&memory.systems), vec!["We use Kafka"]);
    assert_eq!(memory.notes[0].text, "Cache-aside: Redis first");
    assert_eq!(
        memory.notes[0]
            .from
            .as_ref()
            .map(|from| from.title.as_str()),
        Some("Request path")
    );
    let latest = memory.latest.expect("the digest card");
    assert_eq!((latest.notes, latest.tasks), (1, 1));
    assert_eq!(latest.session.cover.len(), 2);
    close(store, task_handle).await;
}

/// A replay reads a log by its exact id, never a path, and only within its bound.
#[tokio::test]
async fn a_session_log_is_read_by_id_only() {
    let folder = TempFolder::new("store-views-log");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let info = store.start_session(None).await.expect("starts");
    store.log_ui(at(5), vec![caption(Speaker::User, "hello")]);
    store
        .finish_session(at(1_000), None, None, CancelToken::new())
        .await
        .expect("finished");
    let log = store.session_log(&info.id).await.expect("read");
    assert!(
        log.contains("\"k\":\"session\"") && log.contains("hello"),
        "{log}"
    );
    let unknown = SessionId::parse("2026-01-01-000000").expect("an id");
    assert!(matches!(
        store.session_log(&unknown).await,
        Err(StoreError::NotFound { .. })
    ));
    assert!(
        SessionId::parse("../../etc/passwd").is_none(),
        "not an id at the boundary"
    );
    close(store, task_handle).await;
}

/// Quitting never waits on the model: past the deadline the minimal digest is written.
#[tokio::test]
async fn a_hanging_scribe_gives_the_minimal_digest_within_the_deadline() {
    let folder = TempFolder::new("store-views-quit");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    store.log_ui(at(5), vec![caption(Speaker::User, "explain Raft")]);
    let scribe = FakeScribe::new(ScribeAnswer::Hangs);
    let began = Instant::now();
    let finished = store
        .finish_session_within(
            at(2_000),
            Some(&scribe),
            None,
            Duration::from_millis(200),
            CancelToken::new(),
        )
        .await
        .expect("finished");
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "{:?}",
        began.elapsed()
    );
    let SessionFinished::Written { written_by, .. } = finished else {
        panic!("written");
    };
    assert_eq!(
        written_by,
        WrittenBy::Fallback {
            reason: FallbackReason::TimedOut
        }
    );
    // A cancel ends the wait at once, the same way.
    store.start_session(None).await.expect("starts");
    let cancel = CancelToken::new();
    cancel.cancel();
    let finished = store
        .finish_session_within(
            at(1_000),
            Some(&scribe),
            None,
            Duration::from_secs(60),
            cancel,
        )
        .await
        .expect("finished");
    assert!(matches!(
        finished,
        SessionFinished::Written {
            written_by: WrittenBy::Fallback {
                reason: FallbackReason::Cancelled
            },
            ..
        }
    ));
    close(store, task_handle).await;
}

/// A crash leaves the log (flushed every 250 ms) and no digest; the next start writes the
/// minimal one, so the session is in the library with a title and a recap.
#[tokio::test]
async fn a_crashed_session_gets_the_minimal_digest_on_the_next_start() {
    let first = TempFolder::new("store-views-crash-a");
    let test = TestStore::new(&first.path);
    let (store, task_handle, _) = test.open().await;
    let info = store.start_session(None).await.expect("starts");
    store.log_ui(
        at(10),
        vec![
            caption(Speaker::User, "How does Raft compact its log?"),
            node(0, "leader", "leader"),
        ],
    );
    store.flush().await.expect("flushed");
    // The folder as the crash left it: a log without an end line, and no digest.
    let crashed = TempFolder::new("store-views-crash-b");
    let next = TestStore::new(&crashed.path);
    copy_folder(
        &test.vault().join(&info.folder),
        &next.vault().join(&info.folder),
    );
    close(store, task_handle).await;
    assert!(!next.vault().join(&info.folder).join("digest.md").exists());

    let (store, task_handle, _) = next.open().await;
    let library = store.library().await.expect("library");
    assert_eq!(library.sessions.len(), 1);
    assert_eq!(library.sessions[0].title, "How does Raft compact its log?");
    let recap = store.recap().await.expect("recap").expect("a digest now");
    assert!(recap.contains("How does Raft compact its log?"), "{recap}");
    close(store, task_handle).await;
    let folders: Vec<String> = std::fs::read_dir(next.vault().join("sessions"))
        .expect("sessions")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(folders.len(), 1);
    let digest = next.read(&format!("sessions/{}/digest.md", folders[0]));
    assert!(
        digest.contains("written_by: fallback") && digest.contains("# How does Raft"),
        "{digest}"
    );
}
