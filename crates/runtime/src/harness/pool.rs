//! Harness limits (LLD §3.5, §5): 2 read-only and 1 change task at a time, 4 waiting, and a
//! timeout per kind. Admission is decided when `run` is called, so a refused task is refused at
//! once and out loud.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use futures_util::{StreamExt, stream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::time::Instant;

use super::process::failed;
use super::{CodingHarness, HarnessStream, Workspace};
use crate::cancel::CancelToken;
use crate::limits::{
    HARNESS_CHANGE_TASKS_MAX, HARNESS_CHANGE_TIMEOUT_MS, HARNESS_QUEUE_MAX, HARNESS_READ_TASKS_MAX,
    HARNESS_RESEARCH_TIMEOUT_MS,
};

/// Timeouts per kind; the limits by default, shorter in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HarnessTimeouts {
    pub read_only: Duration,
    pub change: Duration,
}

impl Default for HarnessTimeouts {
    fn default() -> Self {
        Self {
            read_only: Duration::from_millis(HARNESS_RESEARCH_TIMEOUT_MS),
            change: Duration::from_millis(HARNESS_CHANGE_TIMEOUT_MS),
        }
    }
}

/// Runs tasks on one harness within the limits. Cheap to clone; clones share the limits.
#[derive(Clone)]
pub struct HarnessPool {
    harness: Arc<dyn CodingHarness>,
    read_slots: Arc<Semaphore>,
    change_slots: Arc<Semaphore>,
    waiting: Arc<AtomicUsize>,
    timeouts: HarnessTimeouts,
}

impl HarnessPool {
    pub fn new(harness: Arc<dyn CodingHarness>, timeouts: HarnessTimeouts) -> Self {
        Self {
            harness,
            read_slots: Arc::new(Semaphore::new(HARNESS_READ_TASKS_MAX)),
            change_slots: Arc::new(Semaphore::new(HARNESS_CHANGE_TASKS_MAX)),
            waiting: Arc::new(AtomicUsize::new(0)),
            timeouts,
        }
    }

    /// Runs a task, waits in the queue for a slot, or is refused with `QueueFull`. The stream
    /// ends with exactly one `Finished` or `Failed`; dropping it cancels the task.
    pub fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream {
        self.admit(task, None, cancel)
    }

    /// [`HarnessPool::run`] in a workspace of its own (a shared source, D-184), within the same
    /// limits.
    pub fn run_in(
        &self,
        task: HarnessTask,
        workspace: Workspace,
        cancel: CancelToken,
    ) -> HarnessStream {
        self.admit(task, Some(workspace), cancel)
    }

    fn admit(
        &self,
        task: HarnessTask,
        workspace: Option<Workspace>,
        cancel: CancelToken,
    ) -> HarnessStream {
        let (slots, timeout) = match task.kind {
            HarnessKind::Research | HarnessKind::Review => {
                (Arc::clone(&self.read_slots), self.timeouts.read_only)
            }
            HarnessKind::Change { .. } => (Arc::clone(&self.change_slots), self.timeouts.change),
        };
        let phase = match Arc::clone(&slots).try_acquire_owned() {
            Ok(permit) => Phase::Admitted(permit),
            Err(TryAcquireError::NoPermits) => match QueueTicket::take(&self.waiting) {
                Some(ticket) => Phase::Queued(ticket),
                None => return refused("the task queue is full; try again when one finishes"),
            },
            Err(TryAcquireError::Closed) => return refused("the harness is shutting down"),
        };
        let run = PoolRun {
            harness: Arc::clone(&self.harness),
            task,
            workspace,
            slots,
            timeout,
            caller: cancel,
            phase,
        };
        Box::pin(stream::unfold(run, |mut run| async move {
            let event = run.next().await?;
            Some((event, run))
        }))
    }
}

fn refused(detail: &str) -> HarnessStream {
    Box::pin(stream::once(std::future::ready(failed(
        HarnessFailure::QueueFull,
        detail.to_owned(),
    ))))
}

/// A place in the queue; giving it back (on start, cancel or drop) frees it.
struct QueueTicket {
    waiting: Arc<AtomicUsize>,
}

