//! Turns vault files into index rows. A unit (a session folder, a note, a project, a memory
//! file) is always re-indexed whole: its rows are deleted by path and rebuilt from the files,
//! so the index can never hold a row its file no longer has.
//!
//! Text goes through redaction on its way in: a secret pasted into a vault file by hand stays
//! in your file but never reaches the index.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;

use rusqlite::{Transaction, params};

use crate::limits::{
    DIGEST_TITLE_MAX_CHARS, INDEX_TEXT_MAX_BYTES, VAULT_FILE_MAX_BYTES, VAULT_ITEM_MAX_BYTES,
};
use crate::store::digest::{ParsedDigest, RelationKind, parse_digest};
use crate::store::error::StoreError;
use crate::store::ids::SessionId;
use crate::store::log::{LogSummary, summarize_log};
use crate::store::redact::{Redacted, redact_text};
use crate::store::time::WallTime;
use crate::store::vault::markdown::{
    cut_bytes, cut_chars, parse_item, split_front_matter, split_links,
};
use crate::store::vault::{
    BOARD_FILE, DIGEST_FILE, EVENTS_FILE, FileRead, MemoryFile, NOTES, PROJECTS, SESSIONS, Vault,
    VaultPath, read_text,
};

/// The title a session gets when nothing better is known.
pub(crate) const UNTITLED: &str = "Untitled session";

/// A group of files indexed together.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Unit {
    /// `sessions/<folder>/`: the log, the digest and the board make one session row.
    Session {
        folder: String,
    },
    Note(VaultPath),
    Project(VaultPath),
    Memory(VaultPath),
}

impl Unit {
    pub(crate) fn of(path: &VaultPath) -> Option<Self> {
        let is_markdown = |name: &str| name.ends_with(".md");
        match path.parts().as_slice() {
            [SESSIONS, folder, EVENTS_FILE | DIGEST_FILE | BOARD_FILE] => Some(Self::Session {
                folder: (*folder).to_owned(),
            }),
            [NOTES, name] if is_markdown(name) => Some(Self::Note(path.clone())),
            [PROJECTS, name] if is_markdown(name) => Some(Self::Project(path.clone())),
            _ => MemoryFile::of_path(path).map(|_| Self::Memory(path.clone())),
        }
    }

    fn paths(&self) -> Vec<VaultPath> {
        match self {
            Self::Session { folder } => [EVENTS_FILE, DIGEST_FILE, BOARD_FILE]
                .iter()
                .filter_map(|file| VaultPath::session_file(folder, file))
                .collect(),
            Self::Note(path) | Self::Project(path) | Self::Memory(path) => vec![path.clone()],
        }
    }
}

/// A file's identity for change detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileState {
    pub mtime_ms: i64,
    pub size_bytes: i64,
    pub hash: String,
}

/// Re-indexes one unit inside `transaction`.
pub(crate) fn index_unit(
    transaction: &Transaction<'_>,
    vault: &Vault,
    unit: &Unit,
) -> Result<(), StoreError> {
    let paths = unit.paths();
    for path in &paths {
        delete_rows(transaction, path)?;
    }
    if let Unit::Session { folder } = unit {
        transaction.execute("DELETE FROM sessions WHERE folder = ?1", [folder])?;
    }
    for path in &paths {
        match file_state(&vault.absolute(path))
            .map_err(|error| StoreError::io(path.as_str(), &error))?
        {
            Some(state) => {
                transaction.execute(
                    "INSERT INTO files (path, mtime_ms, size_bytes, hash) VALUES (?1, ?2, ?3, ?4)",
                    params![path.as_str(), state.mtime_ms, state.size_bytes, state.hash],
                )?;
            }
            None => continue,
        }
    }
    match unit {
        Unit::Session { folder } => index_session(transaction, vault, folder),
        Unit::Note(path) => index_note(transaction, vault, path),
        Unit::Project(path) => index_project(transaction, vault, path),
        Unit::Memory(path) => index_memory(transaction, vault, path),
    }
}

fn delete_rows(transaction: &Transaction<'_>, path: &VaultPath) -> Result<(), StoreError> {
    for table in [
        "files",
        "notes",
        "tasks",
        "memories",
        "proposals",
        "entities",
        "relations",
        "search",
    ] {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE path = ?1"),
            [path.as_str()],
        )?;
    }
    Ok(())
}

