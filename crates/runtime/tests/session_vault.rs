//! Memory by voice end to end, on a temporary vault (LLD §2.8, §3.6, I10): typed speech goes
//! through the real core and orchestrator, the talker is scripted, and the store writes real
//! files. "Forget that" undoes the last capture, "yes, keep it" keeps a proposal, "don't
//! remember this session" leaves the vault exactly as it was, and the views read the vault.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::engine::text::NOT_FORGOTTEN_LINE;
use boardai_core::history::{Conversation, ProviderFamily, RawAssistant};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::memory::{MemoryAction, MemoryKind};
use boardai_core::protocol::{Speaker, UiBatch, UiEvent};
use boardai_core::tools::{ToolCall, ToolInvocation};
use boardai_core::views::View;
use boardai_core::{CoreConfig, MicMode, RepoReads};
use boardai_runtime::bridge::{UiSink, UiSinkClosed};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::TokioClock;
use boardai_runtime::llm::{ChatEvent, ChatStream, LlmCapabilities, LlmProvider, Usage};
use boardai_runtime::session::{
    Observed, Progress, SessionChannels, SessionHandle, SessionParts, SpeechInLink, VaultSink,
    VirtualVoice, VirtualVoiceConfig, say_typed, spawn, wait_for_answer,
};
use boardai_runtime::store::{Store, StoreTask};
use boardai_runtime::tools::RepoTools;
use futures_util::stream;
use store_support::{TestStore, at, caption, close, keep, node, snapshot, task};
use support::TempFolder;
use tokio::sync::watch;

/// The talker: one scripted reply per request, "Okay." when the script runs out.
#[derive(Default)]
struct Replies {
    replies: Mutex<VecDeque<Vec<ChatEvent>>>,
    calls: Mutex<u32>,
}

impl Replies {
    fn says(&self, text: &str) {
        self.push(text, &[], StopReason::EndTurn);
    }

    fn calls(&self, text: &str, tools: &[(&str, &str)]) {
        self.push(text, tools, StopReason::ToolUse);
    }

    fn push(&self, text: &str, tools: &[(&str, &str)], stop: StopReason) {
        let mut events = vec![ChatEvent::TextDelta(text.to_owned())];
        for (name, input) in tools {
            let mut calls = self.calls.lock().expect("lock");
            *calls += 1;
            let id = ToolCallId::parse(&format!("call_{calls}")).expect("valid id");
            events.push(ChatEvent::ToolCall(ToolInvocation {
                id,
                name: (*name).to_owned(),
                input_json: (*input).to_owned(),
                call: ToolCall::parse(name, input),
            }));
        }
        events.push(done(stop, text));
        self.replies.lock().expect("lock").push_back(events);
    }
}

fn done(stop: StopReason, text: &str) -> ChatEvent {
    ChatEvent::Done {
        stop,
        usage: Usage::default(),
        raw: Some(RawAssistant {
            provider: ProviderFamily::OpenAiCompatible,
            content_json: serde_json::json!({ "content": text }).to_string(),
        }),
    }
}

impl LlmProvider for Replies {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: true,
            tools: true,
            server_side_fallback: false,
        }
    }

    fn stream(&self, _conversation: &Conversation, _cancel: CancelToken) -> ChatStream {
        let reply = self.replies.lock().expect("lock").pop_front();
        let reply = reply.unwrap_or_else(|| {
            vec![
                ChatEvent::TextDelta("Okay.".to_owned()),
                done(StopReason::EndTurn, "Okay."),
            ]
        });
        Box::pin(stream::iter(reply.into_iter().map(Ok)))
    }
}

/// The UI: every event, in order.
#[derive(Default)]
struct Screen {
    events: Mutex<Vec<UiEvent>>,
}

