//! The store's state and every write it makes. Each method is blocking (files, fsync,
//! SQLite) and runs on a blocking thread, called only by the store actor (law 3).
//!
//! A session moves `Idle → Recording → Ended → Idle` (the next start), or to `Forgotten` when
//! you say "don't remember this session". `Ended` keeps the session's captures so its digest
//! can be written, its proposals accepted, and the whole session still forgotten.
//!
//! Starting a session leaves the previous one behind before anything can fail: a start that
//! fails leaves `Idle`, never the previous session, so no later write or forget can reach it.

mod capture;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use boardai_core::time::SessionTime;
use rusqlite::Connection;

pub(crate) use capture::{Capture, CaptureKind, Proposal};

use super::digest::{
    Digest, DigestDocument, FallbackReason, ProposalLine, SessionMaterial, fallback_digest,
};
use super::error::StoreError;
use super::ids::{IdCounters, ItemKind, SessionId};
use super::index::views::sessions_without_digest;
use super::index::{Unit, index_units, scrub};
use super::log::{EventLogWriter, LogEntry, summarize_log};
use super::redact::{redact_json_text, redact_text};
use super::slug::{first_free, slugify};
use super::time::{WallClock, WallTime};
use super::vault::markdown::one_line;
use super::vault::{
    BOARD_FILE, DIGEST_FILE, EVENTS_FILE, SESSIONS, Vault, VaultPath, write_atomic,
};
use super::views::SessionInfo;
use crate::limits::{RECOVERED_SESSIONS_MAX, VAULT_ITEM_MAX_BYTES};

pub(crate) struct StoreState {
    pub vault: Vault,
    pub index: Connection,
    pub clock: Arc<dyn WallClock>,
    pub ids: IdCounters,
    pub slot: SessionSlot,
    /// Log entries the handle dropped because the inbox was full; recorded at the next flush.
    pub inbox_dropped: Arc<AtomicU64>,
    /// Set by the handle the moment "don't remember this session" is said, before the request
    /// itself arrives: the session then gets no digest whatever comes first (I10).
    pub forget_requested: Arc<AtomicBool>,
}

pub(crate) enum SessionSlot {
    Idle,
    Recording(Box<Session>),
    Ended(Box<Session>),
    /// Nothing more is written for this session: its log entries and memories are dropped
    /// until the next session starts.
    Forgotten,
}

pub(crate) struct Session {
    pub id: SessionId,
    /// The folder under `sessions/`; renamed once the digest gives a title.
    pub folder: String,
    pub started: WallTime,
    pub title_hint: Option<String>,
    pub log: Option<EventLogWriter>,
    pub duration_ms: u64,
    /// Everything this session wrote, in order, so it can be undone.
    pub captures: Vec<Capture>,
    /// Files this session created, with the header they were written with (redacted, as on
    /// disk): forgetting the session deletes them once only that header is left.
    pub created_files: Vec<(VaultPath, String)>,
    pub proposals: Vec<Proposal>,
    /// The digest once written, so accepting a proposal can re-render it.
    pub digest: Option<Digest>,
}

/// What ending a session gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEnded {
    /// The log is closed; the digest can be written from this.
    Material(Box<SessionMaterial>),
    /// The session was forgotten; there is nothing to digest.
    Forgotten,
}

impl StoreState {
    pub(crate) fn start_session(
        &mut self,
        title_hint: Option<String>,
    ) -> Result<SessionInfo, StoreError> {
        if matches!(self.slot, SessionSlot::Recording(_)) {
            return Err(StoreError::SessionActive);
        }
        if let Err(error) = self.ensure_digest() {
            tracing::warn!(%error, "the previous session was left without a digest");
        }
        // The previous session is left behind now: if this start fails, writes and forgets find
        // no session rather than the previous one.
        self.slot = SessionSlot::Idle;
        self.clear_forget_request();
        let started = self.clock.now();
        let base = format!("{}-{}", started.date(), started.time_of_day_compact());
        let sessions = self.vault.root().join(SESSIONS);
        let index = &self.index;
        let folder = first_free(&base, |name| {
            sessions.join(name).exists() || session_id_taken(index, name)
        })
        .ok_or(StoreError::Limit {
            what: "sessions started in the same second",
            max: crate::limits::VAULT_NAME_COLLISIONS_MAX as usize,
        })?;
        // `<date>-<time>[-n]` is always a valid id and path component; these only guard it.
        let id = SessionId::parse(&folder).ok_or_else(|| name_error(SESSIONS))?;
        let events =
            VaultPath::session_file(&folder, EVENTS_FILE).ok_or_else(|| name_error(SESSIONS))?;
        let events_file = self.vault.absolute(&events);
        let io_error = |error: std::io::Error| StoreError::io(events.as_str(), &error);
        if let Some(parent) = events_file.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let log = EventLogWriter::create(&events_file, &id, started).map_err(io_error)?;
        self.slot = SessionSlot::Recording(Box::new(Session {
            id: id.clone(),
            folder: folder.clone(),
            started,
            title_hint: title_hint.map(|hint| one_line(&hint, VAULT_ITEM_MAX_BYTES)),
            log: Some(log),
            duration_ms: 0,
            captures: Vec::new(),
            created_files: Vec::new(),
            proposals: Vec::new(),
            digest: None,
        }));
        Ok(SessionInfo {
            id,
            folder: format!("{}/{folder}", SESSIONS),
        })
    }

