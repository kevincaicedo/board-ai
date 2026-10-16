//! The session's store over the real [`Store`] (LLD §3.1, §3.6): the event log, memory by
//! voice, the views, and the session's start and end. The store is the only writer (law 3) and
//! redacts at its boundary (I8); this adapter only routes.
//!
//! - Log entries go straight to the store, which never makes the orchestrator wait.
//! - Memory writes, forgets and keeps go through one ordered queue to a writer task that applies
//!   them one at a time. "Forget that" is turned into the capture it undoes right here, as the
//!   core emits it: the sink keeps the core's captures in step with the effects (never with the
//!   store's answers), so a write the queue dropped or the store failed can't shift what "that"
//!   is, and the store undoes exactly that capture.
//! - What the store could not do is reported to the core (`Input::MemoryFailed`), which shows it
//!   and tells the talker: BoardAI already said "Remembered.", "Forgotten." or "Kept.".
//! - "Don't remember this session" never waits in the queue and is never dropped (I10): the
//!   store is told at once (no digest for this session, whatever comes first), and the writer
//!   applies it before anything else still queued.
//! - Reads, the session's start and its end first wait for the queue to drain, so they see
//!   every write sent before them.

use std::sync::{Arc, Mutex, PoisonError};

use boardai_core::effect::{ForgetScope, LogRecord};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::{MemoryChange, MemoryWrite};
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};

use super::inbox::StoreLink;
use super::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
use super::store_views::{read_view, session_context};
use crate::limits::STORE_WRITES_CAPACITY;
use crate::llm::LlmProvider;
use crate::store::{Logged, SessionFinished, Store, StoreError, WallClock};

/// A [`StoreSink`] over the vault. The writer task it starts ends once the sink is dropped,
/// after applying what was queued.
pub struct VaultSink {
    store: Store,
    scribe: Option<Arc<dyn LlmProvider>>,
    clock: Arc<dyn WallClock>,
    writes: mpsc::Sender<Write>,
    /// The core's captures of this session, newest last: pushed by `remember` and
    /// `accept_proposal`, popped by "forget that", in the order the core emits them. Bounded by
    /// the core's `CAPTURES_PER_SESSION_MAX`.
    captures: Mutex<Vec<CaptureId>>,
    /// Bumped by "don't remember this session": a signal beside the queue, never full.
    forget_session: watch::Sender<u64>,
    reports: Reports,
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
    /// "Forget that": undo exactly this capture.
    Forget(CaptureId),
    /// "Yes, keep it": keep the proposal made as this capture.
    Keep(CaptureId),
    /// Answered once everything sent before it was applied.
    Settled(oneshot::Sender<()>),
}

impl Write {
    /// What the core is told when this change can't be made.
    fn change(&self) -> Option<MemoryChange> {
        match self {
            Self::Remember(memory) => Some(MemoryChange::Remember {
                capture: memory.capture,
            }),
            Self::Forget(capture) => Some(MemoryChange::Forget { capture: *capture }),
            Self::Keep(capture) => Some(MemoryChange::Keep { capture: *capture }),
            Self::Settled(_) => None,
        }
    }
}

/// Where failures are reported: the session's link, once it hands it over.
#[derive(Clone, Default)]
struct Reports {
    link: Arc<Mutex<Option<StoreLink>>>,
}

impl Reports {
    fn send(&self, change: MemoryChange, detail: String) {
        tracing::warn!(?change, %detail, "a memory change was not made");
        let link = self
            .link
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(link) = link else {
            return;
        };
        if link.memory_failed(change, detail).is_err() {
            tracing::error!(
                ?change,
                "the failure could not be reported: the session is busy"
            );
        }
    }
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
        let (forget_session, forgets) = watch::channel(0);
        let reports = Reports::default();
        tokio::spawn(apply_writes(queue, forgets, store.clone(), reports.clone()));
        Self {
            store,
            scribe,
            clock,
            writes,
            captures: Mutex::new(Vec::new()),
            forget_session,
            reports,
        }
    }

    fn enqueue(&self, write: Write) {
        let change = write.change();
        let detail = match self.writes.try_send(write) {
            Ok(()) => return,
            Err(TrySendError::Full(_)) => {
                format!("the store is not keeping up ({STORE_WRITES_CAPACITY} changes are waiting)")
            }
            Err(TrySendError::Closed(_)) => StoreError::Stopped.to_string(),
        };
        if let Some(change) = change {
            self.reports.send(change, detail);
        }
    }

    fn captures(&self) -> std::sync::MutexGuard<'_, Vec<CaptureId>> {
        // The list is only pushed and popped; a panic elsewhere can't leave it half-changed.
        self.captures.lock().unwrap_or_else(PoisonError::into_inner)
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

async fn apply_writes(
    mut queue: mpsc::Receiver<Write>,
    mut forgets: watch::Receiver<u64>,
    store: Store,
    reports: Reports,
) {
    let mut forgets_open = true;
    loop {
        tokio::select! {
            // A forget of the session goes before anything still queued (I10).
            biased;
            changed = forgets.changed(), if forgets_open => match changed {
                Ok(()) => {
                    forgets.borrow_and_update();
                    match store.forget_session().await {
                        // Memory was off (no session started): nothing was kept to forget.
                        Ok(()) | Err(StoreError::NoSession) => {}
                        Err(error) => reports.send(MemoryChange::ForgetSession, error.to_string()),
                    }
                }
                Err(_) => forgets_open = false,
            },
            write = queue.recv() => match write {
                Some(write) => apply(&store, &reports, write).await,
                None => break,
            },
        }
    }
}

async fn apply(store: &Store, reports: &Reports, write: Write) {
    let change = write.change();
    let failed = match write {
        Write::Remember(memory) => store.remember(*memory).await.err(),
        // `Ok(None)`: the write never reached the vault, so nothing is left to undo. With no
        // session recording, nothing was kept either.
        Write::Forget(capture) => match store.forget_capture(capture).await {
            Ok(_) | Err(StoreError::NoSession) => None,
            Err(error) => Some(error),
        },
        Write::Keep(capture) => store.keep_proposal(capture).await.err(),
        Write::Settled(done) => {
            let _ = done.send(());
            None
        }
    };
    match (failed, change) {
        // "Don't remember this session" came first: nothing more is kept, as asked.
        (Some(StoreError::SessionForgotten), _) | (None, _) | (Some(_), None) => {}
        (Some(error), Some(change)) => reports.send(change, error.to_string()),
    }
}

impl StoreSink for VaultSink {
    fn report_to(&self, link: StoreLink) {
        *self
            .reports
            .link
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(link);
    }

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
        self.captures().push(memory.capture);
        self.enqueue(Write::Remember(Box::new(memory.clone())));
    }

    fn forget(&self, _at: SessionTime, scope: ForgetScope) {
        match scope {
            ForgetScope::Last => {
                let last = self.captures().pop();
                // The core says "forget that" only when it captured something.
                if let Some(capture) = last {
                    self.enqueue(Write::Forget(capture));
                }
            }
            ForgetScope::Session => {
                self.captures().clear();
                self.store.mark_forgotten();
                self.forget_session.send_modify(|count| *count += 1);
            }
        }
    }

    fn accept_proposal(&self, _at: SessionTime, capture: CaptureId) {
        self.captures().push(capture);
        self.enqueue(Write::Keep(capture));
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
        self.captures().clear();
        let store = self.store.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
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
