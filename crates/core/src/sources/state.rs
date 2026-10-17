//! What the core knows about this session's sources: each one's scope and state, the current
//! project, the tray, and the drag over the window. Pure bookkeeping; the engine decides what
//! to say and what to ask of the runtime (`engine::shared`).

use std::collections::VecDeque;

use super::tray::{DropOverlay, ProjectTab, SourceRow, SourcesTray};
use super::{
    BriefTopic, DragOver, Project, SourceId, SourceInfo, SourceKind, SourceState, name_key,
};
use crate::limits::{
    BRIEF_TOPICS_PER_SOURCE_MAX, PROJECT_SOURCES_MAX, READING_GHOSTS_MAX, SESSION_SOURCES_MAX,
    SOURCE_REASON_MAX_BYTES,
};
use crate::narration::tags::bounded;

/// The harness's name when none is known, in what BoardAI says.
pub const HARNESS_FALLBACK_NAME: &str = "the coding harness";

/// The session's project: chosen (by voice, or by keeping a source), or proposed from a shared
/// code folder's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectChoice {
    pub project: Project,
    pub proposed: bool,
}

/// One shared source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub info: SourceInfo,
    /// Kept in the current project, so every session of it can read the source.
    pub kept: bool,
    /// Its state once nothing is reading it.
    pub settled: SourceState,
    /// Files the harness has read so far, while a brief about it is being written.
    pub reading: Option<u32>,
    /// The overview's components `(id, name)`, once an overview brief is known.
    pub components: Vec<(String, String)>,
    /// Topics whose brief the talker already has in this session's history, oldest first.
    pub delivered: VecDeque<BriefTopic>,
}

impl Entry {
    fn new(info: SourceInfo, kept: bool, settled: SourceState) -> Self {
        Self {
            info,
            kept,
            settled,
            reading: None,
            components: Vec::new(),
            delivered: VecDeque::new(),
        }
    }

    /// Records that the talker has this topic's brief, forgetting the oldest past the bound.
    pub fn deliver(&mut self, topic: BriefTopic) {
        self.delivered.retain(|known| *known != topic);
        self.delivered.push_back(topic);
        while self.delivered.len() > BRIEF_TOPICS_PER_SOURCE_MAX {
            self.delivered.pop_front();
        }
    }

    /// A part of the overview by its id or name.
    #[must_use]
    pub fn component(&self, name: &str) -> Option<&(String, String)> {
        let key = name_key(name);
        self.components
            .iter()
            .find(|(id, label)| !key.is_empty() && (name_key(id) == key || name_key(label) == key))
    }

    fn row(&self, harness: &str) -> SourceRow {
        let state = match self.reading {
            Some(files_read) => SourceState::Reading {
                reader: harness.to_owned(),
                files_read,
                files_estimate: self.info.files_estimate,
            },
            None => self.settled.clone(),
        };
        SourceRow {
            id: self.info.id,
            kind: self.info.kind,
            title: self.info.title.clone(),
            path: self.info.shown_path.clone(),
            state,
        }
    }
}

/// Why a source couldn't be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddRefused {
    /// The session, or the project, holds as many as it may.
    Full,
    /// It is already shared.
    Known,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedSources {
    entries: Vec<Entry>,
    project: Option<ProjectChoice>,
    harness: Option<String>,
    open: bool,
    /// The tray opened on its own for a reading; it closes on its own once nothing is read,
    /// unless the user opened or closed it meanwhile.
    opened_for_reading: bool,
    ghosts: VecDeque<String>,
    drag: Option<DragOver>,
}

impl SharedSources {
    pub fn set_harness(&mut self, harness: Option<String>) {
        self.harness = harness.map(|name| bounded(&name, SOURCE_REASON_MAX_BYTES));
    }

    /// The harness's name, as BoardAI says it ("Claude Code").
    #[must_use]
    pub fn harness_name(&self) -> &str {
        self.harness.as_deref().unwrap_or(HARNESS_FALLBACK_NAME)
    }

