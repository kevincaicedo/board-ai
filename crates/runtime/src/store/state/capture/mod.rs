//! Memory writes (LLD §2.8, PRODUCT §7.4). The memory policy in the core has already decided
//! each write's action; the store carries it out and records it as a capture, so "forget that"
//! can undo it and "don't remember this session" can undo them all (I10).
//!
//! | Kind | Kept in |
//! |---|---|
//! | Learning signal | `memory/about-you.md` |
//! | Stated, inferred (once accepted) | `memory/about-you.md` or `memory/facts.md`, by subject |
//! | Note | `notes/<slug>.md` |
//! | Task | `projects/<project>.md` (`inbox` without one) |
//! | Session record | The session's digest |
//! | Raw audio, secret, or `Never` | Nowhere: refused |
//!
//! Everything the store adds is redacted before it is cut, named or written (I8): a file name is
//! the slug of redacted text, so a secret never names a file. The memory files and task lists
//! are yours, so they are edited a line at a time ([`Edited`]) and the rest of each file stays
//! byte for byte as you wrote it (D-042).
//!
//! Each capture carries the core's [`CaptureId`] (a keep carries its proposal's), so "forget
//! that" undoes exactly the capture the core named, whatever failed before it.

mod undo;

use boardai_core::ids::CaptureId;
use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
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
use crate::store::redact::{Redacted, redact_text};
use crate::store::slug::{first_free, slugify};
use crate::store::vault::edit::{Editable, Edited, ItemLine, OnDisk, read_editable};
use crate::store::vault::markdown::{FrontValue, one_line, render_front_matter};
use crate::store::vault::{INBOX_PROJECT, MemoryFile, VaultPath, write_atomic};
use crate::store::views::Remembered;

const ABOUT_YOU_HEADER: &str = "# About you\n\nHow you like to learn and what you've covered. \
Edit freely; keep the ^ids at the ends of lines.\n\n";
const FACTS_HEADER: &str = "# Facts\n\nWhat you told BoardAI about your systems. Edit freely; \
keep the ^ids at the ends of lines.\n\n";
const INBOX_NAME: &str = "Inbox";

/// One thing a session wrote, with what is needed to undo it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Capture {
    pub kind: CaptureKind,
    /// The text kept, redacted, for the digest and for saying what was forgotten.
    pub text: String,
    /// The core's id for it; a keep carries its proposal's. `None` for a proposal of an earlier
    /// session kept with no capture of this one.
    pub capture: Option<CaptureId>,
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
    /// Redacted, one line.
    pub text: String,
    pub accepted: bool,
}

impl Proposal {
    pub(crate) fn new(id: ItemId, about: About, text: &str) -> Self {
        Self {
            id,
            about,
            text: redact_text(text)
                .one_line(VAULT_ITEM_MAX_BYTES)
                .into_string(),
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
    /// The proposal this capture made, when it is one.
    fn proposal(&self) -> Option<ItemId> {
        match self.kind {
            CaptureKind::Proposal { id } => Some(id),
            CaptureKind::Memory { .. }
            | CaptureKind::Note { .. }
            | CaptureKind::Task { .. }
            | CaptureKind::SessionRecord => None,
        }
    }

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
            CaptureKind::SessionRecord => Some(format!("Noted: {}", self.text)),
            // Proposals have their own section.
            CaptureKind::Proposal { .. } => None,
        }
    }
}

