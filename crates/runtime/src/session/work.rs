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
use boardai_core::sources::{SourceId, SourceInput, SourceState};
use boardai_core::tools::{ToolCall, ToolFailure, ToolOutput};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::task::{Id, JoinSet};

use super::inbox::{Envelope, Inbox};
use super::orchestrator::SourceMessage;
use super::ports::{StoreRead, StoreSink};
use crate::cancel::CancelToken;
use crate::harness::{HarnessPool, Workspace};
use crate::limits::{
    SESSION_HARNESS_TASKS_MAX, SESSION_TOOL_TASKS_MAX, SESSION_TOOL_TIMEOUT_MS,
    SOURCE_JOB_TIMEOUT_MS, SOURCE_JOBS_MAX, SOURCE_READERS_RUNNING_MAX,
};
use crate::sources::{ReaderEvent, ReaderStream};
use crate::tools::RepoTools;

/// What a running task is, so its bookkeeping is released when it ends, however it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Running {
    Tool,
    Harness(TaskId),
    /// A source job: a resolve, a project's load, a stamp, a citation check (D-184).
    Source,
    /// A document reader on one source (the `SourceReader` seam).
    Reader(SourceId),
}

/// Whether a document reader can start on a source now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderRoom {
    Free,
    /// As many run as may (`SOURCE_READERS_RUNNING_MAX`): the document waits its turn.
    Full,
    /// One already reads that source.
    Reading,
}

/// What a task that ended was, so the orchestrator can start what waited for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reaped {
    /// A document reader: another document may start reading.
    Reader,
    Other,
}

pub(crate) struct Work {
    tools: RepoTools,
    harness: Option<HarnessPool>,
    tasks: JoinSet<()>,
    running: BTreeMap<Id, Running>,
    harness_running: BTreeMap<TaskId, CancelToken>,
    readers_running: BTreeMap<SourceId, CancelToken>,
}

