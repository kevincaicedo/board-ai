//! The store (LLD §3.6): the only writer (law 3). It owns the session event log, the vault (the
//! source of truth: plain files you own) and the SQLite index derived from it.
//!
//! [`Store`] is a cheap handle to one actor task. The orchestrator logs through it without ever
//! waiting, applies memory writes the core's policy decided, and ends sessions with a digest;
//! the talker tools and the UI's views read through it.
//!
//! Everything written passes [`redact_text`] or [`redact_json_text`] first (I8): writers take a
//! [`Redacted`], which nothing else can make.

mod actor;
mod board;
mod cover;
mod digest;
mod error;
mod ids;
mod index;
mod log;
mod read;
mod redact;
mod slug;
mod state;
mod time;
mod vault;
mod views;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use boardai_core::effect::LogRecord;
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::UiEvent;
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolFailure, ToolOutput};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use digest::digest_within;
pub use digest::{
    About, Digest, Entity, EntityKind, FallbackReason, ProposalDraft, Relation, RelationKind,
    SCRIBE_SYSTEM_PROMPT, SessionMaterial, TranscriptLine, WrittenBy, digest_with_scribe,
    fallback_digest, scribe_config,
};
pub use error::{Refusal, StoreError, VaultError};
pub use ids::{ItemId, ItemKind, SessionId};
pub use index::{IndexOpened, ScanReport, SchemaState};
pub use log::{DropReason, EventLogReader, LOG_VERSION, LogEntry, LogLine, SkipCounts};
pub use redact::{Redacted, redact_json_text, redact_text};
pub use state::SessionEnded;
pub use time::{FakeWallClock, SystemWallClock, WallClock, WallTime};
pub use vault::{MemoryFile, Vault, VaultPath};
pub use views::{
    Forgotten, ForgottenKind, Library, LibraryEntry, MemoryFileName, MemoryView, NowList,
    Remembered, SessionInfo, SessionView, TaskView,
};

use self::actor::{Message, ReadRequest, Reply};
pub(crate) use self::index::UNTITLED_SESSION;
use self::index::{open_reader, open_writer, reconcile, seed_counters};
use self::read::Reader;
use self::state::{SessionSlot, StoreState};
use crate::cancel::CancelToken;
use crate::limits::{EVENT_LOG_FLUSH_MS, SCRIBE_DIGEST_DEADLINE_MS, STORE_INBOX_CAPACITY};
use crate::llm::LlmProvider;
use boardai_core::views::NoteRow;

/// Where the store keeps things, and its bounds. Tests shorten the bounds; the app uses
/// [`StoreConfig::new`].
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// The vault, from settings (`~/BoardAI` by default).
    pub vault: PathBuf,
    /// Your home folder: the vault may not be it or one of its parents.
    pub home: Option<PathBuf>,
    /// `index.sqlite` in the app's data folder.
    pub index_path: PathBuf,
    pub clock: Arc<dyn WallClock>,
    pub inbox_capacity: usize,
    pub flush_interval: Duration,
}

impl StoreConfig {
    #[must_use]
    pub fn new(vault: PathBuf, index_path: PathBuf) -> Self {
        Self {
            vault,
            home: std::env::var_os("HOME").map(PathBuf::from),
            index_path,
            clock: Arc::new(SystemWallClock),
            inbox_capacity: STORE_INBOX_CAPACITY,
            flush_interval: Duration::from_millis(EVENT_LOG_FLUSH_MS),
        }
    }
}

/// What opening the store found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreOpened {
    pub index: IndexOpened,
    /// The startup scan that brought the index in step with the vault.
    pub scan: ScanReport,
}

/// What happened to a log entry. Logging never waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logged {
    Accepted,
    /// The inbox was full; the entry is counted in a `dropped` line at the next flush.
    Dropped,
    /// The store has stopped.
    Stopped,
}

/// How a session finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionFinished {
    Written {
        session: SessionInfo,
        written_by: WrittenBy,
    },
    /// "Don't remember this session": nothing was written.
    Forgotten,
}

/// A handle to the store actor. Clones share it.
#[derive(Clone, Debug)]
pub struct Store {
    inbox: mpsc::Sender<Message>,
    /// Log entries dropped at a full inbox since the last flush (the actor resets it).
    inbox_dropped: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
    /// "Don't remember this session" was said; shared with the actor, which reads it before
    /// any digest and clears it once the session is forgotten.
    forget_requested: Arc<AtomicBool>,
}

