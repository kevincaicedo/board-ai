//! The index schema and its migrations. The version lives in SQLite's `user_version`.
//!
//! - Older index: each migration after its version runs, in order, in one transaction.
//! - Newer index (written by a later build), or a file that isn't a database: the index is
//!   derived, so it is deleted and rebuilt from the vault rather than guessed at.
//!
//! Every row names its vault file in `path`; the `files` table holds each file's modification
//! time, size and content hash, so a changed file is re-indexed and nothing else.

use rusqlite::Connection;

/// The version this build writes.
pub(crate) const SCHEMA_VERSION: i64 = 2;

/// `MIGRATIONS[n]` takes an index from version `n` to `n + 1`.
const MIGRATIONS: [&str; SCHEMA_VERSION as usize] = [V1, V2];

const V1: &str = r"
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    mtime_ms INTEGER NOT NULL,
    size_bytes INTEGER NOT NULL,
    hash TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    folder TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    started TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    has_board INTEGER NOT NULL,
    has_digest INTEGER NOT NULL,
    summary TEXT NOT NULL,
    open_questions TEXT NOT NULL
) STRICT;

CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    title TEXT NOT NULL,
    created TEXT NOT NULL,
    session TEXT,
    links TEXT NOT NULL
) STRICT;

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    project TEXT NOT NULL,
    done INTEGER NOT NULL,
    links TEXT NOT NULL,
    session TEXT
) STRICT;

CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    position INTEGER NOT NULL,
    file TEXT NOT NULL,
    text TEXT NOT NULL
) STRICT;

CREATE TABLE proposals (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    session TEXT NOT NULL,
    about TEXT NOT NULL,
    text TEXT NOT NULL,
    accepted INTEGER NOT NULL
) STRICT;

CREATE TABLE entities (
    name TEXT NOT NULL COLLATE NOCASE,
    kind TEXT NOT NULL,
    path TEXT NOT NULL,
    session TEXT NOT NULL,
    PRIMARY KEY (name, path)
) STRICT;

CREATE TABLE relations (
    subject TEXT NOT NULL COLLATE NOCASE,
    relation TEXT NOT NULL CHECK (relation IN
        ('explains', 'part_of', 'depends_on', 'mentioned_in', 'struggled_with')),
    object TEXT NOT NULL COLLATE NOCASE,
    path TEXT NOT NULL,
    session TEXT NOT NULL,
    PRIMARY KEY (subject, relation, object, path)
) STRICT;

CREATE VIRTUAL TABLE search USING fts5(
    title,
    body,
    kind UNINDEXED,
    ref UNINDEXED,
    path UNINDEXED,
    tokenize = 'porter unicode61'
);

CREATE INDEX notes_path ON notes (path);
CREATE INDEX tasks_path ON tasks (path);
CREATE INDEX tasks_open ON tasks (done, project, position);
CREATE INDEX memories_path ON memories (path);
CREATE INDEX proposals_path ON proposals (path);
CREATE INDEX entities_path ON entities (path);
CREATE INDEX relations_path ON relations (path);
CREATE INDEX sessions_started ON sessions (started_ms);
";

/// FTS5 removes a deleted row's terms from its index at once, instead of leaving them in older
/// segments until a merge: a forgotten session's words must not stay readable (I10). The
/// option is stored in the table's own config, so it holds for every later connection.
const V2: &str = r"
INSERT INTO search (search, rank) VALUES ('secure-delete', 1);
";

/// What opening the index found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaState {
    /// A new, empty index.
    Created,
    /// Already at this build's version.
    Current,
    /// Migrated up from an older version.
    Migrated { from: i64 },
}

/// Why an existing index file is unusable and must be rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
    /// Written by a newer build.
    NewerVersion(i64),
    /// Not a database, or damaged.
    Damaged(String),
}

/// Brings the schema to [`SCHEMA_VERSION`].
pub(crate) fn migrate(connection: &mut Connection) -> Result<SchemaState, Unusable> {
    let damaged = |error: rusqlite::Error| Unusable::Damaged(error.to_string());
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(damaged)?;
    if version > SCHEMA_VERSION {
        return Err(Unusable::NewerVersion(version));
    }
    if version == SCHEMA_VERSION {
        let check: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(damaged)?;
        return if check == "ok" {
            Ok(SchemaState::Current)
        } else {
            Err(Unusable::Damaged(check))
        };
    }
    let transaction = connection.transaction().map_err(damaged)?;
    let first = usize::try_from(version).unwrap_or(0);
    for migration in MIGRATIONS.iter().skip(first) {
        transaction.execute_batch(migration).map_err(damaged)?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(damaged)?;
    transaction.commit().map_err(damaged)?;
    Ok(if version == 0 {
        SchemaState::Created
    } else {
        SchemaState::Migrated { from: version }
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{SchemaState, V1, migrate};

    /// An index from the first build migrates to v2, which turns FTS5's secure delete on: a
    /// deleted row's words leave the search index at once (I10).
    #[test]
    fn a_v1_index_migrates_to_secure_delete() {
        let Ok(mut connection) = Connection::open_in_memory() else {
            panic!("an in-memory index");
        };
        assert!(connection.execute_batch(V1).is_ok());
        assert!(connection.pragma_update(None, "user_version", 1).is_ok());
        assert_eq!(
            migrate(&mut connection),
            Ok(SchemaState::Migrated { from: 1 })
        );
        let secure: Option<i64> = connection
            .query_row(
                "SELECT v FROM search_config WHERE k = 'secure-delete'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(secure, Some(1));
    }
}
