//! A [`StoreSink`] that keeps this session's memories in memory: the text dev loop and tests
//! use it, and the app falls back to it when the vault can't be used. It writes nothing to
//! disk, so nothing outlives the process; `recall` and the views show what was remembered in
//! this session, and no past session exists.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use boardai_core::effect::{ForgetScope, LogRecord};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::{MemoryKind, MemoryWrite, Subject};
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolFailure, ToolOutput};
use boardai_core::views::{MemoryRow, TaskRow, View};
use futures_util::future::BoxFuture;

use super::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
use crate::limits::{MEMORY_STORE_MEMORIES_MAX, MEMORY_STORE_RECALL_HITS_MAX};
use crate::store::{SystemWallClock, WallClock};

#[derive(Debug)]
pub struct MemoryStore {
    // Never held across an await: every method locks, copies and returns.
    memories: Mutex<VecDeque<MemoryWrite>>,
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
            memories: Mutex::new(VecDeque::new()),
            clock,
        }
    }

    /// A copy of what is remembered, oldest first.
    fn held(&self) -> Option<Vec<MemoryWrite>> {
        self.memories
            .lock()
            .ok()
            .map(|memories| memories.iter().cloned().collect())
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
                    matches!(memory.kind, MemoryKind::Stated | MemoryKind::LearningSignal)
                        && memory.subject == subject
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
    fn input(&self, _at: SessionTime, _input: &Input) {}

    fn ui_event(&self, _at: SessionTime, _event: &UiEvent) {}

    fn log(&self, _at: SessionTime, _record: &LogRecord) {}

    fn remember(&self, _at: SessionTime, memory: &MemoryWrite) {
        if let Ok(mut memories) = self.memories.lock() {
            if memories.len() >= MEMORY_STORE_MEMORIES_MAX {
                memories.pop_front();
            }
            memories.push_back(memory.clone());
        }
    }

    fn forget(&self, _at: SessionTime, scope: ForgetScope) {
        if let Ok(mut memories) = self.memories.lock() {
            match scope {
                ForgetScope::Last => {
                    memories.pop_back();
                }
                ForgetScope::Session => memories.clear(),
            }
        }
    }

    /// Nothing here is ever only proposed for long: the session's memory dies with it.
    fn accept_proposal(&self, _at: SessionTime, _capture: CaptureId) {}

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
        if let Ok(mut memories) = self.memories.lock() {
            memories.clear();
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
        let held = store.memories.lock().map_or(1, |memories| memories.len());
        assert_eq!(held, 0);
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

    #[test]
    fn the_oldest_memory_goes_past_the_limit() {
        let store = MemoryStore::new();
        for index in 0..=MEMORY_STORE_MEMORIES_MAX {
            store.remember(SessionTime::ZERO, &memory(&format!("fact {index}")));
        }
        let held = store.memories.lock().map_or(0, |memories| memories.len());
        assert_eq!(held, MEMORY_STORE_MEMORIES_MAX);
    }
}
