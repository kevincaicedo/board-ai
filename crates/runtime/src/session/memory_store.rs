//! A [`StoreSink`] that keeps this session's memories in memory: the text dev loop and tests
//! use it, and the app falls back to it when the vault can't be used. It writes nothing to
//! disk, so nothing outlives the process; `recall` and the views show what was remembered in
//! this session, and no past session exists.
//!
//! It holds the session's captures as the core counts them: every `remember` and every "yes,
//! keep it" is one, so "forget that" undoes the capture the core meant.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use boardai_core::effect::{ForgetScope, LogRecord};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolFailure, ToolOutput};
use boardai_core::views::{MemoryRow, TaskRow, View};
use futures_util::future::BoxFuture;

use super::inbox::StoreLink;
use super::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
use crate::limits::{MEMORY_STORE_MEMORIES_MAX, MEMORY_STORE_RECALL_HITS_MAX};
use crate::store::{SystemWallClock, WallClock};

/// One capture, as the core counts them.
#[derive(Clone, Debug)]
enum Held {
    Memory(MemoryWrite),
    /// "Yes, keep it" for the proposal made as this capture.
    Keep(CaptureId),
}

#[derive(Debug)]
pub struct MemoryStore {
    // Never held across an await: every method locks, copies and returns.
    captures: Mutex<VecDeque<Held>>,
    /// For the views' `today`: the UI refuses a view without one.
    clock: Arc<dyn WallClock>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemWallClock))
    }
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_clock(clock: Arc<dyn WallClock>) -> Self {
        Self {
            captures: Mutex::new(VecDeque::new()),
            clock,
        }
    }

    /// A copy of what is remembered, oldest first: every memory not merely proposed, and the
    /// proposals kept with a yes.
    fn held(&self) -> Option<Vec<MemoryWrite>> {
        let captures = self.captures.lock().ok()?;
        let kept: Vec<CaptureId> = captures
            .iter()
            .filter_map(|held| match held {
                Held::Keep(capture) => Some(*capture),
                Held::Memory(_) => None,
            })
            .collect();
        Some(
            captures
                .iter()
                .filter_map(|held| match held {
                    Held::Memory(memory)
                        if memory.action != MemoryAction::Propose
                            || kept.contains(&memory.capture) =>
                    {
                        Some(memory.clone())
                    }
                    Held::Memory(_) | Held::Keep(_) => None,
                })
                .collect(),
        )
    }

    fn hold(&self, held: Held) {
        if let Ok(mut captures) = self.captures.lock() {
            if captures.len() >= MEMORY_STORE_MEMORIES_MAX {
                captures.pop_front();
            }
            captures.push_back(held);
        }
    }

    fn search(&self, query: &str) -> ToolOutput {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|word| word.len() > 2)
            .collect();
        let Some(memories) = self.held() else {
            return unavailable();
        };
        let hits: Vec<&MemoryWrite> = memories
            .iter()
            .rev()
            .filter(|memory| {
                let text = memory.text.to_lowercase();
                words.iter().any(|word| text.contains(word.as_str()))
            })
            .collect();
        if hits.is_empty() {
            let text = "Nothing in this session's memory matches. Past sessions are not stored \
                        yet."
                .to_owned();
            return ToolOutput::Text { text };
        }
        let mut text = String::new();
        for memory in hits.iter().take(MEMORY_STORE_RECALL_HITS_MAX) {
            let _ = writeln!(text, "- ({:?}) {}", memory.kind, memory.text);
        }
        let more = hits.len().saturating_sub(MEMORY_STORE_RECALL_HITS_MAX);
        if more > 0 {
            let _ = writeln!(text, "({more} more)");
        }
        ToolOutput::Text { text }
    }

    /// The views, from this session alone.
    fn view(&self, read: &StoreRead) -> ToolOutput {
        let Some(memories) = self.held() else {
            return unavailable();
        };
        let rows = |subject: Subject| -> Vec<MemoryRow> {
            memories
                .iter()
                .rev()
                .filter(|memory| {
                    matches!(
                        memory.kind,
                        MemoryKind::Stated | MemoryKind::LearningSignal | MemoryKind::Inferred
                    ) && memory.subject == subject
                })
                .map(|memory| MemoryRow {
                    id: format!("c{}", memory.capture.0),
                    text: memory.text.clone(),
                })
                .collect()
        };
        let view = match read {
            StoreRead::Library => View::Library {
                sessions: Vec::new(),
                more_sessions: 0,
            },
            StoreRead::Tasks { project } => View::Tasks {
                project: project.clone(),
                tasks: memories
                    .iter()
                    .filter(|memory| memory.kind == MemoryKind::Task)
                    .map(|memory| TaskRow {
                        id: format!("c{}", memory.capture.0),
                        title: memory.text.clone(),
                        project: memory.project.clone().unwrap_or_default(),
                        from: None,
                    })
                    .collect(),
                more_tasks: 0,
                notes: Vec::new(),
            },
            StoreRead::Memory => View::Memory {
                about_you: rows(Subject::You),
                systems: rows(Subject::Systems),
                notes: Vec::new(),
                latest: None,
            },
            StoreRead::Recall { .. } | StoreRead::PastSession { .. } | StoreRead::Replay { .. } => {
                return ToolOutput::Failed {
                    kind: ToolFailure::NotFound,
                    message: "past sessions are not stored: memory is off".to_owned(),
                };
            }
        };
        ToolOutput::View {
            view: Box::new(view),
            today: self.clock.now().date(),
        }
    }
}

