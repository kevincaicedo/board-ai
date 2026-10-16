//! The index (LLD §3.6, D-012): `index.sqlite` in the app's data folder, derived from the vault
//! and rebuildable from it at any time. SQLite is bundled with FTS5 for full-text search over
//! notes, digests, memories, tasks and transcripts.
//!
//! One read-write connection belongs to the store actor (the only writer, law 3); reads use a
//! separate read-only connection in WAL mode, so a recall never waits for a write to finish.

mod ingest;
pub(crate) mod query;
mod scan;
mod schema;
pub(crate) mod views;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub(crate) use ingest::{UNTITLED as UNTITLED_SESSION, Unit, index_units};
pub use scan::ScanReport;
pub(crate) use scan::{rebuild, reconcile, seed_counters};
pub use schema::SchemaState;

use self::schema::{Unusable, migrate};
use crate::limits::INDEX_BUSY_TIMEOUT_MS;
use crate::store::error::StoreError;

/// How the index file was found at startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexOpened {
    Schema(SchemaState),
    /// The file was unusable (newer or damaged) and was replaced by an empty index; the scan
    /// that follows rebuilds it from the vault.
    Recreated {
        reason: String,
    },
}

/// Opens (creating or migrating) the writer connection.
pub(crate) fn open_writer(path: &Path) -> Result<(Connection, IndexOpened), StoreError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| StoreError::Index {
            detail: format!("the index folder can't be created: {}", error.kind()),
        })?;
    }
    let mut recreated = None;
    for _ in 0..2 {
        let opened = Connection::open(path).and_then(|connection| {
            configure(&connection)?;
            Ok(connection)
        });
        let unusable = match opened {
            Ok(mut connection) => match migrate(&mut connection) {
                Ok(state) => {
                    let opened = recreated.map_or(IndexOpened::Schema(state), |reason| {
                        IndexOpened::Recreated { reason }
                    });
                    return Ok((connection, opened));
                }
                Err(unusable) => unusable,
            },
            // A file that isn't a database fails as soon as a pragma touches it.
            Err(error) => Unusable::Damaged(error.to_string()),
        };
        let reason = match unusable {
            Unusable::NewerVersion(version) => format!("written by a newer build (v{version})"),
            Unusable::Damaged(detail) => format!("damaged: {detail}"),
        };
        tracing::warn!(%reason, "the index is rebuilt from the vault");
        remove_index_files(path);
        recreated = Some(reason);
    }
    Err(StoreError::Index {
        detail: "the index could not be recreated".to_owned(),
    })
}

/// Opens a read-only connection for queries. The writer must have created the schema first.
pub(crate) fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(INDEX_BUSY_TIMEOUT_MS))?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(Duration::from_millis(INDEX_BUSY_TIMEOUT_MS))?;
    // WAL lets the reader connection query while the writer commits. The index is derived, so
    // `NORMAL` durability is enough: a lost commit is redone by the next scan.
    let _mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    // Deleted rows are overwritten with zeros, not left in free space: what you forget must not
    // stay readable in the file (I10).
    connection.pragma_update(None, "secure_delete", true)?;
    Ok(())
}

/// After "don't remember this session" (I10), nothing of it may stay readable in the index's
/// files. Deleted rows are already zeroed (`secure_delete`) and FTS5 already dropped their
/// terms from its own index (`secure-delete`, schema v2); what is left is the write-ahead log,
/// which still holds pages as they were before. Checkpointing it with `TRUNCATE` copies the
/// clean pages into the database and empties the log.
pub(crate) fn scrub(connection: &Connection) -> Result<(), StoreError> {
    let (busy, _frames, _copied): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(StoreError::Index {
            detail: "the index's log could not be emptied: a read held it past the busy timeout"
                .to_owned(),
        });
    }
    Ok(())
}

fn remove_index_files(path: &Path) {
    let mut names = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        names.push(name.into());
    }
    for name in names {
        let _ = std::fs::remove_file(name);
    }
}
