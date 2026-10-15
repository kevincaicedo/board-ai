//! Memory writes (LLD §2.8, PRODUCT §7.4). The memory policy in the core has already decided
//! each write's action; the store carries it out and records it as a capture, so "forget that"
//! can undo the last one and "don't remember this session" can undo them all (I10).
//!
//! | Kind | Kept in |
//! |---|---|
//! | Learning signal | `memory/about-you.md` |
//! | Stated, inferred (once accepted) | `memory/about-you.md` or `memory/facts.md`, by subject |
//! | Note | `notes/<slug>.md` |
//! | Task | `projects/<project>.md` (`inbox` without one) |
//! | Session record | The session's digest |
//! | Raw audio, secret, or `Never` | Nowhere: refused |

use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite};
use rusqlite::OptionalExtension;

use super::{Session, SessionSlot, StoreState, name_error};
use crate::limits::{
    SESSION_CAPTURES_MAX, VAULT_FILE_MAX_BYTES, VAULT_ITEM_MAX_BYTES, VAULT_LINKS_MAX,
    VAULT_NOTE_MAX_BYTES,
};
use crate::store::digest::{About, ProposalLine};
use crate::store::error::{Refusal, StoreError};
use crate::store::ids::{ItemId, ItemKind};
use crate::store::index::Unit;
use crate::store::redact::redact_text;
use crate::store::slug::{first_free, slugify};
use crate::store::vault::markdown::{
    FrontValue, append_line, cut_bytes, one_line, remove_item, render_front_matter, render_item,
    set_checked,
};
use crate::store::vault::{
    FileRead, INBOX_PROJECT, MemoryFile, VaultPath, read_text, remove_file, write_atomic,
};
use crate::store::views::{Forgotten, ForgottenKind, Remembered};

const ABOUT_YOU_HEADER: &str = "# About you\n\nHow you like to learn and what you've covered. \
Edit freely; keep the ^ids at the ends of lines.\n\n";
const FACTS_HEADER: &str = "# Facts\n\nWhat you told BoardAI about your systems. Edit freely; \
keep the ^ids at the ends of lines.\n\n";
const INBOX_NAME: &str = "Inbox";

/// Words that make a stated or inferred memory about you rather than your systems.
const ABOUT_YOU_WORDS: &[&str] = &[
    "i",
    "i'm",
    "i've",
    "i'd",
    "me",
    "my",
    "mine",
    "myself",
    "prefer",
    "prefers",
    "preferred",
    "like",
    "likes",
    "learn",
    "learning",
    "pace",
    "slower",
    "faster",
    "struggle",
    "struggled",
    "struggles",
    "confused",
    "explain",
    "examples",
];

/// One thing a session wrote, with what is needed to undo it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Capture {
    pub kind: CaptureKind,
    /// The text kept, for the digest and for saying what was forgotten.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Memory {
        id: ItemId,
        file: MemoryFile,
        added_newline: bool,
        source: MemorySource,
    },
    Note {
        id: ItemId,
        path: VaultPath,
    },
    Task {
        id: ItemId,
        path: VaultPath,
        added_newline: bool,
    },
    SessionRecord,
    Proposal {
        id: ItemId,
    },
}

/// Where a kept memory came from, so undoing it also un-accepts its proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemorySource {
    Direct,
    /// A proposal of this session.
    Proposal(ItemId),
    /// A proposal in an earlier session's digest.
    PastProposal {
        digest: VaultPath,
        id: ItemId,
    },
}

/// A memory waiting for a yes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub id: ItemId,
    pub about: About,
    pub text: String,
    pub accepted: bool,
}

impl Proposal {
    pub(crate) fn new(id: ItemId, about: About, text: &str) -> Self {
        Self {
            id,
            about,
            text: one_line(text, VAULT_ITEM_MAX_BYTES),
            accepted: false,
        }
    }

    pub(crate) fn line(&self) -> ProposalLine {
        ProposalLine {
            id: self.id,
            about: self.about,
            text: self.text.clone(),
            accepted: self.accepted,
        }
    }
}

