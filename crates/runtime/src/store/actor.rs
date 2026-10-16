//! The store actor (LLD §3.1): one tokio task that owns every write (law 3). Messages arrive on
//! one bounded inbox, in order. Blocking work (files, fsync, SQLite) runs on the blocking pool:
//! the state moves into the blocking closure and back, so it has one owner and no lock.
//!
//! - Log entries are buffered in memory and written every `flush_interval` (and when the buffer
//!   is full). The orchestrator never waits on the store: the handle `try_send`s log entries
//!   and counts what a full inbox drops.
//! - Requests (start, end, remember, …) are applied one at a time, in inbox order.
//! - Reads start in inbox order, so they see every write sent before them, but run beside the
//!   actor on the read-only connection; the actor owns them in a `JoinSet` and waits for them
//!   at shutdown.
//! - Shutdown (a `Shutdown` message, or every handle dropped) flushes and closes the session's
//!   log durably, then ends the task.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::ids::CaptureId;
use boardai_core::memory::MemoryWrite;
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};

use super::digest::Digest;
use super::error::StoreError;
use super::ids::ItemId;
use super::ids::SessionId;
use super::index::{ScanReport, rebuild, seed_counters};
use super::log::LogEntry;
use super::read::Reader;
use super::state::{SessionEnded, StoreState};
use super::views::{
    Forgotten, Library, LibraryEntry, MemoryView, NowList, Remembered, SessionInfo, SessionView,
};
use boardai_core::views::NoteRow;

pub(crate) type Reply<T> = oneshot::Sender<Result<T, StoreError>>;

pub(crate) enum Message {
    Log(LogEntry),
    StartSession {
        title_hint: Option<String>,
        reply: Reply<SessionInfo>,
    },
    EndSession {
        t_end: SessionTime,
        board_export: Option<String>,
        reply: Reply<SessionEnded>,
    },
    WriteDigest {
        digest: Box<Digest>,
        board_export: Option<String>,
        reply: Reply<SessionInfo>,
    },
    Remember {
        write: MemoryWrite,
        reply: Reply<Remembered>,
    },
    AcceptProposal {
        id: ItemId,
        reply: Reply<Remembered>,
    },
    KeepProposal {
        capture: CaptureId,
        reply: Reply<Remembered>,
    },
    ForgetLast {
        reply: Reply<Option<Forgotten>>,
    },
    ForgetCapture {
        capture: CaptureId,
        reply: Reply<Option<Forgotten>>,
    },
    ForgetSession {
        reply: Reply<()>,
    },
    Flush {
        reply: Reply<()>,
    },
    Rebuild {
        reply: Reply<ScanReport>,
    },
    Read(ReadRequest),
    Shutdown {
        reply: Reply<()>,
    },
}

pub(crate) enum ReadRequest {
    Recall {
        query: String,
        reply: oneshot::Sender<ToolOutput>,
    },
    OpenSession {
        query: String,
        reply: Reply<SessionView>,
    },
    Library {
        reply: Reply<Library>,
    },
    NowList {
        project: Option<String>,
        reply: Reply<NowList>,
    },
    Recap {
        reply: Reply<Option<String>>,
    },
    FindSession {
        query: String,
        reply: Reply<LibraryEntry>,
    },
    MemoryView {
        reply: Reply<MemoryView>,
    },
    Notes {
        reply: Reply<Vec<NoteRow>>,
    },
    SessionLog {
        id: SessionId,
        reply: Reply<String>,
    },
}

type State = Option<Box<StoreState>>;

pub(crate) async fn run(
    state: Box<StoreState>,
    reader: Arc<Reader>,
    mut inbox: mpsc::Receiver<Message>,
    flush_interval: Duration,
) {
    let mut state: State = Some(state);
    let mut reads = JoinSet::new();
    let mut ticker = tokio::time::interval_at(Instant::now() + flush_interval, flush_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    while state.is_some() {
        let message = tokio::select! {
            message = inbox.recv() => message,
            _ = ticker.tick() => {
                flush(&mut state).await;
                continue;
            }
            Some(_) = reads.join_next(), if !reads.is_empty() => continue,
        };
        match message {
            None => {
                let _ = close(&mut state).await;
                break;
            }
            Some(Message::Log(entry)) => {
                let wants_flush = state.as_mut().is_some_and(|state| state.log(entry));
                if wants_flush {
                    flush(&mut state).await;
                }
            }
            Some(Message::Read(request)) => {
                let reader = Arc::clone(&reader);
                reads.spawn_blocking(move || serve_read(&reader, request));
            }
            Some(Message::Shutdown { reply }) => {
                let _ = reply.send(close(&mut state).await);
                break;
            }
            Some(request) => handle(&mut state, request).await,
        }
    }
    // Reads are bounded queries; they finish and answer their callers before the task ends.
    while reads.join_next().await.is_some() {}
}

/// Runs `work` on the state on a blocking thread. A panic there loses the state (a broken
/// invariant): every later request gets `Stopped`, and the actor ends.
async fn blocking<T: Send + 'static>(
    state: &mut State,
    work: impl FnOnce(&mut StoreState) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    let Some(mut taken) = state.take() else {
        return Err(StoreError::Stopped);
    };
    let joined = tokio::task::spawn_blocking(move || {
        let result = work(&mut taken);
        (taken, result)
    })
    .await;
    match joined {
        Ok((taken, result)) => {
            *state = Some(taken);
            result
        }
        Err(error) => {
            tracing::error!(%error, "the store stopped after a failure in a write");
            Err(StoreError::Stopped)
        }
    }
}

