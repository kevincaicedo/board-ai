//! I8: a secret never reaches the store. One planted secret per kind of write (event log
//! lines, notes, tasks, memories, proposals, session records, the scribe's digest, the board,
//! a title, and index rows from a file pasted by hand), then every byte of the vault, the index
//! and the scribe's prompt is searched for them.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use boardai_core::ids::ToolCallId;
use boardai_core::input::Input;
use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::Speaker;
use boardai_core::tools::ToolOutput;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::store::{SessionFinished, Store};
use store_support::{
    FakeScribe, ScribeAnswer, TestStore, all_bytes, at, caption, close, contains, keep,
    recall_text, task,
};
use support::TempFolder;

/// (secret, the path it takes, the marker that proves the path redacted it).
const PLANTED: &[(&str, &str, &str)] = &[
    (
        "sk-ant-api03-CAPTIONsecret0123456789abcdef",
        "ui caption",
        "anthropic-key",
    ),
    (
        "ghp_INPUTsecret0123456789abcdefghijklmnopq",
        "input transcript",
        "github-token",
    ),
    (
        "MIIEpAIBAAKCAQEAtoolRESULTsecretPEMbody",
        "tool result",
        "private-key",
    ),
    ("Hunter22!noteSecret", "note", "assigned-secret"),
    (
        concat!("AKIA", "IOSFODNN7TASKXYZ"),
        "task",
        "aws-access-key-id",
    ),
    (
        concat!("xoxb", "-123456789012-MEMORYsecretToken"),
        "memory",
        "slack-token",
    ),
    (
        "sk-or-v1-PROPOSALsecret0123456789abcdef",
        "proposal",
        "openrouter-key",
    ),
    (
        "eyJhbGciOiJIUzI1NiJ9.eyJyZWNvcmQiOiJzZWNyZXQifQ.UkVDT1JEc2lnbmF0dXJl",
        "session record",
        "jwt",
    ),
    (
        "AIzaSyDIGEST0123456789abcdefghijklmnopq",
        "scribe summary",
        "google-api-key",
    ),
    (
        "sk_live_51ENTITYsecretABCDEFGH",
        "scribe entity",
        "stripe-key",
    ),
    (
        "boardOpaque0123456789TOKENvalue",
        "board export",
        "authorization",
    ),
    ("HintPass99", "title hint", "url-password"),
];

const PASTED: &str = "9f8e7d6c5b4a39281706pasted";

fn scribe_answer() -> String {
    serde_json::json!({
        "title": "Secrets everywhere",
        "summary": format!("The user pasted {} by mistake.", PLANTED[8].0),
        "diagrams": [],
        "open_questions": [],
        "proposed_memories": [],
        "entities": [{"name": format!("Stripe {}", PLANTED[9].0), "kind": "system"}],
        "relations": []
    })
    .to_string()
}

/// Secrets in a caption, a transcript and a key file shown on a code card.
fn plant_in_the_log(store: &Store) {
    store.log_ui(
        at(10),
        vec![caption(
            Speaker::User,
            &format!("my key is {}", PLANTED[0].0),
        )],
    );
    let transcript = Input::Transcript {
        text: format!("token {}", PLANTED[1].0),
        is_final: true,
    };
    store.log_input(at(20), transcript);
    let key_file = ToolOutput::File {
        path: "deploy/key.pem".into(),
        first_line: 1,
        lines: vec![
            "-----BEGIN RSA PRIVATE KEY-----".into(),
            PLANTED[2].0.into(),
            "-----END RSA PRIVATE KEY-----".into(),
        ],
        more_lines: 0,
    };
    let id = ToolCallId::parse("toolu_9").expect("id");
    store.log_input(
        at(30),
        Input::ToolResult {
            id,
            output: key_file,
        },
    );
}

/// Secrets in a note, a task, a memory, a proposal and a session record.
async fn plant_in_memories(store: &Store) {
    let note = format!("db config: password={}", PLANTED[3].0);
    keep(store, MemoryKind::Note, MemoryAction::OnRequest, &note).await;
    let rotate = format!("Rotate {}", PLANTED[4].0);
    store
        .remember(task(&rotate, None, &[]))
        .await
        .expect("task");
    let memory = format!("Our Slack bot uses {}", PLANTED[5].0);
    keep(store, MemoryKind::Stated, MemoryAction::Keep, &memory).await;
    let proposal = format!("Uses {} for OpenRouter", PLANTED[6].0);
    keep(
        store,
        MemoryKind::Inferred,
        MemoryAction::Propose,
        &proposal,
    )
    .await;
    let record = format!("Session cookie {}", PLANTED[7].0);
    keep(
        store,
        MemoryKind::SessionRecord,
        MemoryAction::Keep,
        &record,
    )
    .await;
}

/// Ends the session with secrets in the scribe's answer and the board export.
async fn finish_with_secrets(store: &Store, scribe: &FakeScribe) {
    let board = format!(
        r#"{{"elements":[{{"type":"text","text":"Authorization: Bearer {}"}}]}}"#,
        PLANTED[10].0
    );
    let scribe = Some(scribe as &dyn LlmProvider);
    let finished = store
        .finish_session(at(1_000), scribe, Some(board), CancelToken::new())
        .await
        .expect("finished");
    assert!(matches!(finished, SessionFinished::Written { .. }));
}

/// A second session titled by a hint with a password in it, and no scribe.
async fn session_titled_by_a_secret(test: &TestStore, store: &Store) {
    test.clock.advance_ms(60_000);
    let hint = format!(
        "Connecting to postgres://app:{}@db.internal/app",
        PLANTED[11].0
    );
    store.start_session(Some(hint)).await.expect("starts");
    store
        .finish_session(at(500), None, None, CancelToken::new())
        .await
        .expect("finished");
}

#[tokio::test]
async fn a_planted_secret_in_each_kind_of_write_never_reaches_disk() {
    let folder = TempFolder::new("store-redaction");
    let test = TestStore::new(&folder.path);
    let (store, task_handle, _) = test.open().await;
    store.start_session(None).await.expect("starts");
    plant_in_the_log(&store);
    plant_in_memories(&store).await;
    let scribe = FakeScribe::new(ScribeAnswer::Text(scribe_answer()));
    finish_with_secrets(&store, &scribe).await;
    session_titled_by_a_secret(&test, &store).await;

    // A secret you paste into your own note stays in your file, but never reaches the index.
    std::fs::write(
        test.vault().join("notes/pasted.md"),
        format!("Staging access\n\napi_key = {PASTED}\n"),
    )
    .expect("hand-made note");
    store.rebuild_index().await.expect("rebuilds");
    let text = recall_text(store.recall("staging access").await);
    assert!(text.contains("[note]") && !text.contains(PASTED), "{text}");
    close(store, task_handle).await;

    let vault = all_bytes(&test.vault_root());
    let index = all_bytes(&test.root.join("app-data"));
    let prompts = scribe.prompts().concat();
    for (secret, path, marker) in PLANTED {
        assert!(
            !contains(&vault, secret),
            "{path}: the secret reached the vault"
        );
        assert!(
            !contains(&index, secret),
            "{path}: the secret reached the index"
        );
        assert!(
            !prompts.contains(secret),
            "{path}: the secret reached the scribe"
        );
        assert!(
            contains(&vault, &format!("[redacted:{marker}]")),
            "{path}: no [redacted:{marker}] marker, so this path was not exercised"
        );
    }
    assert!(
        contains(&vault, PASTED),
        "your own file is left as you wrote it"
    );
    assert!(!contains(&index, PASTED), "the index never holds it");
}