impl Screen {
    fn views(&self) -> Vec<View> {
        self.events
            .lock()
            .expect("lock")
            .iter()
            .filter_map(|event| {
                if let UiEvent::ViewShow { view, .. } = event {
                    Some(view.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

impl UiSink for Screen {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        self.events.lock().expect("lock").extend(batch.events);
        Ok(())
    }
}

struct Running {
    session: SessionHandle,
    link: SpeechInLink,
    progress: watch::Receiver<Progress>,
    screen: Arc<Screen>,
}

fn start_session(store: &Store, test: &TestStore, talker: Arc<Replies>) -> Running {
    let sink = VaultSink::new(store.clone(), None, Arc::new(test.clock.clone()));
    let (observed, progress) = Observed::new(sink);
    let channels = SessionChannels::new();
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::with_speed(8.0));
    let screen = Arc::new(Screen::default());
    let parts = SessionParts {
        core: CoreConfig {
            talker_name: "the model".to_owned(),
            mic: MicMode::OpenMic,
            session_context: None,
            suggestion: None,
            repo_reads: RepoReads::NotOffered,
        },
        clock: TokioClock::start(),
        talker,
        tools: RepoTools::default(),
        harness: None,
        speech: Box::new(voice),
        store: Arc::new(observed),
        ui: Arc::clone(&screen) as Arc<dyn UiSink>,
        sources: boardai_runtime::session::SourceParts::in_memory(std::path::PathBuf::from("/")),
    };
    let session = spawn(channels, parts);
    let link = session.speech_in();
    Running {
        session,
        link,
        progress,
        screen,
    }
}

impl Running {
    /// Says a line and waits until BoardAI has answered it.
    async fn say(&mut self, text: &str) {
        let before = *self.progress.borrow();
        say_typed(&self.link, text).await.expect("the session runs");
        wait_for_answer(&mut self.progress, before).await;
    }
}

/// Waits (at most 5 s) for a vault condition.
async fn eventually(what: &str, done: impl Fn() -> bool) {
    for _ in 0..250 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}

async fn open(folder: &TempFolder) -> (TestStore, Store, StoreTask) {
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    (test, store, task)
}

const NOTE: &str = r#"{"kind":"note","text":"Cache-aside reads Redis first","about":"systems"}"#;
const STATED: &str = r#"{"kind":"stated","text":"I prefer Go examples","about":"you"}"#;
const INFERRED: &str = r#"{"kind":"inferred","text":"Likes diagrams first","about":"you"}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forget_that_and_yes_keep_it_change_the_vault() {
    let folder = TempFolder::new("session-vault-memory");
    let (test, store, store_task) = open(&folder).await;
    let talker = Arc::new(Replies::default());
    talker.calls("Noted.", &[("remember", NOTE)]);
    talker.says("Anything else?");
    talker.calls("Got it.", &[("remember", STATED)]);
    talker.says("Go it is.");
    talker.calls("Want me to remember that?", &[("remember", INFERRED)]);
    talker.says("Want me to remember that?");
    let mut running = start_session(&store, &test, talker);
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");

    running.say("note that cache-aside reads Redis first").await;
    eventually("the note is written", || note.is_file()).await;
    running.say("forget that").await;
    eventually("the note is removed", || !note.is_file()).await;

    running.say("I prefer Go examples").await;
    eventually("the stated memory is about you", || {
        std::fs::read_to_string(test.vault().join("memory/about-you.md"))
            .is_ok_and(|text| text.contains("I prefer Go examples"))
    })
    .await;
    running.say("draw it first please").await;
    running.say("yes, keep it").await;
    eventually("the proposal is kept", || {
        std::fs::read_to_string(test.vault().join("memory/about-you.md"))
            .is_ok_and(|text| text.contains("Likes diagrams first"))
    })
    .await;
    assert!(
        !test.vault().join("memory/facts.md").exists(),
        "the subject, not the words, picked the file"
    );
    let said = running.screen.events.lock().expect("lock").iter().any(|event| {
        matches!(event, UiEvent::Caption { who: Speaker::Assistant, text, is_final: true } if text == "Forgotten.")
    });
    assert!(said, "\"Forgotten.\" was said");

    running.session.close().await;
    let digests: Vec<String> = std::fs::read_dir(test.vault().join("sessions"))
        .expect("sessions")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("digest.md")).ok())
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "the session got its digest when it closed"
    );
    assert!(
        digests[0].contains("I prefer Go examples"),
        "{}",
        digests[0]
    );
    assert!(
        !digests[0].contains("Cache-aside reads Redis first"),
        "the forgotten note is not in the digest: {}",
        digests[0]
    );
    close(store, store_task).await;
}

