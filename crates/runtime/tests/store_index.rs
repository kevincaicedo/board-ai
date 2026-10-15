//! The index is derived from the vault: deleting it and rebuilding gives the same answers, a
//! hand edit re-indexes only its file, and an unusable index file is rebuilt.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use boardai_core::ids::LayerId;
use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::{DrawOp, Speaker, UiEvent};
use boardai_core::tools::ToolOutput;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::limits::RECALL_HITS_MAX;
use boardai_runtime::store::{
    IndexOpened, Library, NowList, Remembered, SchemaState, SessionFinished, SessionView, Store,
    StoreError,
};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, at, caption, close, good_answer, keep, node, recall_text,
    task, write,
};
use support::TempFolder;

const BOARD_EXPORT: &str = r#"{"type":"excalidraw","version":2,"elements":[{"id":"lb","type":"rectangle","text":"Load balancer"}]}"#;
const QUERIES: [&str; 5] = [
    "redis",
    "kafka",
    "load balancer pod",
    "jitter retries",
    "readiness",
];

fn text(output: ToolOutput) -> String {
    recall_text(output)
}

/// A session that drew a board with a detour, captured a task and a fact, and got a scribe
/// digest. Returns its id.
async fn load_balancing_session(store: &Store) -> String {
    store.start_session(None).await.expect("starts");
    let detour = LayerId(1);
    store.log_ui(
        at(100),
        vec![caption(
            Speaker::User,
            "How does the load balancer pick a pod?",
        )],
    );
    store.log_ui(
        at(900),
        vec![node(0, "lb", "Load balancer"), node(0, "api", "API")],
    );
    store.log_ui(
        at(1_000),
        vec![UiEvent::LayerPush {
            layer: detour,
            title: "readiness".into(),
        }],
    );
    store.log_ui(at(1_100), vec![node(1, "probe", "Readiness probe")]);
    store.log_ui(at(1_200), vec![UiEvent::LayerPop { layer: detour }]);
    store.log_ui(
        at(1_300),
        vec![caption(Speaker::Assistant, "It checks readiness first.")],
    );
    store
        .remember(task("Check pod readiness", Some("Harness"), &[]))
        .await
        .expect("task");
    keep(
        store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "We use Kafka for events",
    )
    .await;
    let scribe = FakeScribe::new(ScribeAnswer::Text(good_answer("Load balancing")));
    let finished = store
        .finish_session(
            at(5_000),
            Some(&scribe),
            Some(BOARD_EXPORT.into()),
            CancelToken::new(),
        )
        .await
        .expect("finished");
    let SessionFinished::Written { session, .. } = finished else {
        panic!("written: {finished:?}");
    };
    session.id.to_string()
}

/// Two finished sessions, a day apart, with notes, tasks and memories. Returns the first id.
async fn populate(test: &TestStore, store: &Store) -> String {
    let first = load_balancing_session(store).await;
    test.clock.advance_ms(86_400_000);
    store
        .start_session(Some("Retries".into()))
        .await
        .expect("starts");
    store.log_ui(
        at(50),
        vec![caption(Speaker::User, "Explain retries with jitter")],
    );
    keep(
        store,
        MemoryKind::Note,
        MemoryAction::OnRequest,
        "Jitter spreads retries over time",
    )
    .await;
    store
        .remember(task("Add jitter to the client", None, &["client.rs:12"]))
        .await
        .expect("task");
    store
        .finish_session(at(2_000), None, None, CancelToken::new())
        .await
        .expect("finished");
    first
}

#[derive(Debug, PartialEq)]
struct Answers {
    library: Library,
    now: NowList,
    harness: NowList,
    recalls: Vec<String>,
    recap: Option<String>,
    opened: SessionView,
}

async fn answers(store: &Store, first_id: &str) -> Answers {
    let mut recalls = Vec::new();
    for query in QUERIES {
        recalls.push(text(store.recall(query).await));
    }
    Answers {
        library: store.library().await.expect("library"),
        now: store.now_list(None).await.expect("now"),
        harness: store.now_list(Some("harness".into())).await.expect("now"),
        recalls,
        recap: store.recap().await.expect("recap"),
        opened: store.open_session(first_id).await.expect("opens"),
    }
}