/// The actor task. Its owner awaits [`StoreTask::join`] after [`Store::shutdown`].
#[derive(Debug)]
pub struct StoreTask {
    handle: JoinHandle<()>,
}

impl StoreTask {
    pub async fn join(self) -> Result<(), StoreError> {
        self.handle.await.map_err(|_| StoreError::Stopped)
    }
}

impl Store {
    /// Validates the vault, opens (or rebuilds) the index, brings it in step with the vault,
    /// and starts the actor.
    pub async fn open(config: StoreConfig) -> Result<(Self, StoreTask, StoreOpened), StoreError> {
        let inbox_dropped = Arc::new(AtomicU64::new(0));
        let forget_requested = Arc::new(AtomicBool::new(false));
        let shared = Shared {
            inbox_dropped: Arc::clone(&inbox_dropped),
            forget_requested: Arc::clone(&forget_requested),
        };
        let flush_interval = config.flush_interval;
        let capacity = config.inbox_capacity.max(1);
        let opened = tokio::task::spawn_blocking(move || open_blocking(config, shared))
            .await
            .map_err(|_| StoreError::Stopped)??;
        let (state, reader, report) = opened;
        let (sender, receiver) = mpsc::channel(capacity);
        let handle = tokio::spawn(actor::run(
            state,
            Arc::new(reader),
            receiver,
            flush_interval,
        ));
        let store = Self {
            inbox: sender,
            inbox_dropped,
            dropped_total: Arc::new(AtomicU64::new(0)),
            forget_requested,
        };
        Ok((store, StoreTask { handle }, report))
    }

    /// Logs one entry without waiting. A full inbox drops it and counts it.
    pub fn log(&self, entry: LogEntry) -> Logged {
        match self.inbox.try_send(Message::Log(entry)) {
            Ok(()) => Logged::Accepted,
            Err(TrySendError::Full(_)) => {
                self.inbox_dropped.fetch_add(1, Ordering::AcqRel);
                self.dropped_total.fetch_add(1, Ordering::Relaxed);
                Logged::Dropped
            }
            Err(TrySendError::Closed(_)) => Logged::Stopped,
        }
    }

    pub fn log_input(&self, t: SessionTime, input: Input) -> Logged {
        self.log(LogEntry::Input { t, input })
    }

    pub fn log_ui(&self, t: SessionTime, events: Vec<UiEvent>) -> Logged {
        self.log(LogEntry::Ui { t, events })
    }

    pub fn log_record(&self, t: SessionTime, record: LogRecord) -> Logged {
        self.log(LogEntry::Record { t, record })
    }

    /// Log entries dropped at a full inbox since the store opened.
    #[must_use]
    pub fn dropped_log_entries(&self) -> u64 {
        self.dropped_total.load(Ordering::Relaxed)
    }

    /// Starts a session: its folder and its log. `title_hint` titles it if the scribe can't.
    pub async fn start_session(
        &self,
        title_hint: Option<String>,
    ) -> Result<SessionInfo, StoreError> {
        self.request(|reply| Message::StartSession { title_hint, reply })
            .await
    }

    /// Closes the session's log durably and returns what its digest is made from.
    pub async fn end_session(&self, t_end: SessionTime) -> Result<SessionEnded, StoreError> {
        self.request(|reply| Message::EndSession {
            t_end,
            board_export: None,
            reply,
        })
        .await
    }

    /// Writes the ended session's digest and board (the Excalidraw export from the core), and
    /// renames its folder after the title.
    pub async fn write_digest(
        &self,
        digest: Digest,
        board_export: Option<String>,
    ) -> Result<SessionInfo, StoreError> {
        self.request(|reply| Message::WriteDigest {
            digest: Box::new(digest),
            board_export,
            reply,
        })
        .await
    }

    /// Ends the session and writes its digest: the scribe's, or the minimal one when there is
    /// no scribe or it fails. The store stays responsive while the scribe works.
    pub async fn finish_session(
        &self,
        t_end: SessionTime,
        scribe: Option<&dyn LlmProvider>,
        board_export: Option<String>,
        cancel: CancelToken,
    ) -> Result<SessionFinished, StoreError> {
        let deadline = Duration::from_millis(SCRIBE_DIGEST_DEADLINE_MS);
        self.finish_session_within(t_end, scribe, board_export, deadline, cancel)
            .await
    }

