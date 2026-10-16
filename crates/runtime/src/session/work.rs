//! Tool calls and harness tasks the orchestrator runs for the core (LLD §3.1, §3.3, §3.5).
//!
//! Repo tools run on the blocking pool, `recall` and `open_session` go to the store, and harness
//! tasks run through the [`HarnessPool`]. Each is a task in one `JoinSet` that the orchestrator
//! reaps; closing the session aborts them all, and dropping a harness stream kills its process.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessTask};
use boardai_core::ids::{TaskId, ToolCallId};
use boardai_core::input::Input;
use boardai_core::tools::{ToolCall, ToolFailure, ToolOutput};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::task::{Id, JoinSet};

use super::inbox::Inbox;
use super::ports::{StoreRead, StoreSink};
use crate::cancel::CancelToken;
use crate::harness::HarnessPool;
use crate::limits::{SESSION_HARNESS_TASKS_MAX, SESSION_TOOL_TASKS_MAX, SESSION_TOOL_TIMEOUT_MS};
use crate::tools::RepoTools;

/// What a running task is, so its bookkeeping is released when it ends, however it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Running {
    Tool,
    Harness(TaskId),
}

pub(crate) struct Work {
    tools: RepoTools,
    harness: Option<HarnessPool>,
    tasks: JoinSet<()>,
    running: BTreeMap<Id, Running>,
    harness_running: BTreeMap<TaskId, CancelToken>,
}

impl Work {
    pub(crate) fn new(tools: RepoTools, harness: Option<HarnessPool>) -> Self {
        Self {
            tools,
            harness,
            tasks: JoinSet::new(),
            running: BTreeMap::new(),
            harness_running: BTreeMap::new(),
        }
    }

    fn tools_running(&self) -> usize {
        self.running
            .values()
            .filter(|running| **running == Running::Tool)
            .count()
    }

    /// Runs a tool call; returns the result at once when it can't run.
    pub(crate) fn run_tool(
        &mut self,
        id: ToolCallId,
        call: ToolCall,
        store: &Arc<dyn StoreSink>,
        inbox: &Inbox,
    ) -> Option<Input> {
        if self.tools_running() >= SESSION_TOOL_TASKS_MAX {
            let message = "too many tool calls are running; try again in a moment";
            return Some(tool_failed(id, ToolFailure::Unavailable, message));
        }
        let output: BoxFuture<'static, ToolOutput> = match &call {
            ToolCall::ReadFile { .. } | ToolCall::SearchCode { .. } | ToolCall::ListDir { .. } => {
                let tools = self.tools.clone();
                Box::pin(async move { tools.run(&call).await })
            }
            ToolCall::Recall { .. }
            | ToolCall::OpenSession { .. }
            | ToolCall::ShowLibrary
            | ToolCall::ShowTasks { .. }
            | ToolCall::ShowMemory
            | ToolCall::PlaySession { .. } => match StoreRead::of(&call) {
                Some(read) => store.read(read),
                None => return Some(not_run_here(id)),
            },
            // The core answers these itself and never sends them as `RunTool`.
            ToolCall::Research { .. }
            | ToolCall::Review { .. }
            | ToolCall::ProposeChange { .. }
            | ToolCall::Remember { .. }
            | ToolCall::AddTask { .. } => return Some(not_run_here(id)),
        };
        let inbox = inbox.clone();
        let bound = Duration::from_millis(SESSION_TOOL_TIMEOUT_MS);
        let handle = self.tasks.spawn(async move {
            let output = tokio::time::timeout(bound, output)
                .await
                .unwrap_or_else(|_elapsed| ToolOutput::Failed {
                    kind: ToolFailure::Unavailable,
                    message: format!("timed out after {SESSION_TOOL_TIMEOUT_MS} ms"),
                });
            // A closed session needs no answer.
            let _ = inbox.send(Input::ToolResult { id, output }).await;
        });
        self.running.insert(handle.id(), Running::Tool);
        None
    }

    /// Starts a harness task; returns its terminal event at once when it can't start.
    pub(crate) fn run_harness(
        &mut self,
        task: TaskId,
        spec: HarnessTask,
        inbox: &Inbox,
    ) -> Option<Input> {
        let Some(pool) = &self.harness else {
            let detail = "no coding harness is set up for this session".to_owned();
            return Some(harness_failed(task, HarnessFailure::NotInstalled, detail));
        };
        if self.harness_running.len() >= SESSION_HARNESS_TASKS_MAX {
            let detail = "too many harness tasks are running".to_owned();
            return Some(harness_failed(task, HarnessFailure::QueueFull, detail));
        }
        let cancel = CancelToken::new();
        let mut events = pool.run(spec, cancel.clone());
        self.harness_running.insert(task, cancel);
        let inbox = inbox.clone();
        let handle = self.tasks.spawn(async move {
            while let Some(event) = events.next().await {
                if inbox.send(Input::Harness { task, event }).await.is_err() {
                    break;
                }
            }
        });
        self.running.insert(handle.id(), Running::Harness(task));
        None
    }

    /// Asks a harness task to stop; it ends with its own `Failed { Cancelled }` event.
    pub(crate) fn cancel_harness(&self, task: TaskId) {
        if let Some(cancel) = self.harness_running.get(&task) {
            cancel.cancel();
        }
    }

    /// Waits for the next task to end and releases it; `None` when nothing runs.
    pub(crate) async fn reap(&mut self) -> Option<()> {
        let id = match self.tasks.join_next_with_id().await? {
            Ok((id, ())) => id,
            Err(error) => {
                tracing::error!(%error, "a session task ended abnormally");
                error.id()
            }
        };
        if let Some(Running::Harness(task)) = self.running.remove(&id) {
            self.harness_running.remove(&task);
        }
        Some(())
    }

    /// Stops everything: harness tasks are asked to stop, then every task is aborted (dropping
    /// a harness stream kills its process group).
    pub(crate) async fn shutdown(&mut self) {
        for cancel in self.harness_running.values() {
            cancel.cancel();
        }
        self.tasks.shutdown().await;
        self.harness_running.clear();
        self.running.clear();
    }
}