fn remove_index(test: &TestStore) {
    for suffix in ["", "-wal", "-shm"] {
        let mut path = test.index_path().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(path);
    }
}

#[tokio::test]
async fn deleting_the_index_and_rebuilding_gives_the_same_answers() {
    let folder = TempFolder::new("store-index-rebuild");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, opened) = test.open().await;
    assert_eq!(opened.index, IndexOpened::Schema(SchemaState::Created));
    let first_id = populate(&test, &store).await;
    let before = answers(&store, &first_id).await;
    assert_eq!(before.library.sessions.len(), 2);
    assert_eq!(before.now.tasks.len(), 2);
    assert_eq!(before.harness.tasks.len(), 1);
    let jitter = &before.recalls[3];
    assert!(
        jitter.contains("[note]") && jitter.contains("[task]") && jitter.contains("[transcript]"),
        "every kind of row is in the answers: {jitter}"
    );
    assert!(
        before
            .recalls
            .iter()
            .all(|answer| answer.starts_with("Found")),
        "{:?}",
        before.recalls
    );

    let report = store.rebuild_index().await.expect("rebuilds");
    assert!(report.reindexed > 0);
    assert_eq!(
        answers(&store, &first_id).await,
        before,
        "a rebuild in place"
    );
    close(store, task_handle).await;

    remove_index(&test);
    let (store, task_handle, opened) = test.open().await;
    assert_eq!(opened.index, IndexOpened::Schema(SchemaState::Created));
    assert!(opened.scan.reindexed >= 6, "{:?}", opened.scan);
    assert_eq!(
        answers(&store, &first_id).await,
        before,
        "a rebuild from nothing"
    );
    close(store, task_handle).await;

    let (store, task_handle, opened) = test.open().await;
    assert_eq!(opened.index, IndexOpened::Schema(SchemaState::Current));
    assert_eq!(opened.scan.reindexed, 0, "nothing changed");
    close(store, task_handle).await;
}

