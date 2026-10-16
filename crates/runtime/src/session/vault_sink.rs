//! The session's store over the real [`Store`] (LLD §3.1, §3.6): the event log, memory by
//! voice, the views, and the session's start and end. The store is the only writer (law 3) and
//! redacts at its boundary (I8); this adapter only routes.
//!
//! - Log entries go straight to the store, which never makes the orchestrator wait.
//! - Memory writes, forgets and keeps go through one ordered queue to a writer task that applies
//!   them one at a time. It mirrors the core's captures: "forget that" undoes the newest one the
//!   store really kept (a write the store refused has nothing to undo), and "yes, keep it" names
//!   the store's proposal by the core's capture id.
//! - Reads, the session's start and its end first wait for the queue to drain, so they see
//!   every write sent before them.

use std::sync::Arc;

use boardai_core::effect::{ForgetScope, LogRecord};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use super::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
use super::store_views::{read_view, session_context};
use crate::limits::STORE_WRITES_CAPACITY;
use crate::llm::LlmProvider;
use crate::store::{Logged, Remembered, SessionFinished, Store, StoreError, WallClock};

/// A [`StoreSink`] over the vault. The writer task it starts ends once the sink is dropped,
/// after applying what was queued.
pub struct VaultSink {
    store: Store,
    scribe: Option<Arc<dyn LlmProvider>>,
    clock: Arc<dyn WallClock>,
    writes: mpsc::Sender<Write>,
}

impl std::fmt::Debug for VaultSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultSink")
            .field("scribe", &self.scribe.is_some())
            .finish_non_exhaustive()
    }
}

/// One memory change, in the order the core made it.
enum Write {
    Remember(Box<MemoryWrite>),
    Forget(ForgetScope),
    Accept(CaptureId),
    /// A new session: its captures start empty.
    NewSession,
    /// Answered once everything sent before it was applied.
    Settled(oneshot::Sender<()>),
}

/// One capture as the store took it.
struct Captured {
    /// The store kept it, so "forget that" has something to undo.
    kept: bool,
    /// A proposal: the core's capture id and the store's proposal id (`p3`).
    proposal: Option<(CaptureId, String)>,
}

impl VaultSink {
    /// Must be called on a tokio runtime: it starts the writer task.
    #[must_use]
    pub fn new(
        store: Store,
        scribe: Option<Arc<dyn LlmProvider>>,
        clock: Arc<dyn WallClock>,
    ) -> Self {
        let (writes, queue) = mpsc::channel(STORE_WRITES_CAPACITY);
        tokio::spawn(apply_writes(queue, store.clone()));
        Self {
            store,
            scribe,
            clock,
            writes,
        }
    }

    fn enqueue(&self, write: Write) {
        match self.writes.try_send(write) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::error!(
                    capacity = STORE_WRITES_CAPACITY,
                    "a memory change was dropped: the store is not keeping up"
                );
            }
            Err(TrySendError::Closed(_)) => tracing::warn!("the store's writer has stopped"),
        }
    }

    fn logged(logged: Logged) {
        match logged {
            Logged::Accepted | Logged::Dropped => {}
            Logged::Stopped => tracing::debug!("a log entry was lost: the store has stopped"),
        }
    }
}

/// Waits until the writer applied everything sent before; a stopped writer has nothing left.
async fn settled(writes: &mpsc::Sender<Write>) {
    let (done, wait) = oneshot::channel();
    if writes.send(Write::Settled(done)).await.is_ok() {
        let _ = wait.await;
    }
}

