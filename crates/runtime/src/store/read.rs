//! Reads, on the read-only index connection and the vault. They run on blocking threads that
//! the actor starts in inbox order (so a read sees every write sent before it) and does not
//! wait for (so a slow query never delays a log flush).

use std::sync::Mutex;

use boardai_core::tools::{ToolFailure, ToolOutput};
use rusqlite::Connection;

use super::board::final_board;
use super::cover::cover_shapes;
use super::error::StoreError;
use super::ids::SessionId;
use super::index::query::{
    find_session, library, match_expression, now_list, recall, recall_text, recap, two_sentences,
};
use super::index::views::{MemoryList, latest_digest, memories, recent_notes, session_folder};
use super::log::EventLogReader;
use super::vault::{BOARD_FILE, EVENTS_FILE, FileRead, Vault, VaultPath, read_text};
use super::views::{Library, LibraryEntry, MemoryView, NowList, SessionView};
use crate::limits::{
    LIBRARY_COVERS_MAX, MEMORY_VIEW_ITEMS_MAX, RECENT_NOTES_MAX, SESSION_LOG_READ_MAX_BYTES,
    VAULT_FILE_MAX_BYTES,
};
use boardai_core::views::{CoverShape, NoteRow};

pub(crate) struct Reader {
    connection: Mutex<Connection>,
    vault: Vault,
}

impl Reader {
    pub(crate) fn new(connection: Connection, vault: Vault) -> Self {
        Self {
            connection: Mutex::new(connection),
            vault,
        }
    }