    #[must_use]
    pub fn has_harness(&self) -> bool {
        self.harness.is_some()
    }

    #[must_use]
    pub fn project(&self) -> Option<&ProjectChoice> {
        self.project.as_ref()
    }

    /// Sets the project; the sources the previous one kept are no longer shared, and their ids
    /// are returned so the runtime forgets them.
    pub fn set_project(&mut self, project: Project, proposed: bool) -> Vec<SourceId> {
        let same = self
            .project
            .as_ref()
            .is_some_and(|current| current.project.id == project.id);
        self.project = Some(ProjectChoice { project, proposed });
        if same {
            return Vec::new();
        }
        let gone: Vec<SourceId> = self
            .entries
            .iter()
            .filter(|entry| entry.kept)
            .map(|entry| entry.info.id)
            .collect();
        self.entries.retain(|entry| !entry.kept);
        gone
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[must_use]
    pub fn get(&self, id: SourceId) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.info.id == id)
    }

    pub fn get_mut(&mut self, id: SourceId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.info.id == id)
    }

    /// Adds a source, kept or for this session only, within its scope's bound.
    pub fn add(
        &mut self,
        info: SourceInfo,
        kept: bool,
        settled: SourceState,
    ) -> Result<&mut Entry, AddRefused> {
        if self.get(info.id).is_some() {
            return Err(AddRefused::Known);
        }
        let in_scope = self
            .entries
            .iter()
            .filter(|entry| entry.kept == kept)
            .count();
        let bound = if kept {
            PROJECT_SOURCES_MAX
        } else {
            SESSION_SOURCES_MAX
        };
        if in_scope >= bound {
            return Err(AddRefused::Full);
        }
        self.entries.push(Entry::new(info.bounded(), kept, settled));
        let index = self.entries.len() - 1;
        Ok(&mut self.entries[index])
    }

    /// Moves a source into the project (`true`) or back to this session; false past the bound.
    pub fn set_kept(&mut self, id: SourceId, kept: bool) -> bool {
        let in_scope = self
            .entries
            .iter()
            .filter(|entry| entry.kept == kept)
            .count();
        let bound = if kept {
            PROJECT_SOURCES_MAX
        } else {
            SESSION_SOURCES_MAX
        };
        let Some(entry) = self.get_mut(id) else {
            return false;
        };
        if entry.kept == kept {
            return true;
        }
        if in_scope >= bound {
            return false;
        }
        entry.kept = kept;
        true
    }

    pub fn remove(&mut self, id: SourceId) -> Option<Entry> {
        let index = self.entries.iter().position(|entry| entry.info.id == id)?;
        Some(self.entries.remove(index))
    }

    /// The source a spoken name means: its title, or its title without an extension.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<SourceId> {
        let key = name_key(name);
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                let title = &entry.info.title;
                let stem = title
                    .rsplit_once('.')
                    .map_or(title.as_str(), |(stem, _)| stem);
                !key.is_empty() && (name_key(title) == key || name_key(stem) == key)
            })
            .map(|entry| entry.info.id)
    }

    /// The newest source shared for this session only.
    #[must_use]
    pub fn newest_in_session(&self) -> Option<SourceId> {
        self.entries
            .iter()
            .rev()
            .find(|entry| !entry.kept)
            .map(|entry| entry.info.id)
    }

    /// The code source a research question is about: the only one, the one whose name the
    /// question says, else the newest. `None` when no code source is shared.
    #[must_use]
    pub fn code_source_for(&self, question: &str) -> Option<SourceId> {
        let code: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.info.kind.is_code())
            .collect();
        let question = name_key(question);
        let named = code.iter().rev().find(|entry| {
            let key = name_key(&entry.info.title);
            key.len() >= 3 && question.contains(&key)
        });
        named.or_else(|| code.last()).map(|entry| entry.info.id)
    }

    /// The code source that has a part called `name`, newest first.
    #[must_use]
    pub fn with_component(&self, name: &str) -> Option<(SourceId, String)> {
        self.entries.iter().rev().find_map(|entry| {
            let (id, _) = entry.component(name)?;
            Some((entry.info.id, id.clone()))
        })
    }

    #[must_use]
    pub fn is_reading(&self) -> bool {
        self.entries.iter().any(|entry| entry.reading.is_some())
    }

    /// A brief about `id` started: the tray opens on its own, unless it is open already.
    pub fn start_reading(&mut self, id: SourceId) {
        let Some(entry) = self.get_mut(id) else {
            return;
        };
        if entry.reading.is_none() {
            entry.reading = Some(0);
        }
        if !self.open {
            self.open = true;
            self.opened_for_reading = true;
        }
    }

    /// The harness read `path` for a brief about `id`.
    pub fn read_file(&mut self, id: SourceId, path: &str) {
        let Some(entry) = self.get_mut(id) else {
            return;
        };
        if let Some(files) = entry.reading.as_mut() {
            *files = files.saturating_add(1);
        }
        let name: Vec<&str> = path.rsplit('/').take(3).collect();
        let shown: Vec<&str> = name.into_iter().rev().collect();
        self.ghosts
            .push_back(bounded(&shown.join("/"), SOURCE_REASON_MAX_BYTES));
        while self.ghosts.len() > READING_GHOSTS_MAX {
            self.ghosts.pop_front();
        }
    }

    /// The brief about `id` is done: the source settles, and once nothing is read the ghosts go
    /// and a tray that opened on its own closes.
    pub fn finish_reading(&mut self, id: SourceId, settled: Option<SourceState>) {
        if let Some(entry) = self.get_mut(id) {
            entry.reading = None;
            if let Some(settled) = settled {
                entry.settled = settled;
            }
        }
        if !self.is_reading() {
            self.ghosts.clear();
            if std::mem::take(&mut self.opened_for_reading) {
                self.open = false;
            }
        }
    }

    /// The tray button or "show sources" / "hide sources". Returns whether it changed.
    pub fn set_open(&mut self, open: bool) -> bool {
        self.opened_for_reading = false;
        std::mem::replace(&mut self.open, open) != open
    }

    pub fn set_drag(&mut self, drag: Option<DragOver>) -> bool {
        std::mem::replace(&mut self.drag, drag.clone()) != drag
    }

    /// The tray as the UI shows it.
    #[must_use]
    pub fn tray(&self, talker: &str) -> SourcesTray {
        let harness = self.harness_name();
        let rows = |kept: bool| -> Vec<SourceRow> {
            self.entries
                .iter()
                .filter(|entry| entry.kept == kept)
                .map(|entry| entry.row(harness))
                .collect()
        };
        SourcesTray {
            open: self.open,
            session: rows(false),
            project: self.project.as_ref().map(|choice| ProjectTab {
                name: choice.project.name.clone(),
                proposed: choice.proposed,
                sources: rows(true),
            }),
            talker: talker.to_owned(),
            harness: self.harness.clone(),
            reading: self.ghosts.iter().cloned().collect(),
        }
    }

    /// The overlay while something is dragged over the window.
    #[must_use]
    pub fn drop_overlay(&self) -> Option<DropOverlay> {
        let drag = self.drag.as_ref()?;
        let project = match &self.project {
            Some(choice) => choice.project.name.clone(),
            None => drag
                .title
                .rsplit_once('.')
                .map_or(drag.title.as_str(), |(stem, _)| stem)
                .to_owned(),
        };
        Some(DropOverlay {
            title: drag.title.clone(),
            refused: drag.refused.clone(),
            half: drag.half,
            project,
        })
    }
}