    /// Buffers a log entry (no I/O). Returns whether the buffer wants a flush now.
    pub(crate) fn log(&mut self, entry: LogEntry) -> bool {
        match &mut self.slot {
            SessionSlot::Recording(session) => {
                session.log.as_mut().is_some_and(|log| log.push(entry))
            }
            // Outside a session, and after "don't remember this session", nothing is logged.
            SessionSlot::Idle | SessionSlot::Ended(_) | SessionSlot::Forgotten => false,
        }
    }

    pub(crate) fn wants_flush(&self) -> bool {
        match &self.slot {
            SessionSlot::Recording(session) => {
                session
                    .log
                    .as_ref()
                    .is_some_and(EventLogWriter::has_pending)
                    || self.inbox_dropped.load(Ordering::Acquire) > 0
            }
            SessionSlot::Idle | SessionSlot::Ended(_) | SessionSlot::Forgotten => false,
        }
    }

    pub(crate) fn flush(&mut self) -> Result<(), StoreError> {
        let SessionSlot::Recording(session) = &mut self.slot else {
            return Ok(());
        };
        let Some(log) = session.log.as_mut() else {
            return Ok(());
        };
        let dropped = self.inbox_dropped.swap(0, Ordering::AcqRel);
        log.flush(dropped).map_err(|error| {
            let path = format!("sessions/{}/{EVENTS_FILE}", session.folder);
            StoreError::io(&path, &error)
        })
    }

    /// Whether "don't remember this session" was said and not yet carried out.
    fn forget_requested(&self) -> bool {
        self.forget_requested.load(Ordering::Acquire)
    }

    /// The request is carried out, or belongs to a session left behind.
    pub(crate) fn clear_forget_request(&self) {
        self.forget_requested.store(false, Ordering::Release);
    }

    /// Closes the log durably and gathers what the scribe needs. The board (the core's export)
    /// is written now, before the scribe runs, so a session that only gets its minimal digest
    /// later (quitting past its bound) keeps its board.
    pub(crate) fn end_session(
        &mut self,
        t_end: SessionTime,
        board_export: Option<String>,
    ) -> Result<SessionEnded, StoreError> {
        if self.forget_requested() {
            self.forget_session()?;
            return Ok(SessionEnded::Forgotten);
        }
        let mut session = match std::mem::replace(&mut self.slot, SessionSlot::Idle) {
            SessionSlot::Recording(session) => session,
            SessionSlot::Forgotten => {
                self.slot = SessionSlot::Forgotten;
                return Ok(SessionEnded::Forgotten);
            }
            other @ (SessionSlot::Idle | SessionSlot::Ended(_)) => {
                self.slot = other;
                return Err(StoreError::NoSession);
            }
        };
        let events_name = format!("{SESSIONS}/{}/{EVENTS_FILE}", session.folder);
        let closed = match session.log.take() {
            Some(log) => log
                .finish(t_end, self.inbox_dropped.swap(0, Ordering::AcqRel))
                .map(|stats| tracing::info!(?stats, "session log closed"))
                .map_err(|error| StoreError::io(&events_name, &error)),
            None => Ok(()),
        };
        let material = material(&self.vault, &mut session, t_end);
        let folder = session.folder.clone();
        self.slot = SessionSlot::Ended(session);
        // The session is never lost to a failed close: whatever reached the log is digested.
        if let Err(error) = closed {
            tracing::warn!(%error, "the session log did not close cleanly");
        }
        if let Some(export) = board_export {
            self.write_board(&folder, &export)?;
        }
        let material = material?;
        self.reindex(&[Unit::Session { folder }])?;
        Ok(SessionEnded::Material(Box::new(material)))
    }

    fn write_board(&self, folder: &str, export: &str) -> Result<(), StoreError> {
        let board = VaultPath::session_file(folder, BOARD_FILE).ok_or(StoreError::NoSession)?;
        write_atomic(&self.vault.absolute(&board), &redact_json_text(export))
            .map_err(|error| StoreError::io(board.as_str(), &error))
    }