    /// A query on the read connection, for readers in other modules.
    pub(crate) fn query<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with(query)
    }

    pub(crate) fn vault(&self) -> &Vault {
        &self.vault
    }

    fn with<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        // A panic while holding the lock leaves the connection usable: SQLite rolls back the
        // read, so the poison flag carries no broken state.
        let guard = self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        query(&guard)
    }

    /// The talker's `recall` tool: top hits with their sources, as text.
    pub(crate) fn recall(&self, query: &str) -> ToolOutput {
        let Some(expression) = match_expression(query) else {
            return ToolOutput::Text {
                text: "Nothing to search for: ask about a topic, a system or a name.".to_owned(),
            };
        };
        match self.with(|connection| recall(connection, &expression)) {
            Ok((hits, more)) => ToolOutput::Text {
                text: recall_text(&hits, more),
            },
            Err(error) => ToolOutput::Failed {
                kind: ToolFailure::Io,
                message: error.to_string(),
            },
        }
    }

    /// The talker's `open_session` tool: a session by id, or the best match for the words.
    pub(crate) fn open_session(&self, query: &str) -> Result<SessionView, StoreError> {
        let record = self
            .with(|connection| find_session(connection, query))?
            .ok_or_else(|| StoreError::NotFound {
                what: "a session matching that".to_owned(),
            })?;
        let events = VaultPath::session_file(&record.folder, EVENTS_FILE).ok_or_else(|| {
            StoreError::NotFound {
                what: "the session's log".to_owned(),
            }
        })?;
        let events_path = self.vault.absolute(&events);
        let io_error = |error: std::io::Error| StoreError::io(events.as_str(), &error);
        let mut reader = EventLogReader::open(&events_path).map_err(io_error)?;
        let mut failure = None;
        let lines = reader.by_ref().map_while(|line| match line {
            Ok(line) => Some(line),
            Err(error) => {
                failure = Some(error);
                None
            }
        });
        let board = final_board(lines);
        if let Some(error) = failure {
            return Err(io_error(error));
        }
        Ok(SessionView {
            id: record.id,
            title: record.title,
            started: record.started,
            duration_ms: record.duration_ms,
            events_path,
            recap: two_sentences(&record.summary),
            board: board.events,
            board_events_skipped: board.skipped,
            log_skipped: reader.skipped(),
        })
    }

    /// Sessions, newest first; the newest [`LIBRARY_COVERS_MAX`] get their cover shapes.
    pub(crate) fn library(&self) -> Result<Library, StoreError> {
        let mut found = self.with(library)?;
        for entry in found.sessions.iter_mut().take(LIBRARY_COVERS_MAX) {
            entry.shapes = self.cover(entry.cover.as_deref());
        }
        Ok(found)
    }

    /// One session as a library card: by id, or the best match for the words.
    pub(crate) fn find_card(&self, query: &str) -> Result<LibraryEntry, StoreError> {
        let record = self
            .with(|connection| find_session(connection, query))?
            .ok_or_else(|| StoreError::NotFound {
                what: "a session matching that".to_owned(),
            })?;
        let board = VaultPath::session_file(&record.folder, BOARD_FILE)
            .filter(|path| self.vault.absolute(path).is_file())
            .map(|path| path.to_string());
        Ok(LibraryEntry {
            date: super::time::date_of(&record.started),
            shapes: self.cover(board.as_deref()),
            id: record.id,
            title: record.title,
            started: record.started,
            duration_ms: record.duration_ms,
            cover: board,
        })
    }

    /// What memory shows: kept memories, recent notes, and the latest digest.
    pub(crate) fn memory_view(&self) -> Result<MemoryView, StoreError> {
        let (about_you, systems, notes, latest) = self.with(|connection| {
            Ok((
                memories(connection, MemoryList::AboutYou, MEMORY_VIEW_ITEMS_MAX)?,
                memories(connection, MemoryList::Facts, MEMORY_VIEW_ITEMS_MAX)?,
                recent_notes(connection, RECENT_NOTES_MAX)?,
                latest_digest(connection)?,
            ))
        })?;
        let latest = latest.map(|(mut card, board)| {
            card.session.cover = self.cover(board.as_deref());
            card
        });
        Ok(MemoryView {
            about_you,
            systems,
            notes,
            latest,
        })
    }

    pub(crate) fn recent_notes(&self) -> Result<Vec<NoteRow>, StoreError> {
        self.with(|connection| recent_notes(connection, RECENT_NOTES_MAX))
    }

    /// A session's whole log, for a replay: found by its exact id only (never a path), and
    /// at most [`SESSION_LOG_READ_MAX_BYTES`].
    pub(crate) fn session_log(&self, id: &SessionId) -> Result<String, StoreError> {
        let folder = self
            .with(|connection| session_folder(connection, id))?
            .ok_or_else(|| StoreError::NotFound {
                what: "a session with that id".to_owned(),
            })?;
        let events =
            VaultPath::session_file(&folder, EVENTS_FILE).ok_or_else(|| StoreError::NotFound {
                what: "the session's log".to_owned(),
            })?;
        let read = read_text(&self.vault.absolute(&events), SESSION_LOG_READ_MAX_BYTES)
            .map_err(|error| StoreError::io(events.as_str(), &error))?;
        match read {
            FileRead::Text { text, .. } => Ok(text),
            FileRead::Missing => Err(StoreError::NotFound {
                what: "the session's log".to_owned(),
            }),
            FileRead::TooLarge { .. } => Err(StoreError::Limit {
                what: "bytes in a replayed log",
                max: usize::try_from(SESSION_LOG_READ_MAX_BYTES).unwrap_or(usize::MAX),
            }),
        }
    }

    /// The cover shapes of a board file (a vault path); none when it is missing or unreadable.
    fn cover(&self, board: Option<&str>) -> Vec<CoverShape> {
        let Some(path) = board.and_then(VaultPath::parse) else {
            return Vec::new();
        };
        match read_text(&self.vault.absolute(&path), VAULT_FILE_MAX_BYTES) {
            Ok(FileRead::Text { text, .. }) => cover_shapes(&text),
            Ok(FileRead::Missing | FileRead::TooLarge { .. }) | Err(_) => Vec::new(),
        }
    }

    pub(crate) fn now_list(&self, project: Option<&str>) -> Result<NowList, StoreError> {
        self.with(|connection| now_list(connection, project))
    }

    pub(crate) fn recap(&self) -> Result<Option<String>, StoreError> {
        self.with(recap)
    }
}