fn not_run_here(id: ToolCallId) -> Input {
    let message = "this tool is not run by the runtime";
    tool_failed(id, ToolFailure::InvalidInput, message)
}

fn tool_failed(id: ToolCallId, kind: ToolFailure, message: &str) -> Input {
    Input::ToolResult {
        id,
        output: ToolOutput::Failed {
            kind,
            message: message.to_owned(),
        },
    }
}

fn harness_failed(task: TaskId, reason: HarnessFailure, detail: String) -> Input {
    Input::Harness {
        task,
        event: HarnessEvent::Failed { reason, detail },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use boardai_core::effect::{ForgetScope, LogRecord};
    use boardai_core::ids::{CaptureId, ToolCallId};
    use boardai_core::input::Input;
    use boardai_core::memory::MemoryWrite;
    use boardai_core::protocol::UiEvent;
    use boardai_core::time::SessionTime;
    use boardai_core::tools::{ToolCall, ToolFailure, ToolOutput};
    use futures_util::future::BoxFuture;

    use super::Work;
    use crate::limits::SESSION_TOOL_TIMEOUT_MS;
    use crate::session::SessionChannels;
    use crate::session::inbox::Envelope;
    use crate::session::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
    use crate::tools::RepoTools;

    /// A store whose reads never answer, like a read stuck on a FIFO or a hung volume.
    struct StuckReads;

    impl StoreSink for StuckReads {
        fn report_to(&self, _link: crate::session::StoreLink) {}
        fn input(&self, _at: SessionTime, _input: &Input) {}
        fn ui_event(&self, _at: SessionTime, _event: &UiEvent) {}
        fn log(&self, _at: SessionTime, _record: &LogRecord) {}
        fn remember(&self, _at: SessionTime, _memory: &MemoryWrite) {}
        fn forget(&self, _at: SessionTime, _scope: ForgetScope) {}
        fn accept_proposal(&self, _at: SessionTime, _capture: CaptureId) {}
        fn read(&self, _read: StoreRead) -> BoxFuture<'static, ToolOutput> {
            Box::pin(std::future::pending())
        }
        fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
            Box::pin(std::future::ready(SessionOpened::default()))
        }
        fn finish_session(&self, _close: SessionClose) -> BoxFuture<'static, ()> {
            Box::pin(std::future::ready(()))
        }
        fn flush(&self) -> BoxFuture<'static, ()> {
            Box::pin(std::future::ready(()))
        }
    }

    /// Review (low): a read that never returns held the tool round, and so the turn, forever.
    #[tokio::test(start_paused = true)]
    async fn a_tool_that_never_answers_is_answered_with_a_timeout() {
        let mut channels = SessionChannels::new();
        let mut work = Work::new(RepoTools::default(), None);
        let store: Arc<dyn StoreSink> = Arc::new(StuckReads);
        let Some(id) = ToolCallId::parse("toolu_0") else {
            panic!("a valid id");
        };
        let call = ToolCall::Recall {
            query: "kafka".to_owned(),
        };
        assert!(work.run_tool(id, call, &store, &channels.inbox).is_none());
        let wait = Duration::from_millis(SESSION_TOOL_TIMEOUT_MS + 1_000);
        let answer = tokio::time::timeout(wait, channels.receiver.recv())
            .await
            .ok()
            .flatten();
        assert!(
            matches!(
                answer,
                Some(Envelope::Input(Input::ToolResult {
                    output: ToolOutput::Failed {
                        kind: ToolFailure::Unavailable,
                        ..
                    },
                    ..
                }))
            ),
            "{answer:?}"
        );
        work.shutdown().await;
    }
}