fn unavailable() -> ToolOutput {
    ToolOutput::Failed {
        kind: ToolFailure::Unavailable,
        message: "memory is unavailable".to_owned(),
    }
}

impl StoreSink for MemoryStore {
    /// Nothing here can fail to be kept: memory is the only place it goes.
    fn report_to(&self, _link: StoreLink) {}

    fn input(&self, _at: SessionTime, _input: &Input) {}

    fn ui_event(&self, _at: SessionTime, _event: &UiEvent) {}

    fn log(&self, _at: SessionTime, _record: &LogRecord) {}

    fn remember(&self, _at: SessionTime, memory: &MemoryWrite) {
        self.hold(Held::Memory(memory.clone()));
    }

    fn forget(&self, _at: SessionTime, scope: ForgetScope) {
        if let Ok(mut captures) = self.captures.lock() {
            match scope {
                ForgetScope::Last => {
                    captures.pop_back();
                }
                ForgetScope::Session => captures.clear(),
            }
        }
    }

    /// A keep is a capture of its own, as in the core: "forget that" right after undoes it.
    fn accept_proposal(&self, _at: SessionTime, capture: CaptureId) {
        self.hold(Held::Keep(capture));
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        let output = match &read {
            StoreRead::Recall { query } => self.search(query),
            StoreRead::PastSession { .. }
            | StoreRead::Library
            | StoreRead::Tasks { .. }
            | StoreRead::Memory
            | StoreRead::Replay { .. } => self.view(&read),
        };
        Box::pin(std::future::ready(output))
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        if let Ok(mut captures) = self.captures.lock() {
            captures.clear();
        }
        Box::pin(std::future::ready(SessionOpened::default()))
    }

    fn finish_session(&self, _close: SessionClose) -> BoxFuture<'static, ()> {
        Box::pin(std::future::ready(()))
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        Box::pin(std::future::ready(()))
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::effect::ForgetScope;
    use boardai_core::ids::CaptureId;
    use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
    use boardai_core::time::SessionTime;
    use boardai_core::tools::ToolOutput;
    use boardai_core::views::View;

    use std::sync::Arc;

    use super::MemoryStore;
    use crate::limits::MEMORY_STORE_MEMORIES_MAX;
    use crate::session::ports::{StoreRead, StoreSink};
    use crate::store::{FakeWallClock, WallTime};

    fn memory(text: &str) -> MemoryWrite {
        MemoryWrite {
            kind: MemoryKind::Stated,
            action: MemoryAction::Keep,
            text: text.to_owned(),
            links: Vec::new(),
            project: None,
            subject: Subject::Systems,
            capture: CaptureId(0),
        }
    }

    fn recall(query: &str) -> StoreRead {
        StoreRead::Recall {
            query: query.to_owned(),
        }
    }