impl Capture {
    pub(crate) fn digest_line(&self) -> Option<String> {
        match &self.kind {
            CaptureKind::Memory { id, file, .. } => Some(format!(
                "Remembered: {} (memory/{} ^{id})",
                self.text,
                file.file_name()
            )),
            CaptureKind::Note { path, .. } => Some(format!("Note: {} ({path})", self.text)),
            CaptureKind::Task { id, path, .. } => {
                Some(format!("Task: {} ({path} ^{id})", self.text))
            }
            // Records are listed from the session; proposals have their own section.
            CaptureKind::SessionRecord | CaptureKind::Proposal { .. } => None,
        }
    }
}

/// Which memory file a stated or inferred memory goes to. Learning signals are always about
/// you; otherwise first-person and learning words decide, and the rest is about your systems.
pub(crate) fn about_for(kind: MemoryKind, text: &str) -> About {
    if kind == MemoryKind::LearningSignal {
        return About::You;
    }
    let lower = text.to_lowercase();
    let about_you = lower
        .split(|character: char| !(character.is_alphanumeric() || character == '\''))
        .any(|word| ABOUT_YOU_WORDS.contains(&word));
    if about_you {
        About::You
    } else {
        About::Systems
    }
}

const fn memory_file(about: About) -> MemoryFile {
    match about {
        About::You => MemoryFile::AboutYou,
        About::Systems => MemoryFile::Facts,
    }
}

const fn header(file: MemoryFile) -> &'static str {
    match file {
        MemoryFile::AboutYou => ABOUT_YOU_HEADER,
        MemoryFile::Facts => FACTS_HEADER,
    }
}

/// Refuses what must never be stored, whatever the action says (defense in depth: the
/// policy never sends these).
fn refusal(write: &MemoryWrite) -> Result<(), StoreError> {
    let reason = match (write.kind, write.action) {
        (MemoryKind::RawAudio, _) => Refusal::RawAudio,
        (MemoryKind::Secret, _) => Refusal::Secret,
        (_, MemoryAction::Never) => Refusal::NeverAction,
        (
            MemoryKind::SessionRecord
            | MemoryKind::LearningSignal
            | MemoryKind::Stated
            | MemoryKind::Inferred
            | MemoryKind::Note
            | MemoryKind::Task,
            MemoryAction::Keep | MemoryAction::Propose | MemoryAction::OnRequest,
        ) => return Ok(()),
    };
    Err(StoreError::Refused { reason })
}

impl StoreState {
    /// Carries out one memory write as the policy decided it.
    pub(crate) fn remember(&mut self, write: MemoryWrite) -> Result<Remembered, StoreError> {
        refusal(&write)?;
        let session = self.session_mut()?;
        if session.captures.len() >= SESSION_CAPTURES_MAX {
            return Err(StoreError::Limit {
                what: "items captured in one session",
                max: SESSION_CAPTURES_MAX,
            });
        }
        match write.action {
            MemoryAction::Propose => Ok(self.propose(&write)),
            MemoryAction::Keep | MemoryAction::OnRequest => match write.kind {
                MemoryKind::Note => self.write_note(&write),
                MemoryKind::Task => self.add_task(&write),
                MemoryKind::SessionRecord => {
                    let text = one_line(
                        &redact_text(&write.text).into_string(),
                        VAULT_ITEM_MAX_BYTES,
                    );
                    let session = self.session_mut()?;
                    session.records.push(text.clone());
                    session.captures.push(Capture {
                        kind: CaptureKind::SessionRecord,
                        text,
                    });
                    self.refresh_digest()?;
                    Ok(Remembered::SessionRecord)
                }
                MemoryKind::LearningSignal | MemoryKind::Stated | MemoryKind::Inferred => {
                    let about = about_for(write.kind, &write.text);
                    self.keep_memory(about, &write.text, MemorySource::Direct)
                }
                MemoryKind::RawAudio => Err(StoreError::Refused {
                    reason: Refusal::RawAudio,
                }),
                MemoryKind::Secret => Err(StoreError::Refused {
                    reason: Refusal::Secret,
                }),
            },
            MemoryAction::Never => Err(StoreError::Refused {
                reason: Refusal::NeverAction,
            }),
        }
    }

