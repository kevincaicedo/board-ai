//! The vault sink's promises (LLD §3.1, §3.6), at the seam between the orchestrator and the
//! store: a session that failed to start can never write to or forget the previous one; "forget
//! that" undoes the capture the core named even after a failure; and "don't remember this
//! session" is never dropped and never ends in a digest (I10).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::effect::ForgetScope;
use boardai_core::ids::CaptureId;
use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
use boardai_core::protocol::Speaker;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::limits::STORE_WRITES_CAPACITY;
use boardai_runtime::session::{SessionClose, StoreSink, VaultSink};
use boardai_runtime::store::{Store, WallClock};
use store_support::{TestStore, at, caption, close, snapshot};
use support::TempFolder;

fn memory(kind: MemoryKind, action: MemoryAction, text: &str, capture: u32) -> MemoryWrite {
    MemoryWrite {
        kind,
        action,
        text: text.to_owned(),
        links: Vec::new(),
        project: None,
        subject: Subject::You,
        capture: CaptureId(capture),
    }
}

fn folders(sessions: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(sessions)
        .expect("sessions")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn sink(store: &Store, test: &TestStore) -> VaultSink {
    VaultSink::new(store.clone(), None, Arc::new(test.clock.clone()))
}

/// Session A: one question and one note, then its minimal digest.
async fn session_a(sink: &VaultSink, store: &Store) {
    let opened = sink.start_session().await;
    assert!(opened.session.is_some(), "A starts");
    store.log_ui(
        at(10),
        vec![caption(Speaker::User, "How does Raft compact its log?")],
    );
    sink.remember(
        at(20),
        &memory(
            MemoryKind::Note,
            MemoryAction::OnRequest,
            "Raft snapshots compact the log",
            0,
        ),
    );
    sink.finish_session(SessionClose::Digest {
        t_end: at(60_000),
        board: None,
        deadline: Duration::from_secs(1),
        cancel: CancelToken::new(),
    })
    .await;
}

/// Makes session B's start fail: a dangling link stands where its folder would go (as a full
/// disk or an I/O error creating the folder or its log would).
fn break_next_start(test: &TestStore) {
    test.clock.advance_ms(60_000);
    let now = test.clock.now();
    let next =
        test.vault()
            .join("sessions")
            .join(format!("{}-{}", now.date(), now.time_of_day_compact()));
    std::os::unix::fs::symlink("/nonexistent-boardai-target", &next).expect("link");
}

/// Finding 5: B's start fails ("memory is off"), and B's "don't remember this session" must
/// not reach A, which the store's slot still held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_never_lets_the_next_session_forget_the_previous_one() {
    let folder = TempFolder::new("sink-failed-start-forget");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let sink = sink(&store, &test);
    session_a(&sink, &store).await;
    let sessions = test.vault().join("sessions");
    let before = folders(&sessions);
    let note = test.vault().join("notes/raft-snapshots-compact-the-log.md");
    assert!(note.is_file(), "A's note was written");

    break_next_start(&test);
    let opened = sink.start_session().await;
    assert_eq!(opened.session, None, "B's start failed: memory is off");
    sink.remember(
        at(2),
        &memory(MemoryKind::Stated, MemoryAction::Keep, "I prefer Go", 0),
    );
    sink.forget(at(5), ForgetScope::Session);
    sink.flush().await;

    assert_eq!(folders(&sessions), before, "A's folder is still there");
    assert!(note.is_file(), "A's note is still there");
    assert!(
        !test.vault().join("memory/about-you.md").exists(),
        "B's write went nowhere: no session was recording"
    );
    close(store, task).await;
}

/// Finding 5, with nobody speaking in B: discarding B must not delete A.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_then_a_silent_session_keeps_the_previous_one() {
    let folder = TempFolder::new("sink-failed-start-discard");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let sink = sink(&store, &test);
    session_a(&sink, &store).await;
    let sessions = test.vault().join("sessions");
    let before = folders(&sessions);
    let note = test.vault().join("notes/raft-snapshots-compact-the-log.md");

    break_next_start(&test);
    let opened = sink.start_session().await;
    assert_eq!(opened.session, None, "B's start failed: memory is off");
    sink.finish_session(SessionClose::Discard).await;

    assert_eq!(folders(&sessions), before, "A's folder is still there");
    assert!(note.is_file(), "A's note is still there");
    close(store, task).await;
}