async fn apply_writes(mut queue: mpsc::Receiver<Write>, store: Store) {
    // Bounded by the core: a session captures at most `CAPTURES_PER_SESSION_MAX` items.
    let mut captures: Vec<Captured> = Vec::new();
    while let Some(write) = queue.recv().await {
        match write {
            Write::Remember(memory) => {
                let capture = memory.capture;
                let captured = match store.remember(*memory).await {
                    Ok(Remembered::Proposed { id }) => Captured {
                        kept: true,
                        proposal: Some((capture, id.to_string())),
                    },
                    Ok(
                        Remembered::Memory { .. }
                        | Remembered::Note { .. }
                        | Remembered::Task { .. }
                        | Remembered::SessionRecord,
                    ) => Captured {
                        kept: true,
                        proposal: None,
                    },
                    Err(error) => {
                        tracing::warn!(%error, "a memory was not kept");
                        Captured {
                            kept: false,
                            proposal: None,
                        }
                    }
                };
                captures.push(captured);
            }
            Write::Forget(ForgetScope::Last) => {
                let last = captures.pop();
                if last.is_some_and(|last| last.kept)
                    && let Err(error) = store.forget_last().await
                {
                    tracing::warn!(%error, "\"forget that\" could not undo the last capture");
                }
            }
            Write::Forget(ForgetScope::Session) => {
                captures.clear();
                if let Err(error) = store.forget_session().await {
                    tracing::error!(%error, "\"don't remember this session\" failed");
                }
            }
            Write::Accept(capture) => captures.push(accept(&store, &captures, capture).await),
            Write::NewSession => captures.clear(),
            Write::Settled(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Keeps the proposal the core captured as `capture`. The keep is itself a capture, so "forget
/// that" right after undoes it.
async fn accept(store: &Store, captures: &[Captured], capture: CaptureId) -> Captured {
    let proposal = captures
        .iter()
        .filter_map(|captured| captured.proposal.as_ref())
        .find(|(proposed, _)| *proposed == capture)
        .map(|(_, id)| id.clone());
    let kept = match proposal {
        Some(id) => match store.accept_proposal(&id).await {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(%error, "a proposed memory could not be kept");
                false
            }
        },
        None => {
            tracing::warn!(capture = capture.0, "the proposal to keep was never stored");
            false
        }
    };
    Captured {
        kept,
        proposal: None,
    }
}

impl StoreSink for VaultSink {
    fn input(&self, at: SessionTime, input: &Input) {
        Self::logged(self.store.log_input(at, input.clone()));
    }

    fn ui_event(&self, at: SessionTime, event: &UiEvent) {
        Self::logged(self.store.log_ui(at, vec![event.clone()]));
    }

    fn log(&self, at: SessionTime, record: &LogRecord) {
        Self::logged(self.store.log_record(at, record.clone()));
    }

    fn remember(&self, _at: SessionTime, memory: &MemoryWrite) {
        self.enqueue(Write::Remember(Box::new(memory.clone())));
    }

    fn forget(&self, _at: SessionTime, scope: ForgetScope) {
        self.enqueue(Write::Forget(scope));
    }

    fn accept_proposal(&self, _at: SessionTime, capture: CaptureId) {
        self.enqueue(Write::Accept(capture));
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        let store = self.store.clone();
        let writes = self.writes.clone();
        let today = self.clock.now().date();
        Box::pin(async move {
            settled(&writes).await;
            read_view(&store, read, today).await
        })
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        let store = self.store.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            let _ = writes.send(Write::NewSession).await;
            settled(&writes).await;
            let (context, suggestion) = session_context(&store).await;
            let session = match store.start_session(None).await {
                Ok(info) => Some(info.id.to_string()),
                Err(error) => {
                    tracing::error!(%error, "the session could not start its log; memory is off");
                    None
                }
            };
            SessionOpened {
                session,
                context,
                suggestion,
            }
        })
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        let store = self.store.clone();
        let writes = self.writes.clone();
        let scribe = self.scribe.clone();
        Box::pin(async move {
            settled(&writes).await;
            finish(&store, scribe.as_deref(), close).await;
        })
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        let store = self.store.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            settled(&writes).await;
            if let Err(error) = store.flush().await {
                tracing::warn!(%error, "the session log could not be flushed");
            }
        })
    }
}

async fn finish(store: &Store, scribe: Option<&dyn LlmProvider>, close: SessionClose) {
    let finished = match close {
        SessionClose::Discard => store
            .forget_session()
            .await
            .map(|()| SessionFinished::Forgotten),
        SessionClose::Digest {
            t_end,
            board,
            deadline,
            cancel,
        } => {
            store
                .finish_session_within(t_end, scribe, board, deadline, cancel)
                .await
        }
    };
    match finished {
        Ok(SessionFinished::Written {
            session,
            written_by,
        }) => tracing::info!(folder = %session.folder, ?written_by, "session digest written"),
        Ok(SessionFinished::Forgotten) => tracing::info!("the session left nothing in the vault"),
        // A session that never started (memory off) has nothing to finish.
        Err(StoreError::NoSession) => {}
        Err(error) => tracing::error!(%error, "the session could not be finished"),
    }
}