    pub(crate) fn session_mut(&mut self) -> Result<&mut Session, StoreError> {
        match &mut self.slot {
            SessionSlot::Recording(session) | SessionSlot::Ended(session) => Ok(session),
            SessionSlot::Forgotten => Err(StoreError::SessionForgotten),
            SessionSlot::Idle => Err(StoreError::NoSession),
        }
    }

    fn propose(&mut self, write: &MemoryWrite) -> Remembered {
        let about = about_for(write.kind, &write.text);
        let id = self.ids.take(ItemKind::Proposal);
        let text = redact_text(&write.text).into_string();
        if let Ok(session) = self.session_mut() {
            let proposal = Proposal::new(id, about, &text);
            session.captures.push(Capture {
                kind: CaptureKind::Proposal { id },
                text: proposal.text.clone(),
            });
            session.proposals.push(proposal);
        }
        if let Err(error) = self.refresh_digest() {
            tracing::warn!(%error, "the digest was not updated with the new proposal");
        }
        Remembered::Proposed { id }
    }

    fn write_note(&mut self, write: &MemoryWrite) -> Result<Remembered, StoreError> {
        let text = cut_bytes(write.text.trim(), VAULT_NOTE_MAX_BYTES);
        let first_line = one_line(
            text.lines().next().unwrap_or_default(),
            VAULT_ITEM_MAX_BYTES,
        );
        let notes = self.vault.root().join(crate::store::vault::NOTES);
        let slug = slugify(&first_line).unwrap_or_else(|| "note".to_owned());
        let name = first_free(&slug, |name| notes.join(format!("{name}.md")).exists())
            .ok_or_else(|| name_error("notes"))?;
        let path = VaultPath::note(&name).ok_or_else(|| name_error("notes"))?;
        let id = self.ids.take(ItemKind::Note);
        let session_id = self.session_mut()?.id.to_string();
        let links = bounded_links(&write.links);
        let content = format!(
            "{}{text}\n",
            render_front_matter(&[
                ("id", FrontValue::Text(id.to_string())),
                ("created", FrontValue::Text(self.clock.now().rfc3339())),
                ("session", FrontValue::Text(session_id)),
                ("links", FrontValue::List(links)),
            ])
        );
        let redacted = redact_text(&content);
        write_atomic(&self.vault.absolute(&path), &redacted)
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        self.session_mut()?.captures.push(Capture {
            kind: CaptureKind::Note {
                id,
                path: path.clone(),
            },
            text: redact_text(&first_line).into_string(),
        });
        self.reindex(&[Unit::Note(path.clone())])?;
        self.refresh_digest()?;
        Ok(Remembered::Note {
            id,
            path: path.to_string(),
        })
    }