    /// [`Store::finish_session`] with a shorter scribe deadline (quitting can't wait 120 s).
    /// Past `deadline`, or once `cancel` fires, the minimal digest is written instead.
    pub async fn finish_session_within(
        &self,
        t_end: SessionTime,
        scribe: Option<&dyn LlmProvider>,
        board_export: Option<String>,
        deadline: Duration,
        cancel: CancelToken,
    ) -> Result<SessionFinished, StoreError> {
        // The board is written with the end of the log, before the scribe runs: if finishing
        // is cut short, the minimal digest written later still has it.
        let ended = self.request(|reply| Message::EndSession {
            t_end,
            board_export,
            reply,
        });
        let material = match ended.await? {
            SessionEnded::Forgotten => return Ok(SessionFinished::Forgotten),
            SessionEnded::Material(material) => material,
        };
        let digest = match scribe {
            Some(scribe) => {
                tokio::select! {
                    digest = digest_within(scribe, &material, cancel.clone(), deadline) => digest,
                    () = cancel.cancelled() => {
                        fallback_digest(&material, FallbackReason::Cancelled)
                    }
                }
            }
            None => fallback_digest(&material, FallbackReason::NoScribe),
        };
        let written_by = digest.written_by.clone();
        let session = self.write_digest(digest, None).await?;
        Ok(SessionFinished::Written {
            session,
            written_by,
        })
    }

    /// Applies a memory write the policy decided (`Keep`, `OnRequest`, `Propose`).
    pub async fn remember(&self, write: MemoryWrite) -> Result<Remembered, StoreError> {
        self.request(|reply| Message::Remember { write, reply })
            .await
    }

    /// Keeps a proposed memory after a yes. `id` is the proposal's id (`p3`).
    pub async fn accept_proposal(&self, id: &str) -> Result<Remembered, StoreError> {
        let id = ItemId::parse(id)
            .filter(|id| id.kind() == ItemKind::Proposal)
            .ok_or_else(|| StoreError::NotFound {
                what: "a proposal with that id".to_owned(),
            })?;
        self.request(|reply| Message::AcceptProposal { id, reply })
            .await
    }

    /// "Yes, keep it" for the proposal this session made as the core's `capture`.
    pub async fn keep_proposal(&self, capture: CaptureId) -> Result<Remembered, StoreError> {
        self.request(|reply| Message::KeepProposal { capture, reply })
            .await
    }

    /// "Forget that": removes the last thing this session captured.
    pub async fn forget_last(&self) -> Result<Option<Forgotten>, StoreError> {
        self.request(|reply| Message::ForgetLast { reply }).await
    }

    /// "Forget that" for what the core captured as `capture` (a keep and its proposal share
    /// one; the keep goes first). `Ok(None)` when the store holds nothing of it: the write never
    /// reached the vault. A capture that can't be undone stays recorded and is still removed
    /// with the session.
    pub async fn forget_capture(
        &self,
        capture: CaptureId,
    ) -> Result<Option<Forgotten>, StoreError> {
        self.request(|reply| Message::ForgetCapture { capture, reply })
            .await
    }

    /// "Don't remember this session" (I10): leaves nothing of it in the vault or the index.
    pub async fn forget_session(&self) -> Result<(), StoreError> {
        self.request(|reply| Message::ForgetSession { reply }).await
    }

    /// Marks the session being recorded as forgotten at once, without waiting: whatever the
    /// store does next for it (ending it, a digest, shutting down) forgets it instead, even if
    /// the [`Store::forget_session`] request is still on its way. Never blocks, never fails.
    pub fn mark_forgotten(&self) {
        self.forget_requested.store(true, Ordering::Release);
    }

    /// The talker's `recall` tool. Never fails silently: a store problem is a failed output.
    pub async fn recall(&self, query: &str) -> ToolOutput {
        let (reply, response) = oneshot::channel();
        let request = Message::Read(ReadRequest::Recall {
            query: query.to_owned(),
            reply,
        });
        let unavailable = || ToolOutput::Failed {
            kind: ToolFailure::Unavailable,
            message: StoreError::Stopped.to_string(),
        };
        if self.inbox.send(request).await.is_err() {
            return unavailable();
        }
        response.await.unwrap_or_else(|_| unavailable())
    }