/// Which memory file a stated or inferred memory goes to: the subject the core sent (the
/// talker said who it is about), and learning signals are always about you.
pub(crate) const fn about_for(write: &MemoryWrite) -> About {
    match (write.kind, write.subject) {
        (MemoryKind::LearningSignal, _) | (_, Subject::You) => About::You,
        (_, Subject::Systems) => About::Systems,
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

/// Redacted text as one bounded line.
fn item_text(text: &str) -> Redacted {
    redact_text(text).one_line(VAULT_ITEM_MAX_BYTES)
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
        let capture = Some(write.capture);
        match write.action {
            MemoryAction::Propose => Ok(self.propose(&write)),
            MemoryAction::Keep | MemoryAction::OnRequest => match write.kind {
                MemoryKind::Note => self.write_note(&write),
                MemoryKind::Task => self.add_task(&write),
                MemoryKind::SessionRecord => {
                    let text = item_text(&write.text).into_string();
                    self.session_mut()?.captures.push(Capture {
                        kind: CaptureKind::SessionRecord,
                        text,
                        capture,
                    });
                    self.refresh_digest()?;
                    Ok(Remembered::SessionRecord)
                }
                MemoryKind::LearningSignal | MemoryKind::Stated | MemoryKind::Inferred => {
                    let about = about_for(&write);
                    self.keep_memory(about, &write.text, MemorySource::Direct, capture)
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
        let about = about_for(write);
        let id = self.ids.take(ItemKind::Proposal);
        if let Ok(session) = self.session_mut() {
            let proposal = Proposal::new(id, about, &write.text);
            session.captures.push(Capture {
                kind: CaptureKind::Proposal { id },
                text: proposal.text.clone(),
                capture: Some(write.capture),
            });
            session.proposals.push(proposal);
        }
        if let Err(error) = self.refresh_digest() {
            tracing::warn!(%error, "the digest was not updated with the new proposal");
        }
        Remembered::Proposed { id }
    }

    fn write_note(&mut self, write: &MemoryWrite) -> Result<Remembered, StoreError> {
        let text = redact_text(write.text.trim()).cut(VAULT_NOTE_MAX_BYTES);
        let first_line = one_line(
            text.as_str().lines().next().unwrap_or_default(),
            VAULT_ITEM_MAX_BYTES,
        );
        let notes = self.vault.root().join(crate::store::vault::NOTES);
        // The name comes from redacted text: a secret never names a file (I8).
        let slug = slugify(&first_line).unwrap_or_else(|| "note".to_owned());
        let name = first_free(&slug, |name| notes.join(format!("{name}.md")).exists())
            .ok_or_else(|| name_error("notes"))?;
        let path = VaultPath::note(&name).ok_or_else(|| name_error("notes"))?;
        let id = self.ids.take(ItemKind::Note);
        let session_id = self.session_mut()?.id.to_string();
        let links = bounded_links(&write.links)
            .into_iter()
            .map(Redacted::into_string)
            .collect();
        let content = format!(
            "{}{}\n",
            render_front_matter(&[
                ("id", FrontValue::Text(id.to_string())),
                ("created", FrontValue::Text(self.clock.now().rfc3339())),
                ("session", FrontValue::Text(session_id)),
                ("links", FrontValue::List(links)),
            ]),
            text.as_str()
        );
        write_atomic(&self.vault.absolute(&path), &redact_text(&content))
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        self.session_mut()?.captures.push(Capture {
            kind: CaptureKind::Note {
                id,
                path: path.clone(),
            },
            text: first_line,
            capture: Some(write.capture),
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
            .map(item_text)
            .filter(|name| !name.as_str().is_empty())
            .unwrap_or_else(|| redact_text(INBOX_NAME));
        // The file is named after redacted text: a secret never names a file (I8).
        let slug = slugify(project.as_str()).unwrap_or_else(|| INBOX_PROJECT.to_owned());
        let path = VaultPath::project(&slug).ok_or_else(|| name_error("projects"))?;
        let id = self.ids.take(ItemKind::Task);
        let title = item_text(&write.text);
        let mut links = vec![redact_text(&format!("session:{}", self.session_mut()?.id))];
        links.extend(
            bounded_links(&write.links)
                .into_iter()
                .take(VAULT_LINKS_MAX - 1),
        );
        let line = ItemLine::new(Some(false), &title, &links, id);
        let header = redact_text(&format!("# {}\n\n", project.as_str()));
        let added_newline = self.append_line_to(&path, &header, &line)?;
        self.session_mut()?.captures.push(Capture {
            kind: CaptureKind::Task {
                id,
                path: path.clone(),
                added_newline,
            },
            text: title.into_string(),
            capture: Some(write.capture),
        });
        self.reindex(&[Unit::Project(path.clone())])?;
        self.refresh_digest()?;
        Ok(Remembered::Task {
            id,
            project: project.into_string(),
            path: path.to_string(),
        })
    }

    /// Appends a memory line to its file. The capture is recorded when a session is open.
    fn keep_memory(
        &mut self,
        about: About,
        text: &str,
        source: MemorySource,
        capture: Option<CaptureId>,
    ) -> Result<Remembered, StoreError> {
        let file = memory_file(about);
        let path = VaultPath::memory(file);
        let id = self.ids.take(ItemKind::Memory);
        let text = item_text(text);
        let line = ItemLine::new(None, &text, &[], id);
        let added_newline = self.append_line_to(&path, &redact_text(header(file)), &line)?;
        if let Ok(session) = self.session_mut() {
            session.captures.push(Capture {
                kind: CaptureKind::Memory {
                    id,
                    file,
                    added_newline,
                    source,
                },
                text: text.into_string(),
                capture,
            });
        }
        self.reindex(&[Unit::Memory(path)])?;
        self.refresh_digest()?;
        Ok(Remembered::Memory {
            id,
            file: file.into(),
        })
    }

    /// "Yes, keep it": keeps the proposal this session made as the core's `capture`.
    pub(crate) fn keep_proposal(&mut self, capture: CaptureId) -> Result<Remembered, StoreError> {
        let proposal = self
            .session_mut()?
            .captures
            .iter()
            .filter(|kept| kept.capture == Some(capture))
            .find_map(Capture::proposal);
        let Some(id) = proposal else {
            return Err(StoreError::NotFound {
                what: format!("a proposal made as capture {}", capture.0),
            });
        };
        self.accept_proposal(id)
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
            // The keep is undone with its proposal's capture: "forget that" right after a keep
            // undoes the keep, and once more undoes the proposal.
            let capture = session
                .captures
                .iter()
                .find(|kept| kept.proposal() == Some(id))
                .and_then(|kept| kept.capture);
            return self.keep_memory(about, &text, MemorySource::Proposal(id), capture);
        }
        self.accept_past_proposal(id)
    }

    /// Keeps a proposal from an earlier session's digest: its box is ticked there.
    fn accept_past_proposal(&mut self, id: ItemId) -> Result<Remembered, StoreError> {
        let open = || StoreError::NotFound {
            what: format!("an open proposal {id}"),
        };
        let row: Option<(String, String, String, bool)> = self
            .index
            .query_row(
                "SELECT path, about, text, accepted FROM proposals WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((path, about, text, false)) = row else {
            return Err(open());
        };
        let digest = VaultPath::parse(&path).ok_or_else(open)?;
        let content = self.read_editable(&digest)?.ok_or_else(open)?;
        let checked = Edited::set_checked(&content, id, true).ok_or_else(open)?;
        write_atomic(&self.vault.absolute(&digest), &checked)
            .map_err(|error| StoreError::io(digest.as_str(), &error))?;
        let about = About::parse(&about).unwrap_or(About::Systems);
        let source = MemorySource::PastProposal {
            digest: digest.clone(),
            id,
        };
        let remembered = self.keep_memory(about, &text, source, None)?;
        self.reindex(&Unit::of(&digest).into_iter().collect::<Vec<_>>())?;
        Ok(remembered)
    }

    /// Appends a line, creating the file with `header` when missing. Returns whether a newline
    /// had to be added before the line.
    fn append_line_to(
        &mut self,
        path: &VaultPath,
        header: &Redacted,
        line: &ItemLine,
    ) -> Result<bool, StoreError> {
        let (edited, added_newline, created) = match self.read_editable(path)? {
            Some(file) => {
                let (edited, added_newline) = Edited::append(&file, line);
                (edited, added_newline, false)
            }
            None => (Edited::create(header, line), false, true),
        };
        write_atomic(&self.vault.absolute(path), &edited)
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        if created && let Ok(session) = self.session_mut() {
            // The header as written, so forgetting compares like with like.
            session
                .created_files
                .push((path.clone(), header.as_str().to_owned()));
        }
        Ok(added_newline)
    }

    /// Reads a vault file the store may edit: `None` when missing; an error when it is too
    /// large or not UTF-8, so a hand-made file is never mangled by an edit.
    fn read_editable(&self, path: &VaultPath) -> Result<Option<OnDisk>, StoreError> {
        let read = read_editable(&self.vault.absolute(path), VAULT_FILE_MAX_BYTES)
            .map_err(|error| StoreError::io(path.as_str(), &error))?;
        match read {
            Editable::Missing => Ok(None),
            Editable::Text(text) => Ok(Some(text)),
            Editable::LeftAlone => Err(StoreError::Io {
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

/// Links, each redacted and one line, at most [`VAULT_LINKS_MAX`].
fn bounded_links(links: &[String]) -> Vec<Redacted> {
    links
        .iter()
        .map(|link| item_text(link))
        .filter(|link| !link.as_str().is_empty())
        .take(VAULT_LINKS_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use boardai_core::ids::CaptureId;
    use boardai_core::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};

    use super::about_for;
    use crate::store::digest::About;

    #[test]
    fn memories_go_to_the_file_the_subject_names() {
        for (kind, subject, about) in [
            (MemoryKind::Stated, Subject::You, About::You),
            (MemoryKind::Stated, Subject::Systems, About::Systems),
            (MemoryKind::Inferred, Subject::You, About::You),
            (MemoryKind::Inferred, Subject::Systems, About::Systems),
            (MemoryKind::LearningSignal, Subject::Systems, About::You),
        ] {
            let write = MemoryWrite {
                kind,
                action: MemoryAction::Keep,
                // The words no longer decide: "I prefer" about systems stays about systems.
                text: "I prefer Kafka".to_owned(),
                links: Vec::new(),
                project: None,
                subject,
                capture: CaptureId(0),
            };
            assert_eq!(about_for(&write), about, "{kind:?} {subject:?}");
        }
    }
}
