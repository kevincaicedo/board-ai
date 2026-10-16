//! Your files stay yours (D-042, LLD §3.6): the store edits the memory files, task lists and
//! digests a line at a time and never rewrites a byte you wrote, even through redaction; what
//! it adds is redacted, and file names come from redacted text, so a secret never names a file
//! (I8). Forgetting a session still finds and removes every file it created (I10).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::store::{Remembered, SessionFinished};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, close, contains, keep, recall_text, snapshot, task, write,
};
use support::TempFolder;

/// D-042: appending a memory leaves your own line, secret and all, exactly as you wrote it,
/// and forgetting the session restores the file byte for byte (I10).
#[tokio::test]
async fn a_memory_write_leaves_your_own_lines_as_you_wrote_them() {
    let folder = TempFolder::new("own-files-hand-written");
    let test = TestStore::new(&folder.path);
    let facts = test.vault().join("memory/facts.md");
    std::fs::create_dir_all(facts.parent().expect("parent")).expect("mkdir");
    let hand = "# Facts\n\n- staging db password=Hunter2Hunter2 ^m40\n";
    std::fs::write(&facts, hand).expect("hand-written facts");
    let (store, task_handle, _) = test.open().await;
    let before = snapshot(&test.vault_root());
    store.start_session(None).await.expect("starts");
    keep(
        &store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "We use Kafka",
    )
    .await;
    let after_write = test.read("memory/facts.md");
    let recalled = recall_text(store.recall("staging password").await);
    store.forget_session().await.expect("forgets");
    let same = snapshot(&test.vault_root()) == before;
    close(store, task_handle).await;
    assert!(
        after_write.starts_with(hand) && after_write.contains("- We use Kafka ^m41"),
        "your line is kept as written: {after_write}"
    );
    assert!(
        !recalled.contains("Hunter2Hunter2"),
        "the index never holds it: {recalled}"
    );
    assert!(same, "I10: the vault is exactly as it was");
}

/// I10: a task list the session created is found and deleted on forget, even when the
/// project's name is redacted on its way into the file's header.
#[tokio::test]
async fn forgetting_removes_a_task_list_named_by_redacted_text() {
    let folder = TempFolder::new("own-files-redacted-header");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    let before = snapshot(&test.vault_root());
    store.start_session(None).await.expect("starts");
    let added = store
        .remember(task("Rotate it", Some("Secret: S3-bucket-policy"), &[]))
        .await
        .expect("task");
    let Remembered::Task { path, .. } = added else {
        panic!("a task: {added:?}");
    };
    let list = test.read(&path);
    store.forget_session().await.expect("forgets");
    let after = snapshot(&test.vault_root());
    close(store, task_handle).await;
    assert!(
        !list.contains("S3-bucket-policy") && list.contains("[redacted:"),
        "the header is redacted: {list}"
    );
    let left: Vec<&String> = after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .collect();
    assert!(
        left.is_empty(),
        "I10: nothing of the session is left: {left:?}"
    );
}

/// D-042: keeping an earlier session's proposal ticks its box and nothing else in the digest,
/// which you may have edited by hand (here: a secret pasted under the summary).
#[tokio::test]
async fn keeping_a_past_proposal_ticks_one_box() {
    let folder = TempFolder::new("own-files-tick");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    keep(
        &store,
        MemoryKind::Inferred,
        MemoryAction::Propose,
        "Uses Redis for sessions",
    )
    .await;
    let finished = store
        .finish_session(store_support::at(10), None, None, CancelToken::new())
        .await
        .expect("finished");
    let SessionFinished::Written { session, .. } = finished else {
        panic!("written: {finished:?}");
    };
    let digest = format!("{}/digest.md", session.folder);
    let written = test.read(&digest);
    let edited = written.replace(
        "## Summary\n",
        "## Summary\n\nMy note: api_key = 9f8e7d6c5b4a39281706hand\n",
    );
    std::fs::write(test.vault().join(&digest), &edited).expect("edit by hand");
    test.clock.advance_ms(60_000);
    store.start_session(None).await.expect("a second session");
    store.accept_proposal("p1").await.expect("kept");
    let ticked = test.read(&digest);
    close(store, task_handle).await;
    assert_eq!(
        ticked,
        edited.replace("- [ ] About your systems:", "- [x] About your systems:"),
        "one character changed"
    );
}

/// I8: a note's file name, the index and `recall` never carry a secret from its first line.
#[tokio::test]
async fn a_secret_never_names_a_file() {
    let folder = TempFolder::new("own-files-names");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    let token = "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
    let remembered = store
        .remember(write(
            MemoryKind::Note,
            MemoryAction::OnRequest,
            &format!("{token} is the CI token"),
        ))
        .await
        .expect("note");
    store
        .remember(task(
            "Rotate the key",
            Some(&format!("Deploy with {token}")),
            &[],
        ))
        .await
        .expect("task");
    let recall = recall_text(store.recall("CI token").await);
    let scribe = FakeScribe::new(ScribeAnswer::Fails);
    let title_hint = format!("my token is {token}");
    store.flush().await.expect("flush");
    let finished = store
        .finish_session(
            store_support::at(10),
            Some(&scribe as &dyn boardai_runtime::llm::LlmProvider),
            None,
            CancelToken::new(),
        )
        .await
        .expect("finished");
    test.clock.advance_ms(60_000);
    store.start_session(Some(title_hint)).await.expect("starts");
    store
        .finish_session(store_support::at(10), None, None, CancelToken::new())
        .await
        .expect("finished");
    close(store, task_handle).await;
    let lowered = token.to_lowercase().replace('_', "-");
    let names: Vec<String> = snapshot(&test.vault_root()).into_keys().collect();
    let mut index = std::fs::read(test.index_path()).unwrap_or_default();
    index.extend(std::fs::read(test.index_path().with_extension("sqlite-wal")).unwrap_or_default());
    let Remembered::Note { path, .. } = remembered else {
        panic!("a note");
    };
    assert!(matches!(finished, SessionFinished::Written { .. }));
    assert!(
        !names.iter().any(|name| name.contains(&lowered[4..])),
        "a vault name holds the token: {names:?}"
    );
    assert!(!path.contains(&lowered[4..]), "{path}");
    assert!(!contains(&index, &lowered[4..]), "the index holds it");
    assert!(!recall.contains(&lowered[4..]), "{recall}");
}
