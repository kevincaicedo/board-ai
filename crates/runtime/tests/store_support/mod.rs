//! Store test support: a store in a temporary folder on a fake wall clock, a scripted scribe,
//! vault snapshots, and helpers to build log entries. Never the real `~/BoardAI`.
#![allow(
    dead_code,
    reason = "each test binary uses a different part of the support module"
)]
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::geometry::Rect;
use boardai_core::history::{Conversation, Message, UserPart};
use boardai_core::ids::{CaptureId, ElementId, LayerId, OpId};
use boardai_core::input::StopReason;
use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
use boardai_core::protocol::{DrawOp, NodeShape, Speaker, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::limits::{EVENT_LOG_FLUSH_MS, STORE_INBOX_CAPACITY};
use boardai_runtime::llm::{
    ChatEvent, ChatStream, LlmCapabilities, LlmProvider, ProviderError, Usage,
};
use boardai_runtime::store::{
    FakeWallClock, MemoryFileName, Remembered, Store, StoreConfig, StoreOpened, StoreTask, WallTime,
};
use futures_util::stream;

/// 2026-10-15T14:32:10Z.
pub const START_MS: i64 = 1_792_074_730_000;

pub struct TestStore {
    pub root: PathBuf,
    pub clock: FakeWallClock,
}

impl TestStore {
    /// A store rooted in `root` (vault, index and a pretend home folder inside it).
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            clock: FakeWallClock::at(WallTime::from_unix_ms(START_MS)),
        }
    }

    pub fn vault(&self) -> PathBuf {
        self.root.join("vault")
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join("app-data/index.sqlite")
    }

    pub fn config(&self) -> StoreConfig {
        StoreConfig {
            vault: self.vault(),
            home: Some(self.root.join("home")),
            index_path: self.index_path(),
            clock: Arc::new(self.clock.clone()),
            inbox_capacity: STORE_INBOX_CAPACITY,
            flush_interval: Duration::from_millis(EVENT_LOG_FLUSH_MS),
        }
    }

    pub async fn open(&self) -> (Store, StoreTask, StoreOpened) {
        Store::open(self.config()).await.expect("the store opens")
    }

    /// The canonical vault path (the temp folder may sit behind a symlink).
    pub fn vault_root(&self) -> PathBuf {
        self.vault().canonicalize().expect("vault exists")
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.vault().join(relative)).expect("vault file exists")
    }
}

pub async fn close(store: Store, task: StoreTask) {
    store.shutdown().await.expect("shutdown");
    task.join().await.expect("the actor ends");
}

/// Every file and folder under `root`, with file contents. Folders map to `None`.
pub fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).expect("readable").flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.insert(relative, None);
                pending.push(path);
            } else {
                out.insert(relative, Some(std::fs::read(&path).expect("readable file")));
            }
        }
    }
    out
}

/// Every byte under `root`, for "this secret is nowhere" checks.
pub fn all_bytes(root: &Path) -> Vec<u8> {
    snapshot(root).into_values().flatten().flatten().collect()
}

pub fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// A write about your systems (the core sends who a memory is about; see [`about_you`]).
pub fn write(kind: MemoryKind, action: MemoryAction, text: &str) -> MemoryWrite {
    MemoryWrite {
        kind,
        action,
        text: text.to_owned(),
        links: Vec::new(),
        project: None,
        subject: Subject::Systems,
        capture: CaptureId(0),
    }
}

/// A write about you.
pub fn about_you(kind: MemoryKind, action: MemoryAction, text: &str) -> MemoryWrite {
    MemoryWrite {
        subject: Subject::You,
        ..write(kind, action, text)
    }
}

pub fn task(text: &str, project: Option<&str>, links: &[&str]) -> MemoryWrite {
    MemoryWrite {
        kind: MemoryKind::Task,
        action: MemoryAction::OnRequest,
        text: text.to_owned(),
        links: links.iter().map(|link| (*link).to_owned()).collect(),
        project: project.map(str::to_owned),
        subject: Subject::You,
        capture: CaptureId(0),
    }
}

pub fn caption(who: Speaker, text: &str) -> UiEvent {
    UiEvent::Caption {
        who,
        text: text.to_owned(),
        is_final: true,
    }
}