/// Reads a Markdown file for indexing; `None` when missing or too large (counted by the scan).
fn read_markdown(vault: &Vault, path: &VaultPath) -> Result<Option<String>, StoreError> {
    match read_text(&vault.absolute(path), VAULT_FILE_MAX_BYTES)
        .map_err(|error| StoreError::io(path.as_str(), &error))?
    {
        FileRead::Text { text, .. } => Ok(Some(text)),
        FileRead::Missing | FileRead::TooLarge { .. } => Ok(None),
    }
}

fn searchable(text: &str) -> Redacted {
    redact_text(&cut_bytes(text, INDEX_TEXT_MAX_BYTES))
}

fn insert_search(
    transaction: &Transaction<'_>,
    kind: &str,
    reference: &str,
    path: &VaultPath,
    title: &Redacted,
    body: &Redacted,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO search (title, body, kind, ref, path) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            title.as_str(),
            body.as_str(),
            kind,
            reference,
            path.as_str()
        ],
    )?;
    Ok(())
}

fn index_note(
    transaction: &Transaction<'_>,
    vault: &Vault,
    path: &VaultPath,
) -> Result<(), StoreError> {
    let Some(text) = read_markdown(vault, path)? else {
        return Ok(());
    };
    let (front, body) = split_front_matter(&text);
    let front = front.unwrap_or_default();
    let id = front
        .text("id")
        .map_or_else(|| path.as_str().to_owned(), str::to_owned);
    let title = body
        .lines()
        .map(|line| line.trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| path.stem());
    let title = redact_text(&cut_bytes(title, VAULT_ITEM_MAX_BYTES));
    let links = redact_text(&serde_json::to_string(&front.list("links")).unwrap_or_default());
    transaction.execute(
        "INSERT OR IGNORE INTO notes (id, path, title, created, session, links)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            path.as_str(),
            title.as_str(),
            front.text("created").unwrap_or_default(),
            front.text("session"),
            links.as_str()
        ],
    )?;
    insert_search(transaction, "note", &id, path, &title, &searchable(body))
}

fn index_project(
    transaction: &Transaction<'_>,
    vault: &Vault,
    path: &VaultPath,
) -> Result<(), StoreError> {
    let Some(text) = read_markdown(vault, path)? else {
        return Ok(());
    };
    let project = text
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .map_or_else(|| path.stem().to_owned(), |name| name.trim().to_owned());
    for (position, line) in text.lines().enumerate() {
        let Some(item) = parse_item(line) else {
            continue;
        };
        let Some(done) = item.checked else {
            continue;
        };
        let id = item
            .id
            .map_or_else(|| format!("{path}#{position}"), |id| id.to_string());
        let (title, links) = split_links(item.text);
        let session = links
            .iter()
            .find_map(|link| link.strip_prefix("session:"))
            .map(str::to_owned);
        let title = redact_text(title);
        let links = redact_text(&serde_json::to_string(&links).unwrap_or_default());
        transaction.execute(
            "INSERT OR IGNORE INTO tasks (id, path, position, title, project, done, links, session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                path.as_str(),
                i64::try_from(position).unwrap_or(i64::MAX),
                title.as_str(),
                project,
                done,
                links.as_str(),
                session
            ],
        )?;
        insert_search(
            transaction,
            "task",
            &id,
            path,
            &redact_text(&project),
            &title,
        )?;
    }
    Ok(())
}

fn index_memory(
    transaction: &Transaction<'_>,
    vault: &Vault,
    path: &VaultPath,
) -> Result<(), StoreError> {
    let Some(text) = read_markdown(vault, path)? else {
        return Ok(());
    };
    let file = MemoryFile::of_path(path).map_or("memory", MemoryFile::file_name);
    for (position, line) in text.lines().enumerate() {
        let Some(item) = parse_item(line) else {
            continue;
        };
        let id = item
            .id
            .map_or_else(|| format!("{path}#{position}"), |id| id.to_string());
        let memory = redact_text(item.text);
        transaction.execute(
            "INSERT OR IGNORE INTO memories (id, path, position, file, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                path.as_str(),
                i64::try_from(position).unwrap_or(i64::MAX),
                file,
                memory.as_str()
            ],
        )?;
        insert_search(
            transaction,
            "memory",
            &id,
            path,
            &redact_text(file),
            &memory,
        )?;
    }
    Ok(())
}