async fn flush(state: &mut State) {
    if !state.as_ref().is_some_and(|state| state.wants_flush()) {
        return;
    }
    if let Err(error) = blocking(state, StoreState::flush).await {
        // The writer counts the lost lines; the next flush starts on a fresh line.
        tracing::warn!(%error, "the session log could not be written");
    }
}

async fn close(state: &mut State) -> Result<(), StoreError> {
    if state.is_none() {
        return Err(StoreError::Stopped);
    }
    blocking(state, StoreState::close_for_shutdown).await
}

async fn handle(state: &mut State, message: Message) {
    match message {
        Message::StartSession { title_hint, reply } => {
            let result = blocking(state, move |state| state.start_session(title_hint)).await;
            let _ = reply.send(result);
        }
        Message::EndSession {
            t_end,
            board_export,
            reply,
        } => {
            let result = blocking(state, move |state| state.end_session(t_end, board_export)).await;
            let _ = reply.send(result);
        }
        Message::WriteDigest {
            digest,
            board_export,
            reply,
        } => {
            let result = blocking(state, move |state| {
                state.write_digest(*digest, board_export)
            })
            .await;
            let _ = reply.send(result);
        }
        Message::Remember { write, reply } => {
            let _ = reply.send(blocking(state, move |state| state.remember(write)).await);
        }
        Message::AcceptProposal { id, reply } => {
            let _ = reply.send(blocking(state, move |state| state.accept_proposal(id)).await);
        }
        Message::KeepProposal { capture, reply } => {
            let _ = reply.send(blocking(state, move |state| state.keep_proposal(capture)).await);
        }
        Message::ForgetLast { reply } => {
            let _ = reply.send(blocking(state, StoreState::forget_last).await);
        }
        Message::ForgetCapture { capture, reply } => {
            let kept = blocking(state, move |state| state.forget_capture(capture)).await;
            let _ = reply.send(kept);
        }
        Message::ForgetSession { reply } => {
            let _ = reply.send(blocking(state, StoreState::forget_session).await);
        }
        Message::Flush { reply } => {
            let _ = reply.send(blocking(state, StoreState::flush).await);
        }
        Message::Rebuild { reply } => {
            let result = blocking(state, |state| {
                let report = rebuild(&mut state.index, &state.vault)?;
                seed_counters(&state.index, &mut state.ids)?;
                Ok(report)
            })
            .await;
            let _ = reply.send(result);
        }
        // Handled by the loop; listed so a new message kind must be placed deliberately.
        Message::Log(_) | Message::Read(_) | Message::Shutdown { .. } => {}
    }
}

fn serve_read(reader: &Reader, request: ReadRequest) {
    match request {
        ReadRequest::Recall { query, reply } => {
            let _ = reply.send(reader.recall(&query));
        }
        ReadRequest::OpenSession { query, reply } => {
            let _ = reply.send(reader.open_session(&query));
        }
        ReadRequest::Library { reply } => {
            let _ = reply.send(reader.library());
        }
        ReadRequest::NowList { project, reply } => {
            let _ = reply.send(reader.now_list(project.as_deref()));
        }
        ReadRequest::Recap { reply } => {
            let _ = reply.send(reader.recap());
        }
        ReadRequest::FindSession { query, reply } => {
            let _ = reply.send(reader.find_card(&query));
        }
        ReadRequest::MemoryView { reply } => {
            let _ = reply.send(reader.memory_view());
        }
        ReadRequest::Notes { reply } => {
            let _ = reply.send(reader.recent_notes());
        }
        ReadRequest::SessionLog { id, reply } => {
            let _ = reply.send(reader.session_log(&id));
        }
    }
}
