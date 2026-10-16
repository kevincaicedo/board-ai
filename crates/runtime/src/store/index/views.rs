//! Reads for the views summoned by voice (LLD §3.7): kept memories, recent notes with the
//! session they came from, and the latest digest as a card. Every answer is bounded.

use boardai_core::views::{DigestCard, MemoryRow, NoteRow, SessionCard, SessionRef};
use rusqlite::{Connection, OptionalExtension, params};

use crate::store::error::StoreError;
use crate::store::ids::SessionId;
use crate::store::vault::{ABOUT_YOU_FILE, BOARD_FILE, FACTS_FILE, VaultPath};

/// Which memory file a list is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryList {
    AboutYou,
    Facts,
}

/// Kept memories of one file, newest (last written) first.
pub(crate) fn memories(
    connection: &Connection,
    list: MemoryList,
    limit: usize,
) -> Result<Vec<MemoryRow>, StoreError> {
    let file = match list {
        MemoryList::AboutYou => ABOUT_YOU_FILE,
        MemoryList::Facts => FACTS_FILE,
    };
    let mut statement = connection
        .prepare("SELECT id, text FROM memories WHERE file = ?1 ORDER BY position DESC LIMIT ?2")?;
    let rows = statement.query_map(params![file, sql_limit(limit)], |row| {
        Ok(MemoryRow {
            id: row.get(0)?,
            text: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The newest notes, with the session each came from.
pub(crate) fn recent_notes(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<NoteRow>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT notes.id, notes.title, notes.created, notes.session, notes.links,
                sessions.title, sessions.started
         FROM notes LEFT JOIN sessions ON sessions.id = notes.session
         ORDER BY notes.created DESC, notes.id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([sql_limit(limit)], |row| {
        let session: Option<String> = row.get(3)?;
        let session_title: Option<String> = row.get(5)?;
        let session_started: Option<String> = row.get(6)?;
        let from = session.map(|id| SessionRef {
            id,
            title: session_title.unwrap_or_default(),
            date: date_of(session_started.as_deref().unwrap_or_default()),
        });
        Ok(NoteRow {
            id: row.get(0)?,
            text: row.get(1)?,
            date: date_of(&row.get::<_, String>(2)?),
            from,
            links: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The newest session with a digest, as a card, with what it captured. `cover` is filled by
/// the caller, which reads the board file. Also returns the board's vault path, if any.
pub(crate) fn latest_digest(
    connection: &Connection,
) -> Result<Option<(DigestCard, Option<String>)>, StoreError> {
    let latest = connection
        .query_row(
            "SELECT id, folder, title, started, duration_ms, has_board, open_questions
             FROM sessions WHERE has_digest = 1
             ORDER BY started_ms DESC, id DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((id, folder, title, started, duration_ms, has_board, questions)) = latest else {
        return Ok(None);
    };
    let count = |table: &str| -> Result<u32, StoreError> {
        let count: i64 = connection.query_row(
            &format!("SELECT count(*) FROM {table} WHERE session = ?1"),
            [&id],
            |row| row.get(0),
        )?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    };
    let notes = count("notes")?;
    let tasks = count("tasks")?;
    let open_questions = u32::try_from(
        questions
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
    )
    .unwrap_or(u32::MAX);
    let board = has_board
        .then(|| VaultPath::session_file(&folder, BOARD_FILE))
        .flatten()
        .map(|path| path.to_string());
    let card = DigestCard {
        session: SessionCard {
            id,
            title,
            date: date_of(&started),
            duration_ms: u32::try_from(duration_ms).unwrap_or(u32::MAX),
            cover: Vec::new(),
        },
        notes,
        tasks,
        open_questions,
    };
    Ok(Some((card, board)))
}

/// A session's folder, by exact id.
pub(crate) fn session_folder(
    connection: &Connection,
    id: &SessionId,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT folder FROM sessions WHERE id = ?1",
            [id.as_str()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Sessions the index holds without a digest, oldest first: the app stopped before writing
/// one (a crash, a kill). Returns `(id, folder, started, duration_ms)`.
pub(crate) fn sessions_without_digest(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<(String, String, String, u64)>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, folder, started, duration_ms FROM sessions WHERE has_digest = 0
         ORDER BY started_ms, id LIMIT ?1",
    )?;
    let rows = statement.query_map([sql_limit(limit)], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `YYYY-MM-DD` from an RFC 3339 time; empty when there is none.
fn date_of(rfc3339: &str) -> String {
    rfc3339.get(..10).unwrap_or_default().to_owned()
}

fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}