pub fn node(layer: u8, id: &str, label: &str) -> UiEvent {
    UiEvent::OpStart {
        layer: LayerId(layer),
        op: DrawOp::Node {
            id: ElementId::parse(id).expect("valid id"),
            op: OpId(1),
            shape: NodeShape::Service,
            label: label.to_owned(),
            rect: Rect {
                x: 0,
                y: 0,
                w: 160,
                h: 64,
            },
            stack: 1,
            fill: None,
            link: None,
        },
        budget_ms: 600,
    }
}

pub fn at(ms: u64) -> SessionTime {
    SessionTime::from_ms(ms)
}

/// A scribe that answers from a script and records the prompts it was sent.
pub struct FakeScribe {
    pub answer: ScribeAnswer,
    pub prompts: Mutex<Vec<String>>,
}

pub enum ScribeAnswer {
    Text(String),
    Fails,
    Hangs,
    Refuses,
}

impl FakeScribe {
    pub fn new(answer: ScribeAnswer) -> Self {
        Self {
            answer,
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().expect("lock").clone()
    }
}

impl LlmProvider for FakeScribe {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: false,
            tools: false,
            server_side_fallback: false,
        }
    }

    fn stream(&self, conversation: &Conversation, _cancel: CancelToken) -> ChatStream {
        for message in conversation.messages() {
            if let Message::User { parts } = message {
                for part in parts {
                    if let UserPart::Text { text } = part {
                        self.prompts.lock().expect("lock").push(text.clone());
                    }
                }
            }
        }
        let done = |stop| ChatEvent::Done {
            stop,
            usage: Usage::default(),
            raw: None,
        };
        match &self.answer {
            ScribeAnswer::Text(text) => Box::pin(stream::iter(vec![
                Ok(ChatEvent::TextDelta(text.clone())),
                Ok(done(StopReason::EndTurn)),
            ])),
            ScribeAnswer::Refuses => Box::pin(stream::iter(vec![
                Ok(ChatEvent::TextDelta("{\"title\":".to_owned())),
                Ok(done(StopReason::Refusal)),
            ])),
            ScribeAnswer::Fails => Box::pin(stream::iter(vec![Err(ProviderError::Network {
                detail: "connection reset".to_owned(),
            })])),
            ScribeAnswer::Hangs => Box::pin(stream::pending()),
        }
    }
}

/// A scribe answer in the asked-for shape.
pub fn good_answer(title: &str) -> String {
    serde_json::json!({
        "title": title,
        "summary": "We traced a request from the client through the CDN and the load balancer to the API. Redis caches user lookups. We stopped at retries.",
        "diagrams": ["Request path"],
        "open_questions": ["Why does the CDN cache 404s?"],
        "proposed_memories": [
            {"about": "you", "text": "Likes a slower pace on networking"},
            {"about": "systems", "text": "The user API uses cache-aside with Redis"}
        ],
        "entities": [
            {"name": "Redis", "kind": "system"},
            {"name": "Load balancer", "kind": "concept"}
        ],
        "relations": [
            {"subject": "API", "relation": "depends_on", "object": "Redis"},
            {"subject": "you", "relation": "struggled_with", "object": "Load balancer"}
        ]
    })
    .to_string()
}

/// Remembers something about your systems and expects it kept.
pub async fn keep(store: &Store, kind: MemoryKind, action: MemoryAction, text: &str) -> Remembered {
    store
        .remember(write(kind, action, text))
        .await
        .expect("remembered")
}

/// Remembers something about you and expects it kept.
pub async fn keep_you(
    store: &Store,
    kind: MemoryKind,
    action: MemoryAction,
    text: &str,
) -> Remembered {
    store
        .remember(about_you(kind, action, text))
        .await
        .expect("remembered")
}

/// The memory file a write landed in, if it was a memory.
pub fn memory_file(remembered: &Remembered) -> Option<MemoryFileName> {
    if let Remembered::Memory { file, .. } = remembered {
        Some(*file)
    } else {
        None
    }
}

/// A recall answer's text; a failed recall fails the test.
pub fn recall_text(output: ToolOutput) -> String {
    let ToolOutput::Text { text } = output else {
        panic!("recall failed: {output:?}");
    };
    text
}