    /// The talker's `open_session` tool: a session by id or by words in its title.
    pub async fn open_session(&self, query: &str) -> Result<SessionView, StoreError> {
        let query = query.to_owned();
        self.request(|reply| Message::Read(ReadRequest::OpenSession { query, reply }))
            .await
    }

    pub async fn library(&self) -> Result<Library, StoreError> {
        self.request(|reply| Message::Read(ReadRequest::Library { reply }))
            .await
    }

    /// Open tasks, for one project or all.
    pub async fn now_list(&self, project: Option<String>) -> Result<NowList, StoreError> {
        self.request(|reply| Message::Read(ReadRequest::NowList { project, reply }))
            .await
    }

    /// "Where we left off", from the latest digest, for the next session's context.
    pub async fn recap(&self) -> Result<Option<String>, StoreError> {
        self.request(|reply| Message::Read(ReadRequest::Recap { reply }))
            .await
    }

    /// One session as a library card, by id or by words from its title (a replay by voice).
    pub async fn find_session(&self, query: &str) -> Result<LibraryEntry, StoreError> {
        let query = query.to_owned();
        self.request(|reply| Message::Read(ReadRequest::FindSession { query, reply }))
            .await
    }

    /// What the memory view shows: kept memories, recent notes and the latest digest.
    pub async fn memory_view(&self) -> Result<MemoryView, StoreError> {
        self.request(|reply| Message::Read(ReadRequest::MemoryView { reply }))
            .await
    }

    /// The newest notes, with the session each came from.
    pub async fn recent_notes(&self) -> Result<Vec<NoteRow>, StoreError> {
        self.request(|reply| Message::Read(ReadRequest::Notes { reply }))
            .await
    }

    /// A session's whole `events.jsonl`, for a replay: by exact id, never by path, bounded
    /// by `SESSION_LOG_READ_MAX_BYTES`.
    pub async fn session_log(&self, id: &SessionId) -> Result<String, StoreError> {
        let id = id.clone();
        self.request(|reply| Message::Read(ReadRequest::SessionLog { id, reply }))
            .await
    }

    /// Writes the buffered log now.
    pub async fn flush(&self) -> Result<(), StoreError> {
        self.request(|reply| Message::Flush { reply }).await
    }

    /// Empties the index and rebuilds it from one scan of the vault.
    pub async fn rebuild_index(&self) -> Result<ScanReport, StoreError> {
        self.request(|reply| Message::Rebuild { reply }).await
    }

    /// Flushes and closes a session still recording, then stops the actor.
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        self.request(|reply| Message::Shutdown { reply }).await
    }

    async fn request<T>(&self, message: impl FnOnce(Reply<T>) -> Message) -> Result<T, StoreError> {
        let (reply, response) = oneshot::channel();
        self.inbox
            .send(message(reply))
            .await
            .map_err(|_| StoreError::Stopped)?;
        response.await.map_err(|_| StoreError::Stopped)?
    }
}

type Opened = (Box<StoreState>, Reader, StoreOpened);

/// What the handle and the actor share.
struct Shared {
    inbox_dropped: Arc<AtomicU64>,
    forget_requested: Arc<AtomicBool>,
}

fn open_blocking(config: StoreConfig, shared: Shared) -> Result<Opened, StoreError> {
    let vault = Vault::open(&config.vault, config.home.as_deref()).map_err(StoreError::Vault)?;
    let (mut index, opened) = open_writer(&config.index_path)?;
    let scan = reconcile(&mut index, &vault)?;
    let mut ids = ids::IdCounters::default();
    seed_counters(&index, &mut ids)?;
    let reader = Reader::new(open_reader(&config.index_path)?, vault.clone());
    let mut state = Box::new(StoreState {
        vault,
        index,
        clock: config.clock,
        ids,
        slot: SessionSlot::Idle,
        inbox_dropped: shared.inbox_dropped,
        forget_requested: shared.forget_requested,
    });
    state.recover_interrupted();
    Ok((
        state,
        reader,
        StoreOpened {
            index: opened,
            scan,
        },
    ))
}