impl Work {
    pub(crate) fn new(tools: RepoTools, harness: Option<HarnessPool>) -> Self {
        Self {
            tools,
            harness,
            tasks: JoinSet::new(),
            running: BTreeMap::new(),
            harness_running: BTreeMap::new(),
            readers_running: BTreeMap::new(),
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

    /// The repo tools over new roots (a source was shared or let go). Calls already running
    /// keep the roots they started with.
    pub(crate) fn set_tools(&mut self, tools: RepoTools) {
        self.tools = tools;
    }

    #[must_use]
    pub(crate) fn has_harness(&self) -> bool {
        self.harness.is_some()
    }

    /// Runs a source job, which answers through the inbox; false when as many run as may
    /// (`SOURCE_JOBS_MAX`), so the caller answers at once instead. A job past
    /// `SOURCE_JOB_TIMEOUT_MS` is dropped and `timed_out` answers in its place, so whatever waits
    /// on it (a brief checking its citations, a held page read, a resolve) is never left waiting.
    pub(crate) fn spawn_source_job(
        &mut self,
        job: BoxFuture<'static, Option<Envelope>>,
        timed_out: Option<Envelope>,
        inbox: &Inbox,
    ) -> bool {
        let running = self
            .running
            .values()
            .filter(|running| **running == Running::Source)
            .count();
        if running >= SOURCE_JOBS_MAX {
            tracing::warn!("too many source jobs are running; one was answered empty");
            return false;
        }
        let inbox = inbox.clone();
        let bound = Duration::from_millis(SOURCE_JOB_TIMEOUT_MS);
        let handle = self.tasks.spawn(async move {
            match tokio::time::timeout(bound, job).await {
                Ok(Some(envelope)) => {
                    let _ = inbox.send_envelope(envelope).await;
                }
                Ok(None) => {}
                Err(_elapsed) => {
                    tracing::warn!("a source job ran past its bound and was dropped");
                    if let Some(envelope) = timed_out {
                        let _ = inbox.send_envelope(envelope).await;
                    }
                }
            }
        });
        self.running.insert(handle.id(), Running::Source);
        true
    }

    /// Whether another reader may start.
    pub(crate) fn has_reader_room(&self) -> bool {
        self.readers_running.len() < SOURCE_READERS_RUNNING_MAX
    }

    /// Whether a reader can start on `source` now.
    pub(crate) fn reader_room(&self, source: SourceId) -> ReaderRoom {
        if self.readers_running.contains_key(&source) {
            ReaderRoom::Reading
        } else if !self.has_reader_room() {
            ReaderRoom::Full
        } else {
            ReaderRoom::Free
        }
    }

    /// Runs a document reader on `source`, its reports stepped as `DocumentRead`; false when as
    /// many run as may (`SOURCE_READERS_RUNNING_MAX`) or one already reads it.
    pub(crate) fn run_reader(
        &mut self,
        source: SourceId,
        name: String,
        mut events: ReaderStream,
        cancel: CancelToken,
        inbox: &Inbox,
    ) -> bool {
        if self.reader_room(source) != ReaderRoom::Free {
            return false;
        }
        self.readers_running.insert(source, cancel);
        let inbox = inbox.clone();
        let handle = self.tasks.spawn(async move {
            while let Some(event) = events.next().await {
                // Only a whole digest is cached (D-259): a partial one (figures a model didn't
                // read, or no model yet) is read again the next time, when a reader that reads
                // files may be configured.
                let whole = match &event {
                    ReaderEvent::Ready { digest, .. } => Some(digest.clone()),
                    ReaderEvent::Reading { .. }
                    | ReaderEvent::Partial { .. }
                    | ReaderEvent::Failed { .. } => None,
                };
                let (state, digest) = reader_state(&name, event);
                if let Some(markdown) = whole {
                    // The orchestrator caches it first, so a kept document's digest is in the
                    // vault before anything asks again.
                    let digested = SourceMessage::Digested { source, markdown };
                    if inbox
                        .send_envelope(Envelope::Source(digested))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                let input = Input::Source {
                    source: SourceInput::DocumentRead {
                        source,
                        state,
                        digest,
                    },
                };
                if inbox.send(input).await.is_err() {
                    break;
                }
            }
        });
        self.running.insert(handle.id(), Running::Reader(source));
        true
    }

    /// The source is no longer shared: its reader stops.
    pub(crate) fn stop_reader(&self, source: SourceId) {
        if let Some(cancel) = self.readers_running.get(&source) {
            cancel.cancel();
        }
    }

    /// Starts a harness task in a source's workspace (a brief, D-185); like [`Work::run_harness`].
    pub(crate) fn run_harness_in(
        &mut self,
        task: TaskId,
        spec: HarnessTask,
        workspace: Workspace,
        inbox: &Inbox,
    ) -> Option<Input> {
        self.start_harness(task, spec, Some(workspace), inbox)
    }

    /// Starts a harness task; returns its terminal event at once when it can't start.
    pub(crate) fn run_harness(
        &mut self,
        task: TaskId,
        spec: HarnessTask,
        inbox: &Inbox,
    ) -> Option<Input> {
        self.start_harness(task, spec, None, inbox)
    }

    fn start_harness(
        &mut self,
        task: TaskId,
        spec: HarnessTask,
        workspace: Option<Workspace>,
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
        let mut events = match workspace {
            Some(workspace) => pool.run_in(spec, workspace, cancel.clone()),
            None => pool.run(spec, cancel.clone()),
        };
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
    pub(crate) async fn reap(&mut self) -> Option<Reaped> {
        let id = match self.tasks.join_next_with_id().await? {
            Ok((id, ())) => id,
            Err(error) => {
                tracing::error!(%error, "a session task ended abnormally");
                error.id()
            }
        };
        match self.running.remove(&id) {
            Some(Running::Harness(task)) => {
                self.harness_running.remove(&task);
            }
            Some(Running::Reader(source)) => {
                self.readers_running.remove(&source);
                return Some(Reaped::Reader);
            }
            Some(Running::Tool | Running::Source) | None => {}
        }
        Some(Reaped::Other)
    }

    /// Stops everything: harness tasks are asked to stop, then every task is aborted (dropping
    /// a harness stream kills its process group).
    pub(crate) async fn shutdown(&mut self) {
        for cancel in self
            .harness_running
            .values()
            .chain(self.readers_running.values())
        {
            cancel.cancel();
        }
        self.tasks.shutdown().await;
        self.harness_running.clear();
        self.readers_running.clear();
        self.running.clear();
    }
}

/// A reader's report as the core's source state, and the digest it carries.
fn reader_state(reader: &str, event: ReaderEvent) -> (SourceState, Option<String>) {
    match event {
        ReaderEvent::Reading { read, of, by } => (
            SourceState::Reading {
                reader: by.unwrap_or_else(|| reader.to_owned()),
                files_read: read,
                files_estimate: of,
            },
            None,
        ),
        ReaderEvent::Ready { detail, digest } => (SourceState::Ready { detail }, Some(digest)),
        ReaderEvent::Partial { reason, digest } => (SourceState::Partial { reason }, Some(digest)),
        ReaderEvent::Failed { reason } => (SourceState::Failed { reason }, None),
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
    use crate::limits::{SESSION_TOOL_TIMEOUT_MS, SOURCE_JOB_TIMEOUT_MS};
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

    /// Finding 14: a source job past its bound was dropped without a word, so a brief whose
    /// citations it checked waited in `Checking` for ever. Its fallback answers in its place.
    #[tokio::test(start_paused = true)]
    async fn a_source_job_past_its_bound_is_answered_with_its_fallback() {
        use boardai_core::ids::TaskId;
        use boardai_core::sources::SourceInput;

        let mut channels = SessionChannels::new();
        let mut work = Work::new(RepoTools::default(), None);
        let unverified = Input::Source {
            source: SourceInput::CitationsChecked {
                task: TaskId(7),
                read: Vec::new(),
                unverified: Vec::new(),
            },
        };
        let stuck: BoxFuture<'static, Option<Envelope>> = Box::pin(std::future::pending());
        let timed_out = Some(Envelope::Input(unverified.clone()));
        assert!(work.spawn_source_job(stuck, timed_out, &channels.inbox));
        let wait = Duration::from_millis(SOURCE_JOB_TIMEOUT_MS + 1_000);
        let answer = tokio::time::timeout(wait, channels.receiver.recv())
            .await
            .ok()
            .flatten();
        assert!(
            matches!(&answer, Some(Envelope::Input(input)) if *input == unverified),
            "{answer:?}"
        );
        work.shutdown().await;
    }
}