/// One session row from its folder's log, digest and board.
fn index_session(
    transaction: &Transaction<'_>,
    vault: &Vault,
    folder: &str,
) -> Result<(), StoreError> {
    let (Some(events), Some(digest_path), Some(board)) = (
        VaultPath::session_file(folder, EVENTS_FILE),
        VaultPath::session_file(folder, DIGEST_FILE),
        VaultPath::session_file(folder, BOARD_FILE),
    ) else {
        return Ok(());
    };
    let events_file = vault.absolute(&events);
    let log = if events_file.is_file() {
        Some(summarize_log(&events_file).map_err(|error| StoreError::io(events.as_str(), &error))?)
    } else {
        None
    };
    let digest = read_markdown(vault, &digest_path)?.map(|text| parse_digest(&text));
    let has_board = vault.absolute(&board).is_file();
    if log.is_none() && digest.is_none() && !has_board {
        return Ok(());
    }
    let id = digest
        .as_ref()
        .and_then(|digest| digest.id.clone())
        .or_else(|| log.as_ref().and_then(|log| log.id.clone()))
        .and_then(|id| SessionId::parse(&id))
        .or_else(|| SessionId::parse(folder));
    let Some(id) = id else {
        tracing::warn!(
            folder,
            "a session folder without a usable id is not indexed"
        );
        return Ok(());
    };
    let row = SessionRow::derive(log.as_ref(), digest.as_ref());
    let title = redact_text(&row.title);
    let inserted = row.insert(transaction, &id, folder, has_board, digest.is_some())?;
    if !inserted {
        tracing::warn!(folder, "another folder already holds this session id");
        return Ok(());
    }
    if let Some(log) = &log {
        let transcript = searchable(&log.transcript_text());
        insert_search(
            transaction,
            "transcript",
            id.as_str(),
            &events,
            &title,
            &transcript,
        )?;
    }
    if let Some(digest) = &digest {
        index_digest(transaction, &id, &digest_path, &title, digest)?;
    }
    Ok(())
}

/// The session row's values, from the best source for each.
struct SessionRow {
    title: String,
    summary: String,
    open_questions: String,
    started: String,
    started_ms: i64,
    duration_ms: i64,
}

impl SessionRow {
    fn derive(log: Option<&LogSummary>, digest: Option<&ParsedDigest>) -> Self {
        let started = log
            .and_then(|log| log.started.as_deref())
            .and_then(WallTime::parse_rfc3339)
            .or_else(|| digest.and_then(|digest| digest.started));
        let duration_ms = log
            .map(|log| log.last_t.as_ms())
            .or_else(|| digest.and_then(|digest| digest.duration_ms))
            .unwrap_or(0);
        let title = digest
            .and_then(|digest| digest.title.clone())
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                log.and_then(|log| log.first_user_line())
                    .map(|line| cut_chars(line, DIGEST_TITLE_MAX_CHARS))
            })
            .unwrap_or_else(|| UNTITLED.to_owned());
        Self {
            title,
            summary: digest
                .map(|digest| digest.summary.clone())
                .unwrap_or_default(),
            open_questions: digest
                .map(|digest| digest.open_questions.join("\n"))
                .unwrap_or_default(),
            started: started.map(|time| time.rfc3339()).unwrap_or_default(),
            started_ms: started.map_or(0, |time| time.unix_ms()),
            duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
        }
    }
}

impl SessionRow {
    /// Inserts the row; `false` when another folder already holds this session id.
    fn insert(
        &self,
        transaction: &Transaction<'_>,
        id: &SessionId,
        folder: &str,
        has_board: bool,
        has_digest: bool,
    ) -> Result<bool, StoreError> {
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO sessions (id, folder, title, started, started_ms, duration_ms,
                 has_board, has_digest, summary, open_questions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.as_str(),
                folder,
                redact_text(&self.title).as_str(),
                self.started,
                self.started_ms,
                self.duration_ms,
                has_board,
                has_digest,
                redact_text(&self.summary).as_str(),
                redact_text(&self.open_questions).as_str()
            ],
        )?;
        Ok(inserted > 0)
    }
}