/// Finding 13 end to end: a "forget that" the store can't carry out is shown after
/// "Forgotten.", and the next "forget that" still undoes the capture before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forget_that_the_store_cannot_carry_out_is_shown() {
    use std::os::unix::fs::PermissionsExt;
    let folder = TempFolder::new("session-vault-failed-forget");
    let (test, store, store_task) = open(&folder).await;
    let talker = Arc::new(Replies::default());
    talker.calls("Noted.", &[("remember", NOTE)]);
    talker.says("Anything else?");
    talker.calls("Got it.", &[("remember", STATED)]);
    talker.says("Go it is.");
    let mut running = start_session(&store, &test, talker);
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");
    let about = test.vault().join("memory/about-you.md");
    running.say("note that cache-aside reads Redis first").await;
    running.say("I prefer Go examples").await;
    eventually("both are written", || note.is_file() && about.is_file()).await;

    let mode = |bits| std::fs::set_permissions(&about, std::fs::Permissions::from_mode(bits));
    mode(0o000).expect("lock about-you.md");
    running.say("forget that").await;
    let screen = Arc::clone(&running.screen);
    eventually("the failure is shown", || {
        screen.events.lock().expect("lock").iter().any(|event| {
            matches!(event, UiEvent::Problem { spoken, detail }
                if spoken == NOT_FORGOTTEN_LINE && detail.contains("memory/about-you.md"))
        })
    })
    .await;
    mode(0o644).expect("unlock about-you.md");
    running.say("forget that").await;
    eventually("the note is removed", || !note.is_file()).await;
    let kept = std::fs::read_to_string(&about).expect("about-you.md");
    running.session.close().await;
    close(store, store_task).await;
    assert!(kept.contains("I prefer Go examples"), "{kept}");
}

/// I10 end to end: everything the session wrote is undone, and nothing more is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dont_remember_this_session_leaves_the_vault_as_it_was() {
    let folder = TempFolder::new("session-vault-off-record");
    let (test, store, store_task) = open(&folder).await;
    // An earlier session left a fact and a task behind; they must survive.
    store.start_session(None).await.expect("starts");
    keep(
        &store,
        MemoryKind::Stated,
        MemoryAction::Keep,
        "We use Kafka",
    )
    .await;
    store
        .remember(task("Check Redis TTLs", Some("Harness"), &[]))
        .await
        .expect("task");
    store
        .finish_session(at(10_000), None, None, CancelToken::new())
        .await
        .expect("finished");
    let before = snapshot(&test.vault());

    let talker = Arc::new(Replies::default());
    talker.calls(
        "Noted.",
        &[
            ("remember", STATED),
            (
                "add_task",
                r#"{"title":"Spike Tauri audio","project":"Harness"}"#,
            ),
        ],
    );
    talker.says("Done.");
    talker.calls("Sure.", &[("remember", NOTE)]);
    talker.says("Okay.");
    let mut running = start_session(&store, &test, talker);
    running
        .say("I prefer Go examples, and add a task to spike Tauri audio")
        .await;
    eventually("the task is written", || {
        std::fs::read_to_string(test.vault().join("projects/harness.md"))
            .is_ok_and(|text| text.contains("Spike Tauri audio"))
    })
    .await;
    running.say("don't remember this session").await;
    running.say("note that cache-aside reads Redis first").await;
    running.session.close().await;
    close(store, store_task).await;
    assert_eq!(
        snapshot(&test.vault()),
        before,
        "I10: nothing of the session is left"
    );
}

/// A finished session about Raft: a question, two nodes, a task and a one-node board export.
async fn seed_raft(store: &Store) {
    let title = Some("Raft log compaction".to_owned());
    store.start_session(title).await.expect("starts");
    store.log_ui(
        at(10),
        vec![
            caption(Speaker::User, "How does Raft compact its log?"),
            node(0, "leader", "leader"),
            node(0, "follower", "follower"),
        ],
    );
    let read = task("Read Raft section 5.3", Some("Harness"), &[]);
    store.remember(read).await.expect("task");
    let board = r#"{"type":"excalidraw","version":2,"elements":[
        {"id":"l0:leader","type":"rectangle","x":120,"y":250,"width":150,"height":64,"strokeStyle":"solid"},
        {"id":"l0:leader#label","type":"text","x":150,"y":270,"width":60,"height":20,"strokeStyle":"solid"}]}"#;
    let finished =
        store.finish_session(at(60_000), None, Some(board.to_owned()), CancelToken::new());
    finished.await.expect("finished");
}