impl QueueTicket {
    fn take(waiting: &Arc<AtomicUsize>) -> Option<Self> {
        waiting
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < HARNESS_QUEUE_MAX).then_some(count + 1)
            })
            .ok()
            .map(|_| Self {
                waiting: Arc::clone(waiting),
            })
    }
}

impl Drop for QueueTicket {
    fn drop(&mut self) {
        self.waiting.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PoolRun {
    harness: Arc<dyn CodingHarness>,
    task: HarnessTask,
    workspace: Option<Workspace>,
    slots: Arc<Semaphore>,
    timeout: Duration,
    caller: CancelToken,
    phase: Phase,
}

enum Phase {
    Queued(QueueTicket),
    Admitted(OwnedSemaphorePermit),
    Running(Running),
    Done,
}

struct Running {
    _permit: OwnedSemaphorePermit,
    inner: HarnessStream,
    inner_cancel: CancelToken,
    deadline: Instant,
    timed_out: bool,
}

impl PoolRun {
    async fn next(&mut self) -> Option<HarnessEvent> {
        loop {
            match std::mem::replace(&mut self.phase, Phase::Done) {
                Phase::Done => return None,
                Phase::Queued(ticket) => {
                    let slots = Arc::clone(&self.slots);
                    tokio::select! {
                        biased;
                        () = self.caller.cancelled() => {
                            drop(ticket);
                            return Some(failed(HarnessFailure::Cancelled, "cancelled while queued".to_owned()));
                        }
                        permit = slots.acquire_owned() => match permit {
                            Ok(permit) => self.phase = Phase::Admitted(permit),
                            Err(_closed) => return Some(failed(HarnessFailure::QueueFull, "the harness is shutting down".to_owned())),
                        },
                    }
                }
                Phase::Admitted(permit) => {
                    let inner_cancel = CancelToken::new();
                    let task = self.task.clone();
                    let inner = match self.workspace.clone() {
                        Some(workspace) => {
                            self.harness.run_in(task, workspace, inner_cancel.clone())
                        }
                        None => self.harness.run(task, inner_cancel.clone()),
                    };
                    self.phase = Phase::Running(Running {
                        _permit: permit,
                        inner,
                        inner_cancel,
                        deadline: Instant::now() + self.timeout,
                        timed_out: false,
                    });
                }
                Phase::Running(mut running) => {
                    let event = step(&self.caller, self.timeout, &mut running).await;
                    if let Some(event) = event {
                        let terminal = matches!(
                            event,
                            HarnessEvent::Finished { .. } | HarnessEvent::Failed { .. }
                        );
                        if !terminal {
                            self.phase = Phase::Running(running);
                        }
                        return Some(event);
                    }
                    // `None`: the inner stream ended without a terminal event.
                    return Some(failed(
                        HarnessFailure::Crashed,
                        "the harness ended without a result".to_owned(),
                    ));
                }
            }
        }
    }
}

/// The next event from the harness, asking it to stop when the caller cancels or the timeout
/// passes. A stop forced by the timeout is reported as `TimedOut`.
async fn step(
    caller: &CancelToken,
    timeout: Duration,
    running: &mut Running,
) -> Option<HarnessEvent> {
    loop {
        let stopping = running.inner_cancel.is_cancelled();
        tokio::select! {
            biased;
            () = caller.cancelled(), if !stopping => running.inner_cancel.cancel(),
            () = tokio::time::sleep_until(running.deadline), if !stopping => {
                running.timed_out = true;
                running.inner_cancel.cancel();
            }
            event = running.inner.next() => {
                return event.map(|event| match event {
                    HarnessEvent::Failed { reason: HarnessFailure::Cancelled, .. }
                        if running.timed_out =>
                    {
                        failed(HarnessFailure::TimedOut, format!("no result within {timeout:?}"))
                    }
                    HarnessEvent::Progress { .. }
                    | HarnessEvent::FileTouched { .. }
                    | HarnessEvent::Finished { .. }
                    | HarnessEvent::Failed { .. } => event,
                });
            }
        }
    }
}