/// Finding 13: a "forget that" the store fails to carry out must not shift what the next one
/// undoes: the core named the note (capture 0), so the note goes, and the memory whose undo
/// failed is still kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_forget_that_never_makes_the_next_one_undo_the_wrong_capture() {
    let folder = TempFolder::new("sink-forget-after-failure");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let sink = sink(&store, &test);
    assert!(sink.start_session().await.session.is_some());
    sink.remember(
        at(10),
        &memory(MemoryKind::Note, MemoryAction::OnRequest, "Note one", 0),
    );
    sink.remember(
        at(20),
        &memory(MemoryKind::Stated, MemoryAction::Keep, "I prefer Go", 1),
    );
    sink.flush().await;
    let note = test.vault().join("notes/note-one.md");
    let about = test.vault().join("memory/about-you.md");
    assert!(note.is_file() && about.is_file());

    // The first "forget that" (the core's capture 1) fails in the store.
    std::fs::set_permissions(&about, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    sink.forget(at(30), ForgetScope::Last);
    sink.flush().await;
    std::fs::set_permissions(&about, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    // The second "forget that": the core's capture 0, the note.
    sink.forget(at(40), ForgetScope::Last);
    sink.flush().await;

    let kept = std::fs::read_to_string(&about).unwrap_or_default();
    close(store, task).await;
    assert!(!note.is_file(), "the second \"forget that\" undid the note");
    assert!(
        kept.contains("I prefer Go"),
        "the memory whose undo failed is still kept: {kept}"
    );
}

/// A keep is a capture of its own, carrying its proposal's id: "forget that" right after "yes,
/// keep it" takes the memory back out of the file, and the proposal waits in the digest again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forget_that_after_yes_keep_it_undoes_the_keep() {
    let folder = TempFolder::new("sink-forget-keep");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let sink = sink(&store, &test);
    assert!(sink.start_session().await.session.is_some());
    sink.remember(
        at(10),
        &memory(MemoryKind::Stated, MemoryAction::Keep, "I prefer Go", 0),
    );
    sink.remember(
        at(20),
        &memory(
            MemoryKind::Inferred,
            MemoryAction::Propose,
            "Likes diagrams first",
            1,
        ),
    );
    sink.accept_proposal(at(30), CaptureId(1));
    sink.flush().await;
    let about = test.vault().join("memory/about-you.md");
    let kept = std::fs::read_to_string(&about).unwrap_or_default();
    sink.forget(at(40), ForgetScope::Last);
    sink.flush().await;
    let after = std::fs::read_to_string(&about).unwrap_or_default();
    let finished = store
        .finish_session(at(50), None, None, CancelToken::new())
        .await
        .expect("finished");
    close(store, task).await;
    assert!(kept.contains("Likes diagrams first"), "kept: {kept}");
    assert!(
        !after.contains("Likes diagrams first") && after.contains("I prefer Go"),
        "the keep is undone, the memory before it stays: {after}"
    );
    let boardai_runtime::store::SessionFinished::Written { session, .. } = finished else {
        panic!("written: {finished:?}");
    };
    let digest = test.read(&format!("{}/digest.md", session.folder));
    assert!(
        digest.contains("- [ ] About you: Likes diagrams first"),
        "the proposal waits again: {digest}"
    );
}

/// Finding 25: "don't remember this session" behind a full write queue is never dropped, and it
/// goes before the writes still queued (I10). The writer can't run until the test awaits, so
/// the queue really is full when it is said.
#[tokio::test]
async fn dont_remember_this_session_is_never_dropped_at_a_full_queue() {
    let folder = TempFolder::new("sink-forget-full-queue");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let before = snapshot(&test.vault_root());
    let sink = sink(&store, &test);
    assert!(sink.start_session().await.session.is_some());
    store.log_ui(
        at(10),
        vec![caption(Speaker::User, "Remember the launch plan")],
    );
    let record = memory(
        MemoryKind::SessionRecord,
        MemoryAction::Keep,
        "Launch plan",
        0,
    );
    for capture in 0..=u32::try_from(STORE_WRITES_CAPACITY).expect("fits") {
        sink.remember(
            at(20),
            &MemoryWrite {
                capture: CaptureId(capture),
                ..record.clone()
            },
        );
    }
    sink.forget(at(30), ForgetScope::Session);
    sink.finish_session(SessionClose::Digest {
        t_end: at(60_000),
        board: None,
        deadline: Duration::from_secs(1),
        cancel: CancelToken::new(),
    })
    .await;
    close(store, task).await;
    assert_eq!(
        snapshot(&test.vault_root()),
        before,
        "I10: nothing of the session is left"
    );
}

/// Finding 25: quitting before the writer reached "don't remember this session" (the session's
/// end ran past its bound) must not let the store's shutdown give the session a digest.
#[tokio::test]
async fn a_forgotten_session_gets_no_digest_at_shutdown() {
    let folder = TempFolder::new("sink-forget-shutdown");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let before = snapshot(&test.vault_root());
    let sink = sink(&store, &test);
    assert!(sink.start_session().await.session.is_some());
    store.log_ui(
        at(10),
        vec![caption(Speaker::User, "Let's plan the Orvelline migration")],
    );
    store.flush().await.expect("flushed");
    // No await between the forget and the shutdown: the writer has not run yet.
    sink.forget(at(30), ForgetScope::Session);
    close(store, task).await;
    assert_eq!(
        snapshot(&test.vault_root()),
        before,
        "I10: the forgotten session got no digest and left nothing"
    );
}