/// How a newly shared source starts: code is read by the harness (or waits for one), documents
/// wait for the document reader (T-031).
#[must_use]
pub fn first_state(kind: SourceKind, has_harness: bool) -> SourceState {
    match kind {
        SourceKind::CodeFolder | SourceKind::CodeFile if has_harness => SourceState::Ready {
            detail: "Shared".to_owned(),
        },
        SourceKind::CodeFolder | SourceKind::CodeFile => SourceState::Waiting {
            reason: "Waiting for a coding harness: install Claude Code or pi".to_owned(),
        },
        SourceKind::Document { .. } => SourceState::Waiting {
            reason: "Waiting for the document reader".to_owned(),
        },
    }
}

/// A reason held to the tray's bound.
#[must_use]
pub fn reason(text: &str) -> String {
    bounded(text, SOURCE_REASON_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::{AddRefused, SharedSources, first_state};
    use crate::limits::SESSION_SOURCES_MAX;
    use crate::sources::{Project, SourceId, SourceInfo, SourceKind, SourceState};

    fn info(id: u32, title: &str) -> SourceInfo {
        SourceInfo {
            id: SourceId(id),
            kind: SourceKind::CodeFolder,
            title: title.to_owned(),
            shown_path: format!("~/code/{title}"),
            files_estimate: Some(40),
        }
    }

    #[test]
    fn a_session_holds_a_bounded_number_of_sources() {
        let mut sources = SharedSources::default();
        for id in 0..SESSION_SOURCES_MAX {
            let added = sources.add(
                info(u32::try_from(id).unwrap_or(0), "x"),
                false,
                first_state(SourceKind::CodeFolder, true),
            );
            assert!(added.is_ok());
        }
        let past = sources.add(
            info(999, "y"),
            false,
            first_state(SourceKind::CodeFolder, true),
        );
        assert_eq!(past.err(), Some(AddRefused::Full));
        let again = sources.add(
            info(0, "x"),
            true,
            first_state(SourceKind::CodeFolder, true),
        );
        assert_eq!(again.err(), Some(AddRefused::Known));
    }

    #[test]
    fn reading_opens_the_tray_and_closes_it_once_done() {
        let mut sources = SharedSources::default();
        sources.set_harness(Some("Claude Code".into()));
        let _ = sources.add(
            info(1, "inf-alloc"),
            false,
            first_state(SourceKind::CodeFolder, true),
        );
        sources.start_reading(SourceId(1));
        sources.read_file(SourceId(1), "crates/inf-alloc/src/arena.rs");
        let tray = sources.tray("OpenRouter");
        assert!(tray.open);
        assert_eq!(tray.reading, vec!["inf-alloc/src/arena.rs".to_owned()]);
        assert!(matches!(
            tray.session[0].state,
            SourceState::Reading {
                files_read: 1,
                files_estimate: Some(40),
                ..
            }
        ));
        sources.finish_reading(SourceId(1), None);
        let tray = sources.tray("OpenRouter");
        assert!(!tray.open && tray.reading.is_empty());
    }

    #[test]
    fn a_new_project_lets_the_old_projects_sources_go() {
        let mut sources = SharedSources::default();
        let _ = sources.set_project(
            Project::named("one").unwrap_or_else(|| unreachable!()),
            false,
        );
        let _ = sources.add(
            info(1, "a"),
            true,
            first_state(SourceKind::CodeFolder, true),
        );
        let _ = sources.add(
            info(2, "b"),
            false,
            first_state(SourceKind::CodeFolder, true),
        );
        let gone = sources.set_project(
            Project::named("two").unwrap_or_else(|| unreachable!()),
            false,
        );
        assert_eq!(gone, vec![SourceId(1)]);
        assert_eq!(sources.entries().len(), 1);
        assert_eq!(sources.named("B"), Some(SourceId(2)));
        assert_eq!(
            sources.code_source_for("how does it work?"),
            Some(SourceId(2))
        );
    }
}