fn index_digest(
    transaction: &Transaction<'_>,
    id: &SessionId,
    path: &VaultPath,
    title: &Redacted,
    digest: &ParsedDigest,
) -> Result<(), StoreError> {
    let mut body = format!("{}\n", digest.summary);
    for question in &digest.open_questions {
        body.push_str(question);
        body.push('\n');
    }
    for entity in &digest.entities {
        body.push_str(&entity.name);
        body.push('\n');
    }
    insert_search(
        transaction,
        "digest",
        id.as_str(),
        path,
        title,
        &searchable(&body),
    )?;
    for proposal in &digest.proposals {
        transaction.execute(
            "INSERT OR IGNORE INTO proposals (id, path, session, about, text, accepted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                proposal.id.to_string(),
                path.as_str(),
                id.as_str(),
                proposal.about.as_str(),
                redact_text(&proposal.text).as_str(),
                proposal.accepted
            ],
        )?;
    }
    for entity in &digest.entities {
        let name = redact_text(&entity.name);
        transaction.execute(
            "INSERT OR IGNORE INTO entities (name, kind, path, session) VALUES (?1, ?2, ?3, ?4)",
            params![
                name.as_str(),
                entity.kind.as_str(),
                path.as_str(),
                id.as_str()
            ],
        )?;
        // Every entity of a digest was mentioned in its session.
        insert_relation(
            transaction,
            &name,
            RelationKind::MentionedIn,
            id.as_str(),
            path,
            id,
        )?;
    }
    for relation in &digest.relations {
        let (subject, object) = (
            redact_text(&relation.subject),
            redact_text(&relation.object),
        );
        insert_relation(
            transaction,
            &subject,
            relation.relation,
            object.as_str(),
            path,
            id,
        )?;
    }
    Ok(())
}

fn insert_relation(
    transaction: &Transaction<'_>,
    subject: &Redacted,
    relation: RelationKind,
    object: &str,
    path: &VaultPath,
    session: &SessionId,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT OR IGNORE INTO relations (subject, relation, object, path, session)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            subject.as_str(),
            relation.as_str(),
            object,
            path.as_str(),
            session.as_str()
        ],
    )?;
    Ok(())
}

/// The modification time, size and content hash of a file, or `None` when it is missing.
pub(crate) fn file_state(path: &Path) -> io::Result<Option<FileState>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(None);
    }
    Ok(Some(FileState {
        mtime_ms: modified_ms(&metadata),
        size_bytes: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
        hash: content_hash(&mut file)?,
    }))
}

pub(crate) fn modified_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// FNV-1a over the file's bytes, as 16 hex digits. It detects changes; it is not a security
/// hash (nobody is attacking their own vault), and unlike `DefaultHasher` it is the same in
/// every build, so a toolchain update doesn't re-index everything.
fn content_hash(reader: &mut impl Read) -> io::Result<String> {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    /// Bytes read per call; any size works, this one keeps system calls few.
    const READ_CHUNK_BYTES: usize = 64 * 1024;
    let mut hash = OFFSET;
    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read] {
            hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
        }
    }
    Ok(format!("{hash:016x}"))
}

#[cfg(test)]
mod tests {
    use super::{Unit, content_hash};
    use crate::store::vault::VaultPath;

    #[test]
    fn units_follow_the_vault_layout() {
        let unit = |text: &str| VaultPath::parse(text).and_then(|path| Unit::of(&path));
        assert!(
            matches!(unit("sessions/s1/digest.md"), Some(Unit::Session { folder }) if folder == "s1")
        );
        assert!(matches!(unit("notes/a.md"), Some(Unit::Note(_))));
        assert!(matches!(unit("projects/inbox.md"), Some(Unit::Project(_))));
        assert!(matches!(unit("memory/facts.md"), Some(Unit::Memory(_))));
        for ignored in [
            "notes/a.txt",
            "memory/other.md",
            "sessions/s1/other.json",
            "readme.md",
        ] {
            assert_eq!(unit(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn the_content_hash_is_fnv1a() {
        let hash = |bytes: &[u8]| content_hash(&mut &bytes[..]).unwrap_or_default();
        assert_eq!(hash(b""), "cbf29ce484222325");
        assert_eq!(hash(b"a"), "af63dc4c8601ec8c");
        assert_ne!(hash(b"facts"), hash(b"fact5"));
    }
}
