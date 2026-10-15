//! A [`StoreSink`] that keeps this session's memories in memory, until the store lands. It
//! writes nothing to disk, so nothing outlives the process; `recall` searches what was
//! remembered in this session and `open_session` finds no past session.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;

use boardai_core::effect::LogRecord;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolFailure, ToolOutput};
use futures_util::future::BoxFuture;

use super::ports::StoreSink;
use crate::limits::{MEMORY_STORE_MEMORIES_MAX, MEMORY_STORE_RECALL_HITS_MAX};

#[derive(Debug, Default)]
pub struct MemoryStore {
    // Never held across an await: every method locks, copies and returns.
    memories: Mutex<VecDeque<MemoryWrite>>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn search(&self, query: &str) -> ToolOutput {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|word| word.len() > 2)
            .collect();
        let Ok(memories) = self.memories.lock() else {
            return ToolOutput::Failed {
                kind: ToolFailure::Unavailable,
                message: "memory is unavailable".to_owned(),
            };
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

    fn recall(&self, query: String) -> BoxFuture<'static, ToolOutput> {
        let output = self.search(&query);
        Box::pin(std::future::ready(output))
    }

    fn open_session(&self, _id: String) -> BoxFuture<'static, ToolOutput> {
        let output = ToolOutput::Failed {
            kind: ToolFailure::NotFound,
            message: "past sessions are not stored yet".to_owned(),
        };
        Box::pin(std::future::ready(output))
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        Box::pin(std::future::ready(()))
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite};
    use boardai_core::time::SessionTime;
    use boardai_core::tools::ToolOutput;

    use super::MemoryStore;
    use crate::limits::MEMORY_STORE_MEMORIES_MAX;
    use crate::session::ports::StoreSink;

    fn memory(text: &str) -> MemoryWrite {
        MemoryWrite {
            kind: MemoryKind::Stated,
            action: MemoryAction::Keep,
            text: text.to_owned(),
            links: Vec::new(),
            project: None,
        }
    }

    #[tokio::test]
    async fn recall_finds_what_this_session_remembered() {
        let store = MemoryStore::new();
        store.remember(SessionTime::ZERO, &memory("We use Kafka for events"));
        store.remember(SessionTime::ZERO, &memory("Postgres is the main store"));
        let ToolOutput::Text { text } = store.recall("what about kafka".into()).await else {
            panic!("text");
        };
        assert!(
            text.contains("Kafka") && !text.contains("Postgres"),
            "{text}"
        );
        let ToolOutput::Text { text } = store.recall("redis".into()).await else {
            panic!("text");
        };
        assert!(text.starts_with("Nothing"), "{text}");
        assert!(matches!(
            store.open_session("2026-10-14".into()).await,
            ToolOutput::Failed { .. }
        ));
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
