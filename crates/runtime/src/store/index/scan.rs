//! Keeping the index in step with the vault. A scan lists the vault's files once and compares
//! each with its `files` row: same modification time and size means unchanged; otherwise the
//! content hash decides, and a changed file re-indexes its unit. A full rebuild is the same
//! scan over an emptied index, in the same transaction, so "delete the index and rebuild"
//! gives the same rows, and a rebuild that fails leaves the index it started from.
//!
//! One unreadable file never stops a scan: its unit is skipped, counted in the report, and
//! tried again at the next scan.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;

use rusqlite::{Connection, Transaction, params};
use serde::Serialize;

use super::ingest::{FileState, Unit, file_state, index_units, modified_ms};
use crate::limits::VAULT_SCAN_FILES_MAX;
use crate::store::error::StoreError;
use crate::store::ids::{IdCounters, ItemId};
use crate::store::vault::{
    BOARD_FILE, DIGEST_FILE, EVENTS_FILE, MemoryFile, NOTES, PROJECTS, SESSIONS, Vault, VaultPath,
};

/// What a scan did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    pub files_seen: u64,
    pub unchanged: u64,
    /// Units (session folders, notes, projects, memory files) indexed again.
    pub reindexed: u64,
    /// Files in the index that are gone from the vault.
    pub removed: u64,
    /// Units left out because a file couldn't be read (a permission, an I/O error); each is
    /// logged with its path and tried again at the next scan.
    pub skipped: u64,
    /// The scan stopped at `VAULT_SCAN_FILES_MAX`; files past it are not indexed, and nothing
    /// is removed from the index because the scan did not see everything.
    pub truncated: bool,
}

struct Found {
    path: VaultPath,
    mtime_ms: i64,
    size_bytes: i64,
}

const TABLES: [&str; 9] = [
    "files",
    "sessions",
    "notes",
    "tasks",
    "memories",
    "proposals",
    "entities",
    "relations",
    "search",
];

/// Empties the index and scans the whole vault, in one transaction: a rescan that fails
/// leaves the index as it was.
pub(crate) fn rebuild(
    connection: &mut Connection,
    vault: &Vault,
) -> Result<ScanReport, StoreError> {
    let mut transaction = connection.transaction()?;
    for table in TABLES {
        transaction.execute(&format!("DELETE FROM {table}"), [])?;
    }
    let report = reconcile_in(&mut transaction, vault)?;
    transaction.commit()?;
    Ok(report)
}

/// Re-indexes what changed since the last scan.
pub(crate) fn reconcile(
    connection: &mut Connection,
    vault: &Vault,
) -> Result<ScanReport, StoreError> {
    let mut transaction = connection.transaction()?;
    let report = reconcile_in(&mut transaction, vault)?;
    transaction.commit()?;
    Ok(report)
}

fn reconcile_in(
    transaction: &mut Transaction<'_>,
    vault: &Vault,
) -> Result<ScanReport, StoreError> {
    let known = known_files(transaction)?;
    let (found, truncated) = list_files(vault);
    let mut report = ScanReport {
        files_seen: found.len() as u64,
        truncated,
        ..ScanReport::default()
    };
    let mut units = BTreeSet::new();
    let mut seen = HashSet::new();
    for entry in &found {
        seen.insert(entry.path.as_str());
        let changed = match known.get(entry.path.as_str()) {
            Some(state)
                if state.mtime_ms == entry.mtime_ms && state.size_bytes == entry.size_bytes =>
            {
                false
            }
            Some(state) => {
                // A file that can't be read now counts as changed: its unit is re-indexed, which
                // skips and reports it.
                match file_state(&vault.absolute(&entry.path)) {
                    Ok(Some(current)) if current.hash == state.hash => {
                        transaction.execute(
                            "UPDATE files SET mtime_ms = ?2, size_bytes = ?3 WHERE path = ?1",
                            params![entry.path.as_str(), current.mtime_ms, current.size_bytes],
                        )?;
                        false
                    }
                    Ok(Some(_) | None) | Err(_) => true,
                }
            }
            None => true,
        };
        if changed {
            units.extend(Unit::of(&entry.path));
        } else {
            report.unchanged += 1;
        }
    }
    if !truncated {
        for path in known.keys().filter(|path| !seen.contains(path.as_str())) {
            report.removed += 1;
            match VaultPath::parse(path).as_ref().and_then(Unit::of) {
                Some(unit) => {
                    units.insert(unit);
                }
                None => {
                    transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
                }
            }
        }
    }
    let units: Vec<Unit> = units.into_iter().collect();
    let skipped = index_units(transaction, vault, &units)?;
    report.skipped = skipped.len() as u64;
    report.reindexed = (units.len() - skipped.len()) as u64;
    Ok(report)
}