    #[tokio::test]
    async fn recall_finds_what_this_session_remembered() {
        let store = MemoryStore::new();
        store.remember(SessionTime::ZERO, &memory("We use Kafka for events"));
        store.remember(SessionTime::ZERO, &memory("Postgres is the main store"));
        let ToolOutput::Text { text } = store.read(recall("what about kafka")).await else {
            panic!("text");
        };
        assert!(
            text.contains("Kafka") && !text.contains("Postgres"),
            "{text}"
        );
        let ToolOutput::Text { text } = store.read(recall("redis")).await else {
            panic!("text");
        };
        assert!(text.starts_with("Nothing"), "{text}");
        let past = StoreRead::PastSession {
            query: "2026-10-14".into(),
        };
        assert!(matches!(store.read(past).await, ToolOutput::Failed { .. }));
    }

    #[tokio::test]
    async fn forgetting_undoes_the_last_or_everything() {
        let store = MemoryStore::new();
        store.remember(SessionTime::ZERO, &memory("We use Kafka"));
        store.remember(SessionTime::ZERO, &memory("We use Postgres"));
        store.forget(SessionTime::ZERO, ForgetScope::Last);
        let ToolOutput::View { view, .. } = store.read(StoreRead::Memory).await else {
            panic!("a view");
        };
        let View::Memory { systems, .. } = *view else {
            panic!("a memory view");
        };
        let texts: Vec<&str> = systems.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(texts, vec!["We use Kafka"]);
        store.forget(SessionTime::ZERO, ForgetScope::Session);
        let held = store.captures.lock().map_or(1, |captures| captures.len());
        assert_eq!(held, 0);
    }

    /// The core counts a keep as a capture of its own: two "forget that"s after "yes, keep it"
    /// undo the keep, then the proposal, and the memory before them stays.
    #[tokio::test]
    async fn forget_that_undoes_the_capture_the_core_meant() {
        let store = MemoryStore::new();
        let about_you = |kind, action, text: &str, capture| MemoryWrite {
            kind,
            action,
            subject: Subject::You,
            capture: CaptureId(capture),
            ..memory(text)
        };
        let stated = about_you(MemoryKind::Stated, MemoryAction::Keep, "I prefer Go", 0);
        let inferred = about_you(
            MemoryKind::Inferred,
            MemoryAction::Propose,
            "Likes diagrams first",
            1,
        );
        store.remember(SessionTime::ZERO, &stated);
        store.remember(SessionTime::ZERO, &inferred);
        assert_eq!(
            about(&store).await,
            vec!["I prefer Go"],
            "a proposal is not kept"
        );
        store.accept_proposal(SessionTime::ZERO, CaptureId(1));
        assert_eq!(
            about(&store).await,
            vec!["Likes diagrams first", "I prefer Go"]
        );
        store.forget(SessionTime::ZERO, ForgetScope::Last);
        store.forget(SessionTime::ZERO, ForgetScope::Last);
        assert_eq!(about(&store).await, vec!["I prefer Go"]);
    }

    /// Review (low): the views carried an empty `today`, and the UI refuses a `ViewShow`
    /// without a date, so no view ever showed while memory was off.
    #[tokio::test]
    async fn views_carry_today() {
        let clock = FakeWallClock::at(WallTime::from_unix_ms(1_792_022_400_000));
        let store = MemoryStore::with_clock(Arc::new(clock));
        let ToolOutput::View { today, .. } = store.read(StoreRead::Memory).await else {
            panic!("a view");
        };
        assert_eq!(today, "2026-10-15");
    }

    async fn about(store: &MemoryStore) -> Vec<String> {
        let ToolOutput::View { view, .. } = store.read(StoreRead::Memory).await else {
            panic!("a view");
        };
        let View::Memory { about_you, .. } = *view else {
            panic!("a memory view");
        };
        about_you.into_iter().map(|row| row.text).collect()
    }

    #[test]
    fn the_oldest_memory_goes_past_the_limit() {
        let store = MemoryStore::new();
        for index in 0..=MEMORY_STORE_MEMORIES_MAX {
            store.remember(SessionTime::ZERO, &memory(&format!("fact {index}")));
        }
        let held = store.captures.lock().map_or(0, |captures| captures.len());
        assert_eq!(held, MEMORY_STORE_MEMORIES_MAX);
    }
}