    /// Writes the minimal digest for an ended session that has none: the app is quitting, or
    /// the next session started before a digest was written. A session is never left without
    /// one.
    pub(crate) fn ensure_digest(&mut self) -> Result<(), StoreError> {
        if self.forget_requested() {
            return match self.forget_session() {
                Ok(()) | Err(StoreError::NoSession) => Ok(()),
                Err(error) => Err(error),
            };
        }
        let SessionSlot::Ended(session) = &mut self.slot else {
            return Ok(());
        };
        if session.digest.is_some() {
            return Ok(());
        }
        let t_end = SessionTime::from_ms(session.duration_ms);
        let material = material(&self.vault, session, t_end)?;
        let digest = fallback_digest(&material, FallbackReason::Interrupted);
        self.write_digest(digest, None).map(|_| ())
    }

    /// Writes `board.excalidraw` and `digest.md`, renames the folder after the title, and
    /// indexes the session. Proposals from the digest join the session's own.
    pub(crate) fn write_digest(
        &mut self,
        digest: Digest,
        board_export: Option<String>,
    ) -> Result<SessionInfo, StoreError> {
        if self.forget_requested() {
            self.forget_session()?;
            return Err(StoreError::SessionForgotten);
        }
        let session = match &mut self.slot {
            SessionSlot::Ended(session) => session,
            SessionSlot::Forgotten => return Err(StoreError::SessionForgotten),
            SessionSlot::Idle | SessionSlot::Recording(_) => return Err(StoreError::NoSession),
        };
        for draft in &digest.proposals {
            let id = self.ids.take(ItemKind::Proposal);
            session
                .proposals
                .push(Proposal::new(id, draft.about, &draft.text));
        }
        let folder = session.folder.clone();
        session.digest = Some(digest);
        if let Some(export) = board_export {
            self.write_board(&folder, &export)?;
        }
        self.render_digest()?;
        let renamed = self.rename_after_title();
        let mut units = vec![Unit::Session {
            folder: folder.clone(),
        }];
        if let Some(new_folder) = &renamed {
            units.push(Unit::Session {
                folder: new_folder.clone(),
            });
        }
        self.reindex(&units)?;
        let SessionSlot::Ended(session) = &self.slot else {
            return Err(StoreError::NoSession);
        };
        Ok(SessionInfo {
            id: session.id.clone(),
            folder: format!("{}/{}", SESSIONS, session.folder),
        })
    }

    /// Writes the ended session's `digest.md` from its digest and current captures.
    pub(crate) fn render_digest(&mut self) -> Result<(), StoreError> {
        let SessionSlot::Ended(session) = &self.slot else {
            return Ok(());
        };
        let Some(digest) = &session.digest else {
            return Ok(());
        };
        let path =
            VaultPath::session_file(&session.folder, DIGEST_FILE).ok_or(StoreError::NoSession)?;
        let has_board = VaultPath::session_file(&session.folder, BOARD_FILE)
            .is_some_and(|board| self.vault.absolute(&board).is_file());
        let proposals: Vec<ProposalLine> = session.proposals.iter().map(Proposal::line).collect();
        let text = DigestDocument {
            id: &session.id,
            started: session.started,
            duration_ms: session.duration_ms,
            digest,
            has_board,
            captured: &session.captured_lines(),
            proposals: &proposals,
        }
        .render();
        write_atomic(&self.vault.absolute(&path), &redact_text(&text))
            .map_err(|error| StoreError::io(path.as_str(), &error))
    }

    /// Renames `sessions/<date>-<time>` to `sessions/<date>-<title slug>`. The session id does
    /// not change, so links stay valid; a failed rename keeps the old name. Returns the new
    /// folder.
    fn rename_after_title(&mut self) -> Option<String> {
        let sessions = self.vault.root().join(SESSIONS);
        let SessionSlot::Ended(session) = &mut self.slot else {
            return None;
        };
        // The folder is named after the redacted title: a secret never names a folder (I8).
        let slug = slugify(redact_text(&session.digest.as_ref()?.title).as_str())?;
        let base = format!("{}-{slug}", session.started.date());
        if base == session.folder {
            return None;
        }
        let target = first_free(&base, |name| sessions.join(name).exists())?;
        match std::fs::rename(sessions.join(&session.folder), sessions.join(&target)) {
            Ok(()) => {
                super::vault::sync_folder(&sessions);
                session.folder = target.clone();
                Some(target)
            }
            Err(error) => {
                tracing::warn!(error = %error.kind(), "the session folder keeps its first name");
                None
            }
        }
    }