/// Makes the id counters hand out numbers above every id in the index.
pub(crate) fn seed_counters(
    connection: &Connection,
    counters: &mut IdCounters,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT id FROM notes UNION ALL SELECT id FROM tasks
         UNION ALL SELECT id FROM memories UNION ALL SELECT id FROM proposals",
    )?;
    let ids = statement.query_map([], |row| row.get::<_, String>(0))?;
    for id in ids {
        if let Some(id) = ItemId::parse(&id?) {
            counters.observe(id);
        }
    }
    Ok(())
}

fn known_files(connection: &Connection) -> Result<HashMap<String, FileState>, StoreError> {
    let mut statement = connection.prepare("SELECT path, mtime_ms, size_bytes, hash FROM files")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            FileState {
                mtime_ms: row.get(1)?,
                size_bytes: row.get(2)?,
                hash: row.get(3)?,
            },
        ))
    })?;
    let mut known = HashMap::new();
    for row in rows {
        let (path, state) = row?;
        known.insert(path, state);
    }
    Ok(known)
}

/// Every indexable file in the vault, at most `VAULT_SCAN_FILES_MAX`. Hidden files and folders
/// (temporary files among them) are never listed.
fn list_files(vault: &Vault) -> (Vec<Found>, bool) {
    let mut found = Vec::new();
    let mut candidates: Vec<VaultPath> = Vec::new();
    for folder in names_in(vault, SESSIONS) {
        for file in [EVENTS_FILE, DIGEST_FILE, BOARD_FILE] {
            candidates.extend(VaultPath::session_file(&folder, file));
        }
    }
    for folder in [NOTES, PROJECTS] {
        for name in names_in(vault, folder) {
            if name.ends_with(".md") {
                candidates.extend(VaultPath::join(&[folder, &name]));
            }
        }
    }
    candidates.push(VaultPath::memory(MemoryFile::AboutYou));
    candidates.push(VaultPath::memory(MemoryFile::Facts));
    for path in candidates {
        if found.len() >= VAULT_SCAN_FILES_MAX {
            return (found, true);
        }
        let Ok(metadata) = fs::metadata(vault.absolute(&path)) else {
            continue;
        };
        if metadata.is_file() {
            found.push(Found {
                path,
                mtime_ms: modified_ms(&metadata),
                size_bytes: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            });
        }
    }
    (found, false)
}

/// The visible entries of a vault folder, sorted; an unreadable folder has none.
fn names_in(vault: &Vault, folder: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir(vault.root().join(folder)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .take(VAULT_SCAN_FILES_MAX)
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::{rebuild, reconcile};
    use crate::store::index::open_writer;
    use crate::store::vault::Vault;

    /// A rebuild whose rescan fails in the index (here a planted trigger; a full disk in life)
    /// rolls back to the index it started from, never to an empty one.
    #[test]
    fn a_rebuild_that_fails_keeps_the_index_it_started_from() {
        let root = std::env::temp_dir().join(format!("boardai-rebuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let vault = Vault::open(&root.join("vault"), None);
        let opened = open_writer(&root.join("index.sqlite"));
        let (Ok(vault), Ok((mut connection, _))) = (vault, opened) else {
            panic!("a vault and an index");
        };
        assert!(std::fs::write(vault.root().join("notes/alpha.md"), "# Alpha\n").is_ok());
        assert!(reconcile(&mut connection, &vault).is_ok());
        let notes = |connection: &rusqlite::Connection| -> i64 {
            connection
                .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
                .unwrap_or(-1)
        };
        assert_eq!(notes(&connection), 1);
        let planted = "CREATE TEMP TRIGGER planted BEFORE INSERT ON notes \
                       BEGIN SELECT RAISE(ABORT, 'planted'); END;";
        assert!(connection.execute_batch(planted).is_ok());
        assert!(rebuild(&mut connection, &vault).is_err());
        assert_eq!(notes(&connection), 1, "the note is still indexed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