/// The views read the vault: a finished session shows in the library with its cover, its
/// task in the Now list, its board when opened, and a replay by its title.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_views_show_what_the_vault_holds() {
    let folder = TempFolder::new("session-vault-views");
    let (test, store, store_task) = open(&folder).await;
    seed_raft(&store).await;
    let talker = Arc::new(Replies::default());
    talker.calls("Here they are.", &[("show_library", "{}")]);
    talker.says("Raft is the last one.");
    talker.calls("Here.", &[("show_tasks", r#"{"project":"harness"}"#)]);
    talker.says("One thing.");
    talker.calls("Here it is.", &[("open_session", r#"{"id":"raft"}"#)]);
    talker.says("That's the board.");
    talker.calls("Playing it.", &[("play_session", r#"{"id":"raft"}"#)]);
    talker.says("It's playing.");
    let mut running = start_session(&store, &test, talker);
    running.say("show me my sessions").await;
    running.say("what's next on the harness").await;
    running.say("open the Raft session").await;
    running.say("play the Raft one").await;
    let views = running.screen.views();
    assert_eq!(views.len(), 4, "{views:#?}");
    let View::Library { sessions, .. } = &views[0] else {
        panic!("the library first: {views:#?}");
    };
    assert_eq!(sessions.len(), 1, "the live session is not listed yet");
    assert_eq!(sessions[0].title, "Raft log compaction");
    assert_eq!(
        sessions[0].cover.len(),
        1,
        "the board's one node is the cover"
    );
    let View::Tasks { tasks, .. } = &views[1] else {
        panic!("the Now list: {views:#?}");
    };
    assert_eq!(tasks[0].title, "Read Raft section 5.3");
    assert_eq!(
        tasks[0].from.as_ref().map(|from| from.title.as_str()),
        Some("Raft log compaction")
    );
    let View::PastSession { layers, .. } = &views[2] else {
        panic!("the past board: {views:#?}");
    };
    assert_eq!(layers[0].ops.len(), 2, "both nodes come back");
    assert!(
        matches!(&views[3], View::Replay { session } if session.title == "Raft log compaction")
    );
    let log = store
        .session_log(&boardai_runtime::store::SessionId::parse(&sessions[0].id).expect("an id"))
        .await
        .expect("the log replays");
    assert!(log.lines().count() >= 4, "{log}");
    running.session.close().await;
    close(store, store_task).await;
}

const SECOND_NOTE: &str =
    r#"{"kind":"note","text":"The leader replicates entries in order","about":"systems"}"#;
const SECOND_TASK: &str = r#"{"title":"Read Raft section 5.4","project":"Harness"}"#;

/// The owner's report: notes said by voice were kept in the first session, never in the next.
/// A settings change ends the session the way quitting does and starts a new one on the same
/// store; its notes and tasks must reach the vault like the first session's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_are_kept_in_the_session_after_a_restart() {
    let folder = TempFolder::new("session-vault-restart");
    let (test, store, store_task) = open(&folder).await;
    let first = Arc::new(Replies::default());
    first.calls("Noted.", &[("remember", NOTE)]);
    first.says("Anything else?");
    let mut running = start_session(&store, &test, first);
    running.say("note that cache-aside reads Redis first").await;
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");
    eventually("the first session's note is written", || note.is_file()).await;
    running.session.close().await;

    let second = Arc::new(Replies::default());
    second.calls(
        "Noted.",
        &[("remember", SECOND_NOTE), ("add_task", SECOND_TASK)],
    );
    second.says("Done.");
    let mut running = start_session(&store, &test, second);
    running
        .say("note that the leader replicates entries in order, and add a task")
        .await;
    let note = test
        .vault()
        .join("notes/the-leader-replicates-entries-in-order.md");
    eventually("the second session's note is written", || note.is_file()).await;
    eventually("the second session's task is written", || {
        std::fs::read_to_string(test.vault().join("projects/harness.md"))
            .is_ok_and(|text| text.contains("Read Raft section 5.4"))
    })
    .await;
    running.session.close().await;
    close(store, store_task).await;
}

/// The same across a relaunch: the store is shut down and opened again on the same vault.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_are_kept_in_the_session_after_a_relaunch() {
    let folder = TempFolder::new("session-vault-relaunch");
    let (test, store, store_task) = open(&folder).await;
    let first = Arc::new(Replies::default());
    first.calls("Noted.", &[("remember", NOTE)]);
    first.says("Anything else?");
    let mut running = start_session(&store, &test, first);
    running.say("note that cache-aside reads Redis first").await;
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");
    eventually("the first session's note is written", || note.is_file()).await;
    running.session.close().await;
    close(store, store_task).await;

    let (store, store_task, _) = test.open().await;
    let second = Arc::new(Replies::default());
    second.calls("Noted.", &[("remember", SECOND_NOTE)]);
    second.says("Done.");
    let mut running = start_session(&store, &test, second);
    running
        .say("note that the leader replicates entries in order")
        .await;
    let note = test
        .vault()
        .join("notes/the-leader-replicates-entries-in-order.md");
    eventually("the second session's note is written", || note.is_file()).await;
    running.session.close().await;
    close(store, store_task).await;
}

/// The same after an idle rest: the session ends after `SESSION_IDLE_TIMEOUT_MS`, and the next
/// words open a new one with a fresh core, on the same orchestrator and store.
#[tokio::test(start_paused = true)]
async fn notes_are_kept_in_the_session_after_an_idle_rest() {
    let folder = TempFolder::new("session-vault-idle");
    let (test, store, store_task) = open(&folder).await;
    let talker = Arc::new(Replies::default());
    talker.calls("Noted.", &[("remember", NOTE)]);
    talker.says("Anything else?");
    talker.calls("Noted.", &[("remember", SECOND_NOTE)]);
    talker.says("Done.");
    let mut running = start_session(&store, &test, talker);
    running.say("note that cache-aside reads Redis first").await;
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");
    eventually("the first session's note is written", || note.is_file()).await;
    let idle = boardai_runtime::limits::SESSION_IDLE_TIMEOUT_MS + 1_000;
    tokio::time::sleep(Duration::from_millis(idle)).await;

    running
        .say("note that the leader replicates entries in order")
        .await;
    let note = test
        .vault()
        .join("notes/the-leader-replicates-entries-in-order.md");
    eventually("the second session's note is written", || note.is_file()).await;
    running.session.close().await;
    close(store, store_task).await;
}

/// The owner's report as it happened (session log of 2026-10-16): the first session's note was
/// a tool call and was kept; in the next session gemma4 wrote every call as a tag, so the note
/// was dropped while BoardAI said it was made. A note written as a tag is kept (D-143).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_written_as_a_tag_in_the_next_session_is_kept() {
    let folder = TempFolder::new("session-vault-written-note");
    let (test, store, store_task) = open(&folder).await;
    let first = Arc::new(Replies::default());
    first.calls("Noted.", &[("remember", NOTE)]);
    first.says("Anything else?");
    let mut running = start_session(&store, &test, first);
    running.say("note that cache-aside reads Redis first").await;
    let note = test.vault().join("notes/cache-aside-reads-redis-first.md");
    eventually("the first session's note is written", || note.is_file()).await;
    running.session.close().await;

    let second = Arc::new(Replies::default());
    second.says(
        "I can create that note for you. <remember kind=\"note\" text=\"story consensus \
         protocol\" about=\"you\"/> I have made a note for you about the story consensus \
         protocol.",
    );
    let mut running = start_session(&store, &test, second);
    running
        .say("Let's create one note, reminding me to review the consensus protocol.")
        .await;
    let note = test.vault().join("notes/story-consensus-protocol.md");
    eventually("the note written as a tag is kept", || note.is_file()).await;
    running.session.close().await;
    close(store, store_task).await;
}