    fn add_task(&mut self, write: &MemoryWrite) -> Result<Remembered, StoreError> {
        let project = write
            .project
            .as_deref()
            .map(|name| one_line(name, VAULT_ITEM_MAX_BYTES))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| INBOX_NAME.to_owned());
        let slug = slugify(&project).unwrap_or_else(|| INBOX_PROJECT.to_owned());
        let path = VaultPath::project(&slug).ok_or_else(|| name_error("projects"))?;
        let id = self.ids.take(ItemKind::Task);
        let title = one_line(&write.text, VAULT_ITEM_MAX_BYTES);
        let mut links = vec![format!("session:{}", self.session_mut()?.id)];
        links.extend(
            bounded_links(&write.links)
                .into_iter()
                .take(VAULT_LINKS_MAX - 1),
        );
        let line = render_item(Some(false), &title, &links, Some(id));
        let added_newline = self.append_line_to(&path, &format!("# {project}\n\n"), &line)?;
        self.session_mut()?.captures.push(Capture {
            kind: CaptureKind::Task {
                id,
                path: path.clone(),
                added_newline,
            },
            text: redact_text(&title).into_string(),
        });
        self.reindex(&[Unit::Project(path.clone())])?;
        self.refresh_digest()?;
        Ok(Remembered::Task {
            id,
            project,
            path: path.to_string(),
        })
    }

    /// Appends a memory line to its file. The capture is recorded when a session is open.
    fn keep_memory(
        &mut self,
        about: About,
        text: &str,
        source: MemorySource,
    ) -> Result<Remembered, StoreError> {
        let file = memory_file(about);
        let path = VaultPath::memory(file);
        let id = self.ids.take(ItemKind::Memory);
        let text = one_line(text, VAULT_ITEM_MAX_BYTES);
        let line = render_item(None, &text, &[], Some(id));
        let added_newline = self.append_line_to(&path, header(file), &line)?;
        if let Ok(session) = self.session_mut() {
            session.captures.push(Capture {
                kind: CaptureKind::Memory {
                    id,
                    file,
                    added_newline,
                    source,
                },
                text: redact_text(&text).into_string(),
            });
        }
        self.reindex(&[Unit::Memory(path)])?;
        self.refresh_digest()?;
        Ok(Remembered::Memory {
            id,
            file: file.into(),
        })
    }

    /// Keeps a proposal after your yes: one from this session, or from an earlier digest.
    pub(crate) fn accept_proposal(&mut self, id: ItemId) -> Result<Remembered, StoreError> {
        if matches!(self.slot, SessionSlot::Forgotten) {
            return Err(StoreError::SessionForgotten);
        }
        let open = |what: ItemId| StoreError::NotFound {
            what: format!("an open proposal {what}"),
        };
        if let Ok(session) = self.session_mut()
            && let Some(proposal) = session
                .proposals
                .iter_mut()
                .find(|proposal| proposal.id == id)
        {
            if proposal.accepted {
                return Err(open(id));
            }
            proposal.accepted = true;
            let (about, text) = (proposal.about, proposal.text.clone());
            return self.keep_memory(about, &text, MemorySource::Proposal(id));
        }
        let row: Option<(String, String, String, bool)> = self
            .index
            .query_row(
                "SELECT path, about, text, accepted FROM proposals WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((path, about, text, false)) = row else {
            return Err(open(id));
        };
        let digest = VaultPath::parse(&path).ok_or_else(|| open(id))?;
        let content = self.read_editable(&digest)?.ok_or_else(|| open(id))?;
        let checked = set_checked(&content, id, true).ok_or_else(|| open(id))?;
        write_atomic(&self.vault.absolute(&digest), &redact_text(&checked))
            .map_err(|error| StoreError::io(digest.as_str(), &error))?;
        let about = About::parse(&about).unwrap_or(About::Systems);
        let source = MemorySource::PastProposal {
            digest: digest.clone(),
            id,
        };
        let remembered = self.keep_memory(about, &text, source)?;
        self.reindex(&Unit::of(&digest).into_iter().collect::<Vec<_>>())?;
        Ok(remembered)
    }

    /// "Forget that": undoes the last capture of this session.
    pub(crate) fn forget_last(&mut self) -> Result<Option<Forgotten>, StoreError> {
        let mut session = match std::mem::replace(&mut self.slot, SessionSlot::Idle) {
            SessionSlot::Recording(session) | SessionSlot::Ended(session) => session,
            other @ SessionSlot::Forgotten => {
                self.slot = other;
                return Ok(None);
            }
            SessionSlot::Idle => return Err(StoreError::NoSession),
        };
        let recording = session.log.is_some();
        let result = match session.captures.pop() {
            None => Ok(None),
            Some(capture) => match self.undo(&mut session, &capture) {
                Ok(units) => self.reindex(&units).map(|()| Some(forgotten(&capture))),
                Err(error) => {
                    session.captures.push(capture);
                    Err(error)
                }
            },
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
    /// and its index rows. Nothing is written for the session afterwards.
    pub(crate) fn forget_session(&mut self) -> Result<(), StoreError> {
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
                Ok(()) => crate::store::vault::sync_folder(
                    &self.vault.root().join(crate::store::vault::SESSIONS),
                ),
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
                            && let Some(unchecked) = set_checked(&content, *id, false)
                        {
                            write_atomic(&self.vault.absolute(digest), &redact_text(&unchecked))
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
            CaptureKind::SessionRecord => {
                session.records.pop();
                Ok(Vec::new())
            }
            CaptureKind::Proposal { id } => {
                session.proposals.retain(|proposal| proposal.id != *id);
                Ok(Vec::new())
            }
        }
    }

    /// Appends a line, creating the file with `header` when missing. Returns whether a newline
    /// had to be added before the line.
    fn append_line_to(
        &mut self,
        path: &VaultPath,
        header: &str,
        line: &str,
    ) -> Result<bool, StoreError> {
        let (content, created) = match self.read_editable(path)? {
            Some(content) => (content, false),
            None => (header.to_owned(), true),
        };
        let (updated, added_newline) = append_line(&content, line);
        write_atomic(&self.vault.absolute(path), &redact_text(&updated))
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        if created && let Ok(session) = self.session_mut() {
            session
                .created_files
                .push((path.clone(), header.to_owned()));
        }
        Ok(added_newline)
    }

    /// Removes an item line; a file this session created is deleted once only its header is
    /// left, so forgetting leaves the vault as it was.
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
        let Some((updated, _removed)) = remove_item(&content, id, added_newline) else {
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
            write_atomic(&absolute, &redact_text(&updated)).map_err(io_error)?;
        }
        Ok(())
    }

    /// Reads a vault file the store may rewrite: `None` when missing; an error when it is too
    /// large or not UTF-8, so a hand-made file is never mangled by an edit.
    fn read_editable(&self, path: &VaultPath) -> Result<Option<String>, StoreError> {
        let read = read_text(&self.vault.absolute(path), VAULT_FILE_MAX_BYTES)
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        match read {
            FileRead::Missing => Ok(None),
            FileRead::Text { text, lossy: false } => Ok(Some(text)),
            FileRead::Text { lossy: true, .. } | FileRead::TooLarge { .. } => Err(StoreError::Io {
                path: path.to_string(),
                detail: "the file is too large or not UTF-8 text, so it is left as it is"
                    .to_owned(),
            }),
        }
    }

    /// Re-renders the digest when the session has one, so it lists the current captures.
    fn refresh_digest(&mut self) -> Result<(), StoreError> {
        let SessionSlot::Ended(session) = &self.slot else {
            return Ok(());
        };
        if session.digest.is_none() {
            return Ok(());
        }
        let folder = session.folder.clone();
        self.render_digest()?;
        self.reindex(&[Unit::Session { folder }])
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

fn bounded_links(links: &[String]) -> Vec<String> {
    links
        .iter()
        .map(|link| one_line(link, VAULT_ITEM_MAX_BYTES))
        .filter(|link| !link.is_empty())
        .take(VAULT_LINKS_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use boardai_core::memory::MemoryKind;

    use super::about_for;
    use crate::store::digest::About;

    #[test]
    fn memories_go_to_the_file_of_their_subject() {
        for (kind, text, about) in [
            (MemoryKind::Stated, "I prefer Go examples", About::You),
            (
                MemoryKind::Stated,
                "We use Kafka for the event bus",
                About::Systems,
            ),
            (MemoryKind::Inferred, "Likes a slower pace", About::You),
            (
                MemoryKind::Inferred,
                "Service X uses cache-aside",
                About::Systems,
            ),
            (
                MemoryKind::LearningSignal,
                "Covered Raft log matching",
                About::You,
            ),
            (
                MemoryKind::Stated,
                "Our cluster has 5 nodes",
                About::Systems,
            ),
        ] {
            assert_eq!(about_for(kind, text), about, "{text}");
        }
    }
}
