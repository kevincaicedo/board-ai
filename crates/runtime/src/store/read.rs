//! Reads, on the read-only index connection and the vault. They run on blocking threads that
//! the actor starts in inbox order (so a read sees every write sent before it) and does not
//! wait for (so a slow query never delays a log flush).

use std::sync::Mutex;

use boardai_core::tools::{ToolFailure, ToolOutput};
use rusqlite::Connection;

use super::board::final_board;
use super::error::StoreError;
use super::index::query::{
    find_session, library, match_expression, now_list, recall, recall_text, recap, two_sentences,
};
use super::log::EventLogReader;
use super::vault::{EVENTS_FILE, Vault, VaultPath};
use super::views::{Library, NowList, SessionView};

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

    pub(crate) fn library(&self) -> Result<Library, StoreError> {
        self.with(library)
    }

    pub(crate) fn now_list(&self, project: Option<&str>) -> Result<NowList, StoreError> {
        self.with(|connection| now_list(connection, project))
    }

    pub(crate) fn recap(&self) -> Result<Option<String>, StoreError> {
        self.with(recap)
    }
}
