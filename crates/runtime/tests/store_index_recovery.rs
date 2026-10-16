//! The index stays usable and honest whatever the vault holds (LLD §3.6): a session folder
//! renamed by hand is still indexed, one unreadable file is skipped and reported instead of
//! stopping the store, a rebuild that fails keeps the index it started from, a forgotten
//! session leaves no word in the index's files (I10), and a hand-made note can't put a value
//! in a view that the UI rejects.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::Speaker;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::store::{SessionFinished, Store};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, at, caption, close, contains, good_answer, keep,
    recall_text,
};
use support::TempFolder;

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

/// Makes a file unreadable until the guard drops (a permission, as an I/O error would).
struct Locked<'a>(&'a Path);

impl<'a> Locked<'a> {
    fn new(path: &'a Path) -> Self {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        Self(path)
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o644));
    }
}

/// A session folder renamed by hand to a name that sorts before its old one stays indexed,
/// at this open and the next.
#[tokio::test]
async fn a_session_folder_renamed_by_hand_stays_indexed() {
    let folder = TempFolder::new("index-renamed");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    store.log_ui(
        at(10),
        vec![caption(Speaker::User, "How does the load balancer pick?")],
    );
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Request path")));
    let session_folder = finish(&store, Some(&scribe)).await;
    close(store, task_handle).await;
    std::fs::rename(
        test.vault().join(&session_folder),
        test.vault().join("sessions/2026-10-15-a-renamed-by-hand"),
    )
    .expect("rename by hand");
    let (store, task_handle, _) = test.open().await;
    let first = store.library().await.expect("library").sessions;
    close(store, task_handle).await;
    let (store, task_handle, _) = test.open().await;
    let second = store.library().await.expect("library").sessions.len();
    let recall = recall_text(store.recall("load balancer").await);
    close(store, task_handle).await;
    assert_eq!(
        (first.len(), second),
        (1, 1),
        "the renamed session is still listed"
    );
    assert_eq!(first[0].title, "Request path");
    assert!(recall.contains("Request path"), "{recall}");
}

/// One unreadable note never stops the store: it opens, the note is skipped and reported, and
/// it is indexed once it can be read.
#[tokio::test]
async fn an_unreadable_note_is_skipped_and_reported() {
    let folder = TempFolder::new("index-unreadable");
    let test = TestStore::new(&folder.path);
    folder.write(
        "vault/notes/locked.md",
        b"# Locked away\n\nZephyrine notes.\n",
    );
    folder.write("vault/notes/open.md", b"# Open book\n\nReadable notes.\n");
    let note = test.vault().join("notes/locked.md");
    let opened = {
        let _locked = Locked::new(&note);
        Store::open(test.config()).await
    };
    let Ok((store, task_handle, opened)) = opened else {
        panic!("the store did not open: {opened:?}");
    };
    let open_book = recall_text(store.recall("open book").await);
    close(store, task_handle).await;
    assert_eq!(opened.scan.skipped, 1, "{:?}", opened.scan);
    assert!(open_book.contains("Open book"), "{open_book}");
    let (store, task_handle, reopened) = test.open().await;
    let locked = recall_text(store.recall("zephyrine").await);
    close(store, task_handle).await;
    assert_eq!(reopened.scan.skipped, 0, "{:?}", reopened.scan);
    assert!(
        locked.contains("Locked away"),
        "read once it can be: {locked}"
    );
}

/// A rebuild with an unreadable file still lists every session it could read.
#[tokio::test]
async fn a_rebuild_with_an_unreadable_file_keeps_the_library() {
    let folder = TempFolder::new("index-rebuild");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    store.log_ui(
        at(10),
        vec![caption(Speaker::User, "How does Raft elect a leader?")],
    );
    finish(&store, None).await;
    let note = folder.write("vault/notes/locked.md", b"# locked\n");
    let rebuilt = {
        let _locked = Locked::new(&note);
        store.rebuild_index().await
    };
    let sessions = store.library().await.expect("library").sessions.len();
    close(store, task_handle).await;
    let report = rebuilt.expect("the rebuild finishes");
    assert_eq!(report.skipped, 1, "{report:?}");
    assert_eq!(sessions, 1, "the library still lists the session");
}

/// I10: after "don't remember this session", none of its words are left anywhere in the
/// index's files (the database, its write-ahead log and shared memory).
#[tokio::test]
async fn a_forgotten_session_leaves_no_words_in_the_index_files() {
    let folder = TempFolder::new("index-scrubbed");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    store.log_ui(
        at(10),
        vec![caption(
            Speaker::User,
            "Tell me about Pellucidarquux deployments",
        )],
    );
    keep(
        &store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "Our project is codenamed Zyqvarnoth",
    )
    .await;
    keep(
        &store,
        MemoryKind::Note,
        MemoryAction::OnRequest,
        "Quorvantel rollout checklist",
    )
    .await;
    store.end_session(at(20)).await.expect("ends");
    store.forget_session().await.expect("forgets");
    let mut bytes = Vec::new();
    let data = test
        .index_path()
        .parent()
        .expect("data folder")
        .to_path_buf();
    for entry in std::fs::read_dir(&data).expect("data folder").flatten() {
        bytes.extend(std::fs::read(entry.path()).unwrap_or_default());
    }
    close(store, task_handle).await;
    let found: Vec<&str> = [
        "Zyqvarnoth",
        "zyqvarnoth",
        "Pellucidarquux",
        "pellucidarquux",
        "Quorvantel",
        "quorvantel",
    ]
    .into_iter()
    .filter(|word| contains(&bytes, word))
    .collect();
    assert!(found.is_empty(), "still in the index files: {found:?}");
}

/// A hand-made note with a date in another form, a blank id and a session link that isn't
/// one reaches the views as values the UI reads (`YYYY-MM-DD` or empty, a non-blank id, no
/// link), so it can't make the whole view unreadable.
#[tokio::test]
async fn a_hand_made_note_reaches_the_views_in_the_shape_the_ui_reads() {
    let folder = TempFolder::new("index-hand-made-note");
    let test = TestStore::new(&folder.path);
    folder.write(
        "vault/notes/handmade.md",
        b"---\nid: \"\"\ncreated: 15/10/2026\nsession: not a session\n---\n# Kafka\n\nKeyed.\n",
    );
    folder.write(
        "vault/projects/harness.md",
        b"# Harness\n\n- [ ] Check TTLs [[session:]] ^t9\n",
    );
    let (store, task_handle, _) = test.open().await;
    let notes = store.recent_notes().await.expect("notes");
    let tasks = store.now_list(None).await.expect("now").tasks;
    close(store, task_handle).await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].date, "", "a date the UI rejects becomes no date");
    assert_eq!(notes[0].id, "notes/handmade.md", "a blank id is the path");
    assert_eq!(notes[0].from, None, "not a session id: no link");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].session, None, "an empty session link: no link");
}
