//! Undoing captures: "forget that" (one capture, named by the core's id) and "don't remember this
//! session" (all of them, I10). A capture that can't be undone stays recorded, so the digest
//! still lists what is still in the vault and forgetting the session still removes it.

use boardai_core::ids::CaptureId;

use super::{Capture, CaptureKind, MemorySource};
use crate::store::error::StoreError;
use crate::store::ids::ItemId;
use crate::store::index::Unit;
use crate::store::state::{Session, SessionSlot, StoreState};
use crate::store::vault::edit::Edited;
use crate::store::vault::{SESSIONS, VaultPath, remove_file, sync_folder, write_atomic};
use crate::store::views::{Forgotten, ForgottenKind};

impl StoreState {
    /// "Forget that" with no capture named: undoes the newest capture of this session.
    pub(crate) fn forget_last(&mut self) -> Result<Option<Forgotten>, StoreError> {
        self.forget_where(|captures| captures.len().checked_sub(1))
    }

    /// "Forget that" for the capture the core made as `capture`: the newest one carrying it (a
    /// keep before its proposal). `None` when the store holds no such capture: the write never
    /// reached the vault, so there is nothing to undo.
    pub(crate) fn forget_capture(
        &mut self,
        capture: CaptureId,
    ) -> Result<Option<Forgotten>, StoreError> {
        self.forget_where(|captures| {
            captures
                .iter()
                .rposition(|kept| kept.capture == Some(capture))
        })
    }

    fn forget_where(
        &mut self,
        find: impl FnOnce(&[Capture]) -> Option<usize>,
    ) -> Result<Option<Forgotten>, StoreError> {
        let mut session = match std::mem::replace(&mut self.slot, SessionSlot::Idle) {
            SessionSlot::Recording(session) | SessionSlot::Ended(session) => session,
            other @ SessionSlot::Forgotten => {
                self.slot = other;
                return Ok(None);
            }
            SessionSlot::Idle => return Err(StoreError::NoSession),
        };
        let recording = session.log.is_some();
        let result = match find(&session.captures) {
            None => Ok(None),
            Some(index) => {
                let capture = session.captures.remove(index);
                match self.undo(&mut session, &capture) {
                    Ok(units) => self.reindex(&units).map(|()| Some(forgotten(&capture))),
                    Err(error) => {
                        // Still in the vault: still recorded.
                        session.captures.insert(index, capture);
                        Err(error)
                    }
                }
            }
        };
        self.slot = if recording {
            SessionSlot::Recording(session)
        } else {
            SessionSlot::Ended(session)
        };
        self.refresh_digest()?;
        result
    }

    /// "Don't remember this session" (I10): undoes every capture, deletes the session folder
    /// and its index rows, and scrubs the index files. Nothing is written for the session
    /// afterwards.
    pub(crate) fn forget_session(&mut self) -> Result<(), StoreError> {
        self.clear_forget_request();
        let mut session = match std::mem::replace(&mut self.slot, SessionSlot::Idle) {
            SessionSlot::Recording(session) | SessionSlot::Ended(session) => session,
            other @ SessionSlot::Forgotten => {
                self.slot = other;
                return Ok(());
            }
            SessionSlot::Idle => return Err(StoreError::NoSession),
        };
        self.slot = SessionSlot::Forgotten;
        // Buffered log lines are dropped unwritten.
        drop(session.log.take());
        let mut first_error = None;
        let mut units = Vec::new();
        let captures = std::mem::take(&mut session.captures);
        for capture in captures.iter().rev() {
            match self.undo(&mut session, capture) {
                Ok(touched) => units.extend(touched),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(folder) = VaultPath::session_folder(&session.folder) {
            match std::fs::remove_dir_all(self.vault.absolute(&folder)) {
                Ok(()) => sync_folder(&self.vault.root().join(SESSIONS)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    first_error.get_or_insert(StoreError::io(folder.as_str(), &error));
                }
            }
        }
        units.push(Unit::Session {
            folder: session.folder.clone(),
        });
        units.sort();
        units.dedup();
        self.reindex(&units)?;
        self.scrub_index()?;
        first_error.map_or(Ok(()), Err)
    }

    /// Undoes one capture's file changes. Returns the units to re-index.
    fn undo(&mut self, session: &mut Session, capture: &Capture) -> Result<Vec<Unit>, StoreError> {
        match &capture.kind {
            CaptureKind::Memory {
                id,
                file,
                added_newline,
                source,
            } => {
                let path = VaultPath::memory(*file);
                self.remove_line_from(session, &path, *id, *added_newline)?;
                let mut units = vec![Unit::Memory(path)];
                match source {
                    MemorySource::Direct => {}
                    MemorySource::Proposal(proposal) => {
                        if let Some(open) = session
                            .proposals
                            .iter_mut()
                            .find(|open| open.id == *proposal)
                        {
                            open.accepted = false;
                        }
                    }
                    MemorySource::PastProposal { digest, id } => {
                        if let Some(content) = self.read_editable(digest)?
                            && let Some(unchecked) = Edited::set_checked(&content, *id, false)
                        {
                            write_atomic(&self.vault.absolute(digest), &unchecked)
                                .map_err(|error| StoreError::io(digest.as_str(), &error))?;
                        }
                        units.extend(Unit::of(digest));
                    }
                }
                Ok(units)
            }
            CaptureKind::Note { path, .. } => {
                remove_file(&self.vault.absolute(path))
                    .map_err(|error| StoreError::io(path.as_str(), &error))?;
                Ok(vec![Unit::Note(path.clone())])
            }
            CaptureKind::Task {
                id,
                path,
                added_newline,
            } => {
                self.remove_line_from(session, path, *id, *added_newline)?;
                Ok(vec![Unit::Project(path.clone())])
            }
            // The record lives in the capture list itself, which the digest is rendered from.
            CaptureKind::SessionRecord => Ok(Vec::new()),
            CaptureKind::Proposal { id } => {
                session.proposals.retain(|proposal| proposal.id != *id);
                Ok(Vec::new())
            }
        }
    }

    /// Removes an item line by its id, leaving every other byte; a file this session created is
    /// deleted once only its header is left, so forgetting leaves the vault as it was.
    fn remove_line_from(
        &self,
        session: &Session,
        path: &VaultPath,
        id: ItemId,
        added_newline: bool,
    ) -> Result<(), StoreError> {
        let Some(content) = self.read_editable(path)? else {
            return Ok(());
        };
        let Some(updated) = Edited::remove(&content, id, added_newline) else {
            return Ok(());
        };
        let absolute = self.vault.absolute(path);
        let created_header = session
            .created_files
            .iter()
            .find(|(created, _)| created == path)
            .map(|(_, header)| header.as_str());
        let io_error = |error: std::io::Error| StoreError::io(path.as_str(), &error);
        if created_header == Some(updated.as_str()) {
            remove_file(&absolute).map_err(io_error)?;
        } else {
            write_atomic(&absolute, &updated).map_err(io_error)?;
        }
        Ok(())
    }
}

fn forgotten(capture: &Capture) -> Forgotten {
    let what = match capture.kind {
        CaptureKind::Memory { .. } => ForgottenKind::Memory,
        CaptureKind::Note { .. } => ForgottenKind::Note,
        CaptureKind::Task { .. } => ForgottenKind::Task,
        CaptureKind::SessionRecord => ForgottenKind::SessionRecord,
        CaptureKind::Proposal { .. } => ForgottenKind::Proposal,
    };
    Forgotten {
        what,
        text: capture.text.clone(),
    }
}