#[tokio::test]
async fn the_library_and_open_session_bring_a_past_board_back() {
    let folder = TempFolder::new("store-index-library");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let first_id = populate(&test, &store).await;
    let library = store.library().await.expect("library");
    let titles: Vec<&str> = library
        .sessions
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Retries", "Load balancing"], "newest first");
    assert_eq!(library.sessions[1].date, "2026-10-15");
    assert_eq!(library.sessions[1].duration_ms, 5_000);
    assert_eq!(
        library.sessions[1].cover.as_deref(),
        Some("sessions/2026-10-15-load-balancing/board.excalidraw")
    );
    assert_eq!(library.sessions[0].cover, None);

    let opened = store.open_session(&first_id).await.expect("by id");
    let labels: Vec<String> = opened
        .board
        .iter()
        .filter_map(|event| {
            if let UiEvent::OpStart {
                op: DrawOp::Node { label, .. },
                ..
            } = event
            {
                Some(label.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        labels,
        vec!["Load balancer", "API"],
        "the closed detour is not on the board"
    );
    assert!(opened.recap.starts_with("We traced a request"));
    assert!(
        opened
            .events_path
            .ends_with("sessions/2026-10-15-load-balancing/events.jsonl")
    );
    let by_words = store
        .open_session("the load balancing session")
        .await
        .expect("by words");
    assert_eq!(by_words.id, opened.id);
    assert!(matches!(
        store.open_session("quantum knitting").await,
        Err(StoreError::NotFound { .. })
    ));

    let recap = store
        .recap()
        .await
        .expect("recap")
        .expect("a digest exists");
    assert!(
        recap.starts_with("Last time (2026-10-16), \"Retries\": You asked:"),
        "{recap}"
    );
    close(store, task_handle).await;
}

#[tokio::test]
async fn a_hand_edit_re_indexes_that_file_only() {
    let folder = TempFolder::new("store-index-hand-edit");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    populate(&test, &store).await;
    close(store, task_handle).await;

    let facts = test.vault().join("memory/facts.md");
    let mut content = std::fs::read_to_string(&facts).expect("facts");
    content.push_str("- We run Postgres 16 ^m9\n");
    std::fs::write(&facts, content).expect("hand edit");
    let note = std::fs::read_dir(test.vault().join("notes"))
        .expect("notes")
        .flatten()
        .next()
        .expect("a note")
        .path();
    let file = std::fs::File::options()
        .write(true)
        .open(&note)
        .expect("note");
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
        .expect("touch");

    let (store, task_handle, opened) = test.open().await;
    assert_eq!(opened.scan.reindexed, 1, "{:?}", opened.scan);
    assert!(text(store.recall("postgres").await).contains("«Postgres»"));
    store.start_session(None).await.expect("starts");
    let kept = store
        .remember(write(
            MemoryKind::Stated,
            MemoryAction::Keep,
            "We shard by tenant",
        ))
        .await
        .expect("kept");
    assert!(
        matches!(kept, Remembered::Memory { id, .. } if id.to_string() == "m10"),
        "ids continue after hand-made ones: {kept:?}"
    );
    close(store, task_handle).await;

    std::fs::remove_file(&note).expect("delete by hand");
    let (store, task_handle, opened) = test.open().await;
    assert_eq!(opened.scan.removed, 1);
    assert!(text(store.recall("spreads").await).starts_with("Nothing"));
    close(store, task_handle).await;
}

#[tokio::test]
async fn an_unusable_index_file_is_rebuilt_from_the_vault() {
    let folder = TempFolder::new("store-index-unusable");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    populate(&test, &store).await;
    close(store, task_handle).await;

    remove_index(&test);
    std::fs::write(
        test.index_path(),
        b"this is not a database at all, just bytes",
    )
    .expect("junk");
    let (store, task_handle, opened) = test.open().await;
    assert!(
        matches!(opened.index, IndexOpened::Recreated { .. }),
        "{:?}",
        opened.index
    );
    assert!(text(store.recall("kafka").await).contains("«Kafka»"));
    close(store, task_handle).await;

    let connection = rusqlite::Connection::open(test.index_path()).expect("open");
    connection
        .pragma_update(None, "user_version", 99)
        .expect("future version");
    drop(connection);
    let (store, task_handle, opened) = test.open().await;
    assert!(
        matches!(&opened.index, IndexOpened::Recreated { reason } if reason.contains("newer")),
        "{:?}",
        opened.index
    );
    assert_eq!(store.library().await.expect("library").sessions.len(), 2);
    close(store, task_handle).await;
}

#[tokio::test]
async fn recall_is_bounded_and_says_how_much_more_matched() {
    let folder = TempFolder::new("store-index-bounded");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    for number in 0..(RECALL_HITS_MAX + 5) {
        store
            .remember(write(
                MemoryKind::Note,
                MemoryAction::OnRequest,
                &format!("Raft note {number}: leaders replicate the log"),
            ))
            .await
            .expect("note");
    }
    let answer = text(store.recall("raft leaders").await);
    assert_eq!(
        answer
            .lines()
            .filter(|line| line.contains("[note]"))
            .count(),
        RECALL_HITS_MAX
    );
    assert!(answer.ends_with("(5 more not shown)\n"), "{answer}");
    let nothing = text(store.recall("what did we say?").await);
    assert!(nothing.starts_with("Nothing to search for"), "{nothing}");
    close(store, task_handle).await;
}

#[tokio::test]
async fn a_hand_made_session_folder_is_listed_by_its_folder_name() {
    let folder = TempFolder::new("store-index-hand-made");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    close(store, task_handle).await;
    let session = test.vault().join("sessions/my-own-notes");
    std::fs::create_dir_all(&session).expect("folder");
    std::fs::write(
        session.join("digest.md"),
        "# Notes I wrote\n\n## Summary\n\nAbout gossip protocols.\n",
    )
    .expect("digest");
    let (store, task_handle, _) = test.open().await;
    let library = store.library().await.expect("library");
    assert_eq!(library.sessions.len(), 1);
    assert_eq!(library.sessions[0].id.as_str(), "my-own-notes");
    assert_eq!(library.sessions[0].title, "Notes I wrote");
    assert!(text(store.recall("gossip").await).contains("«gossip»"));
    close(store, task_handle).await;
}
