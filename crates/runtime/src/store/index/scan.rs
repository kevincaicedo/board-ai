//! Keeping the index in step with the vault. A scan lists the vault's files once and compares
//! each with its `files` row: same modification time and size means unchanged; otherwise the
//! content hash decides, and a changed file re-indexes its unit. A full rebuild is the same
//! scan over an emptied index, so "delete the index and rebuild" gives the same rows.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;

use rusqlite::{Connection, params};
use serde::Serialize;

use super::ingest::{FileState, Unit, file_state, index_unit, modified_ms};
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

/// Empties the index and scans the whole vault.
pub(crate) fn rebuild(
    connection: &mut Connection,
    vault: &Vault,
) -> Result<ScanReport, StoreError> {
    let transaction = connection.transaction()?;
    for table in TABLES {
        transaction.execute(&format!("DELETE FROM {table}"), [])?;
    }
    transaction.commit()?;
    reconcile(connection, vault)
}

/// Re-indexes what changed since the last scan.
pub(crate) fn reconcile(
    connection: &mut Connection,
    vault: &Vault,
) -> Result<ScanReport, StoreError> {
    let known = known_files(connection)?;
    let (found, truncated) = list_files(vault);
    let mut report = ScanReport {
        files_seen: found.len() as u64,
        truncated,
        ..ScanReport::default()
    };
    let mut units = BTreeSet::new();
    let mut seen = HashSet::new();
    let transaction = connection.transaction()?;
    for entry in &found {
        seen.insert(entry.path.as_str());
        let changed = match known.get(entry.path.as_str()) {
            Some(state)
                if state.mtime_ms == entry.mtime_ms && state.size_bytes == entry.size_bytes =>
            {
                false
            }
            Some(state) => {
                let current = file_state(&vault.absolute(&entry.path))
                    .map_err(|error| StoreError::io(entry.path.as_str(), &error))?;
                match current {
                    Some(current) if current.hash == state.hash => {
                        transaction.execute(
                            "UPDATE files SET mtime_ms = ?2, size_bytes = ?3 WHERE path = ?1",
                            params![entry.path.as_str(), current.mtime_ms, current.size_bytes],
                        )?;
                        false
                    }
                    Some(_) | None => true,
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
    for unit in &units {
        index_unit(&transaction, vault, unit)?;
    }
    transaction.commit()?;
    report.reindexed = units.len() as u64;
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