    /// Sessions a crash or a kill left without a digest get the minimal one when the store
    /// opens, so every session in the library has one (LLD §3.6). One nobody spoke in leaves
    /// nothing, as it would have had it closed. At most
    /// [`RECOVERED_SESSIONS_MAX`] per open, oldest first; one that fails is logged and tried
    /// again at the next open.
    pub(crate) fn recover_interrupted(&mut self) {
        let found = match sessions_without_digest(&self.index, RECOVERED_SESSIONS_MAX) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "sessions without a digest could not be listed");
                return;
            }
        };
        for (id, folder, started, duration_ms) in found {
            let Some(id) = SessionId::parse(&id) else {
                continue;
            };
            let started = WallTime::parse_rfc3339(&started).unwrap_or_else(|| self.clock.now());
            self.slot = SessionSlot::Ended(Box::new(Session {
                id,
                folder,
                started,
                title_hint: None,
                log: None,
                duration_ms,
                captures: Vec::new(),
                created_files: Vec::new(),
                proposals: Vec::new(),
                digest: None,
            }));
            let recovered = if self.nobody_spoke_in_the_ended_session() {
                self.forget_session()
            } else {
                self.ensure_digest()
            };
            if let Err(error) = recovered {
                tracing::warn!(%error, "an interrupted session is still without a digest");
            }
            self.slot = SessionSlot::Idle;
        }
    }

    /// Whether the ended session in the slot is one nobody spoke in that holds nothing but its
    /// log, as a silent session killed before it could close leaves it (LLD §3.6: such a session
    /// leaves nothing). A folder with anything else in it is kept and digested.
    fn nobody_spoke_in_the_ended_session(&self) -> bool {
        let SessionSlot::Ended(session) = &self.slot else {
            return false;
        };
        let Some(folder) = VaultPath::session_folder(&session.folder) else {
            return false;
        };
        let only_the_log = std::fs::read_dir(self.vault.absolute(&folder)).is_ok_and(|entries| {
            entries
                .flatten()
                .all(|entry| entry.file_name().to_str() == Some(EVENTS_FILE))
        });
        let events = self.vault.absolute(&folder).join(EVENTS_FILE);
        only_the_log
            && summarize_log(&events).is_ok_and(|summary| summary.first_user_line().is_none())
    }

    /// Closes a session still recording at shutdown and gives it the minimal digest. The app
    /// finishes a session (with the scribe) before shutting down when it can.
    pub(crate) fn close_for_shutdown(&mut self) -> Result<(), StoreError> {
        if matches!(self.slot, SessionSlot::Recording(_)) && !self.forget_requested() {
            // The end line takes the last time logged; session time has no later reading here.
            self.end_session(SessionTime::ZERO, None)?;
        }
        self.ensure_digest()
    }

    /// Re-indexes units in one transaction. A unit whose files can't be read is left out
    /// (logged); the next scan tries it again.
    pub(crate) fn reindex(&mut self, units: &[Unit]) -> Result<(), StoreError> {
        let mut transaction = self.index.transaction()?;
        index_units(&mut transaction, &self.vault, units)?;
        transaction.commit()?;
        Ok(())
    }

    /// Leaves nothing readable of what was just deleted in the index's files (I10).
    pub(crate) fn scrub_index(&self) -> Result<(), StoreError> {
        scrub(&self.index)
    }
}

impl Session {
    /// One line per capture, for the digest and the scribe's prompt: the items first, then
    /// what was kept with the session record.
    pub(crate) fn captured_lines(&self) -> Vec<String> {
        let is_record = |capture: &&Capture| matches!(capture.kind, CaptureKind::SessionRecord);
        let items = self.captures.iter().filter(|capture| !is_record(capture));
        let records = self.captures.iter().filter(is_record);
        items
            .chain(records)
            .filter_map(Capture::digest_line)
            .collect()
    }
}

/// Gathers what a digest is made from, from the session's closed log.
fn material(
    vault: &Vault,
    session: &mut Session,
    t_end: SessionTime,
) -> Result<SessionMaterial, StoreError> {
    let events = VaultPath::session_file(&session.folder, EVENTS_FILE)
        .ok_or_else(|| name_error(SESSIONS))?;
    let summary = summarize_log(&vault.absolute(&events))
        .map_err(|error| StoreError::io(events.as_str(), &error))?;
    session.duration_ms = session
        .duration_ms
        .max(summary.last_t.as_ms())
        .max(t_end.as_ms());
    Ok(SessionMaterial {
        id: session.id.clone(),
        title_hint: session.title_hint.clone(),
        started: session.started,
        duration_ms: session.duration_ms,
        transcript: summary.transcript,
        captured: session.captured_lines(),
    })
}

pub(crate) fn name_error(folder: &str) -> StoreError {
    StoreError::Io {
        path: folder.to_owned(),
        detail: "no usable file name".to_owned(),
    }
}

fn session_id_taken(index: &Connection, id: &str) -> bool {
    index
        .query_row("SELECT 1 FROM sessions WHERE id = ?1", [id], |_| Ok(()))
        .is_ok()
}
