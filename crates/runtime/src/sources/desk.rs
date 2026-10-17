//! The session's sources in the runtime: every registered path by its id, which of them are
//! readable now, and the briefs cached for them (D-184, D-185). Paths live only here; the core
//! and the UI know a source by its id, title and `~/…` path.
//!
//! The orchestrator owns the desk (law 3) and asks it for the repo tools' roots and a harness
//! task's workspace. A new session starts it empty, keeping only the id counter, so a late
//! answer about a source of the last session names nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use boardai_core::RepoReads;
use boardai_core::sources::tray::SourcesTray;
use boardai_core::sources::{DocumentFormat, Project, ProjectId, SourceId, SourceInfo, SourceKind};

use super::stamp::Stamp;
use super::validate::{ValidSource, written_path};
use crate::harness::{Answer, Workspace};
use crate::limits::SESSION_BRIEFS_CACHED_MAX;
use crate::tools::{RepoRoots, ToolError};

#[derive(Clone, Debug)]
struct Entry {
    path: PathBuf,
    kind: SourceKind,
    info: SourceInfo,
    /// Shared or kept and loaded: the repo tools and the harness may read it.
    readable: bool,
    /// Kept in the current project: its briefs are saved in the project's folder.
    kept: bool,
    /// Where a brief's harness may read (D-226); `None`: no brief runs on it.
    reach: Option<PathBuf>,
}

/// A brief the cache holds, with what its code looked like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedBrief {
    pub markdown: String,
    pub stamp: Stamp,
    /// `YYYY-MM-DD`.
    pub written: String,
}

/// What the session wrote to a project's folder, undone if the session is forgotten (I10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    Kept {
        project: ProjectId,
        path: String,
    },
    Brief {
        project: ProjectId,
        source: String,
        topic: String,
    },
}

#[derive(Debug)]
pub struct SourceDesk {
    home: PathBuf,
    code_roots: Vec<PathBuf>,
    harness: Option<String>,
    next_id: u32,
    entries: BTreeMap<SourceId, Entry>,
    /// By the source's path and the topic's key; the least recently used goes first.
    cache: VecDeque<((PathBuf, String), CachedBrief)>,
    project: Option<Project>,
    written: Vec<Written>,
    /// "Don't remember this session" (I10): nothing more of it goes to a project's folder.
    forgotten: bool,
    /// The ids the core's latest tray shows, session and project: sources the core holds.
    held: BTreeSet<SourceId>,
    /// Ids the core discarded while its tray still showed them (the same path offered again as
    /// a candidate or a kept line keeps its id); forgotten once a tray no longer does.
    discarded_while_held: BTreeSet<SourceId>,
    /// What the core was last told the talker's requests offer (D-232, D-244): the repo reads,
    /// over the code folders and readable code sources. A shared document, readable by its
    /// lines too (D-256), offers none.
    reads: RepoReads,
}

impl SourceDesk {
    /// `code_roots` are the allowed code folders (already held to `tools::too_broad`);
    /// `harness` is the installed harness's name, `None` without one.
    #[must_use]
    pub fn new(home: PathBuf, code_roots: Vec<PathBuf>, harness: Option<String>) -> Self {
        // Canonical, as every source path is, so "inside a code folder" compares like for like.
        let code_roots = code_roots
            .into_iter()
            .map(|root| std::fs::canonicalize(&root).unwrap_or(root))
            .collect();
        Self {
            home: std::fs::canonicalize(&home).unwrap_or(home),
            code_roots,
            harness,
            next_id: 1,
            entries: BTreeMap::new(),
            cache: VecDeque::new(),
            project: None,
            written: Vec::new(),
            forgotten: false,
            held: BTreeSet::new(),
            discarded_while_held: BTreeSet::new(),
            reads: RepoReads::NotOffered,
        }
        .with_reads()
    }

    /// The reads the code folders alone offer: what a session starts with.
    fn with_reads(mut self) -> Self {
        self.reads = self.code_reads();
        self
    }

    fn code_reads(&self) -> RepoReads {
        let code = !self.code_roots.is_empty()
            || self
                .entries
                .values()
                .any(|entry| entry.readable && entry.kind.is_code());
        if code {
            RepoReads::Offered
        } else {
            RepoReads::NotOffered
        }
    }

    /// The reads offered now, when they changed since the core was last told.
    pub fn changed_reads(&mut self) -> Option<RepoReads> {
        let reads = self.code_reads();
        (reads != self.reads).then(|| {
            self.reads = reads;
            reads
        })
    }

    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    #[must_use]
    pub fn harness(&self) -> Option<&str> {
        self.harness.as_deref()
    }

    #[must_use]
    pub fn code_roots(&self) -> &[PathBuf] {
        &self.code_roots
    }

    /// A new session: nothing shared, nothing cached, no project yet.
    pub fn start_session(&mut self) {
        self.entries.clear();
        self.cache.clear();
        self.project = None;
        self.written.clear();
        self.forgotten = false;
        self.held.clear();
        self.discarded_while_held.clear();
        self.reads = self.code_reads();
    }

    /// Registers a validated path, not yet readable. The same path keeps its id.
    pub fn register(&mut self, valid: &ValidSource) -> SourceInfo {
        if let Some(entry) = self.entries.values().find(|entry| entry.path == valid.path) {
            return entry.info.clone();
        }
        let id = SourceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let info = SourceInfo {
            id,
            kind: valid.kind,
            title: valid.title.clone(),
            shown_path: valid.shown.clone(),
            files_estimate: valid.files_estimate,
        }
        .bounded();
        self.entries.insert(
            id,
            Entry {
                path: valid.path.clone(),
                kind: valid.kind,
                info: info.clone(),
                readable: false,
                kept: false,
                reach: valid.reach.clone(),
            },
        );
        info
    }

    /// A kept source whose path is gone or no longer passes the checks: shown, never read.
    pub fn register_missing(&mut self, line: &crate::store::KeptLine) -> SourceInfo {
        let id = SourceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let title = line
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&line.path)
            .to_owned();
        let info = SourceInfo {
            id,
            kind: line.kind,
            title,
            shown_path: line.path.clone(),
            files_estimate: None,
        }
        .bounded();
        self.entries.insert(
            id,
            Entry {
                path: PathBuf::new(),
                kind: line.kind,
                info: info.clone(),
                readable: false,
                kept: true,
                reach: None,
            },
        );
        info
    }

    pub fn set_kept(&mut self, id: SourceId, kept: bool) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.kept = kept;
        }
    }

    #[must_use]
    pub fn is_kept(&self, id: SourceId) -> bool {
        self.entries.get(&id).is_some_and(|entry| entry.kept)
    }

    /// Makes a registered source readable; false for an id it doesn't know.
    pub fn share(&mut self, id: SourceId) -> bool {
        self.entries
            .get_mut(&id)
            .map(|entry| entry.readable = true)
            .is_some()
    }

    /// The core let these ids go. One its latest tray still shows is kept until a tray no
    /// longer does: the core holds it as a source, whatever else it discarded the id as (finding
    /// 12: the same path keeps its id). Returns the ids forgotten now.
    pub fn discard(&mut self, ids: &[SourceId]) -> Vec<SourceId> {
        let mut gone = Vec::new();
        for id in ids {
            if self.held.contains(id) {
                self.discarded_while_held.insert(*id);
            } else if self.entries.remove(id).is_some() {
                gone.push(*id);
            }
        }
        gone
    }

    /// The core's tray as it sent it: what it holds now. Ids it discarded while showing them,
    /// and shows no longer, are forgotten; returns them.
    pub fn tray_seen(&mut self, tray: &SourcesTray) -> Vec<SourceId> {
        self.held = tray
            .session
            .iter()
            .chain(tray.project.iter().flat_map(|project| &project.sources))
            .map(|row| row.id)
            .collect();
        let gone: Vec<SourceId> = self
            .discarded_while_held
            .iter()
            .filter(|id| !self.held.contains(id))
            .copied()
            .collect();
        for id in &gone {
            self.discarded_while_held.remove(id);
            self.entries.remove(id);
        }
        gone
    }

    #[must_use]
    pub fn is_readable(&self, id: SourceId) -> bool {
        self.entries.get(&id).is_some_and(|entry| entry.readable)
    }

    #[must_use]
    pub fn path(&self, id: SourceId) -> Option<&Path> {
        self.entries.get(&id).map(|entry| entry.path.as_path())
    }

    #[must_use]
    pub fn kind(&self, id: SourceId) -> Option<SourceKind> {
        self.entries.get(&id).map(|entry| entry.kind)
    }

    /// The path as a project's `sources.md` writes it.
    #[must_use]
    pub fn written_path(&self, id: SourceId) -> Option<String> {
        self.path(id).map(|path| written_path(path, &self.home))
    }

    /// Where a brief about a readable code source runs: its folder, or a single file's folder
    /// (the task message points the harness at that file), reading only inside its reach
    /// (D-226). `None` when it isn't readable or has no reach; a document never has one (D-231).
    #[must_use]
    pub fn workspace(&self, id: SourceId) -> Option<Workspace> {
        let entry = self.entries.get(&id).filter(|entry| entry.readable)?;
        let folder = match entry.kind {
            SourceKind::CodeFolder => entry.path.clone(),
            SourceKind::CodeFile => entry.path.parent()?.to_path_buf(),
            SourceKind::Document { .. } => return None,
        };
        Some(Workspace {
            folder,
            reach: entry.reach.clone()?,
            answer: Answer::Brief,
        })
    }

    /// A readable source that no harness may read around: a single file loose in the home
    /// folder (D-226).
    #[must_use]
    pub fn has_no_reach(&self, id: SourceId) -> bool {
        self.entries
            .get(&id)
            .is_some_and(|entry| entry.readable && entry.reach.is_none())
    }

    /// The reach, when a brief's citations beyond the source may be read natively: it is wider
    /// than the source and inside an allowed code folder (D-226). Otherwise those citations are
    /// dropped.
    #[must_use]
    pub fn cited_beyond(&self, id: SourceId) -> Option<PathBuf> {
        let entry = self.entries.get(&id).filter(|entry| entry.readable)?;
        let reach = entry.reach.as_ref()?;
        let wider = reach != &entry.path;
        let allowed = self.code_roots.iter().any(|root| reach.starts_with(root));
        (wider && allowed).then(|| reach.clone())
    }

    /// The repo tools' roots: the code folders and every readable source (a file alone, never
    /// its folder).
    pub fn roots(&self) -> Result<RepoRoots, ToolError> {
        let mut folders = self.code_roots.clone();
        let mut files = Vec::new();
        for entry in self.entries.values().filter(|entry| entry.readable) {
            let inside = self
                .code_roots
                .iter()
                .any(|root| entry.path.starts_with(root));
            // A Markdown or text document is read by its lines like a code file (D-256); a PDF
            // only through its reader's extracted text.
            let by_lines = match entry.kind {
                SourceKind::CodeFile
                | SourceKind::Document {
                    format: DocumentFormat::Markdown | DocumentFormat::Text,
                } => true,
                SourceKind::CodeFolder
                | SourceKind::Document {
                    format: DocumentFormat::Pdf,
                } => false,
            };
            match entry.kind {
                SourceKind::CodeFolder if !inside => folders.push(entry.path.clone()),
                SourceKind::CodeFile | SourceKind::Document { .. } if !inside && by_lines => {
                    files.push(entry.path.clone());
                }
                SourceKind::CodeFolder | SourceKind::CodeFile | SourceKind::Document { .. } => {}
            }
        }
        RepoRoots::open_with_files(&folders, &files)
    }

    #[must_use]
    pub fn cached(&self, path: &Path, topic: &str) -> Option<&CachedBrief> {
        self.cache
            .iter()
            .find(|((known, key), _)| known == path && key == topic)
            .map(|(_, brief)| brief)
    }

    pub fn cache(&mut self, path: PathBuf, topic: String, brief: CachedBrief) {
        self.evict(&path, &topic);
        self.cache.push_back(((path, topic), brief));
        while self.cache.len() > SESSION_BRIEFS_CACHED_MAX {
            self.cache.pop_front();
        }
    }

    /// Every topic cached for a path, with its brief.
    #[must_use]
    pub fn cached_for(&self, path: &Path) -> Vec<(String, CachedBrief)> {
        self.cache
            .iter()
            .filter(|((known, _), _)| known == path)
            .map(|((_, topic), brief)| (topic.clone(), brief.clone()))
            .collect()
    }

    pub fn evict(&mut self, path: &Path, topic: &str) {
        self.cache
            .retain(|((known, key), _)| !(known == path && key == topic));
    }

    #[must_use]
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    pub fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    /// Something this session wrote to a project's folder.
    pub fn wrote(&mut self, written: Written) {
        if !self.written.contains(&written) {
            self.written.push(written);
        }
    }

    /// "Don't remember this session" (I10): what it wrote to a project's folder, to undo. What it
    /// kept is no longer kept, and nothing more of it is written there: no keep, and no brief or
    /// digest that arrives later.
    pub fn forget_session(&mut self) -> Vec<Written> {
        self.forgotten = true;
        let written = std::mem::take(&mut self.written);
        for item in &written {
            let Written::Kept { path, .. } = item else {
                continue;
            };
            let home = self.home.clone();
            for entry in self.entries.values_mut() {
                if written_path(&entry.path, &home) == *path {
                    entry.kept = false;
                }
            }
        }
        written
    }

    /// Whether the session was forgotten: nothing of it goes to a project's folder.
    #[must_use]
    pub fn is_forgotten(&self) -> bool {
        self.forgotten
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use boardai_core::sources::tray::{SourceRow, SourcesTray};
    use boardai_core::sources::{Project, ProjectId, SourceId, SourceKind, SourceState};

    use super::{CachedBrief, SourceDesk, Written};
    use crate::harness::Answer;
    use crate::sources::stamp::Stamp;
    use crate::sources::validate::ValidSource;

    fn valid(path: &str, kind: SourceKind) -> ValidSource {
        ValidSource {
            path: PathBuf::from(path),
            kind,
            title: path.rsplit('/').next().unwrap_or(path).to_owned(),
            shown: path.to_owned(),
            files_estimate: None,
            reach: Some(PathBuf::from(path)),
        }
    }

    /// A brief's citations beyond the source are read natively only when its reach (the
    /// enclosing repository) is inside an allowed code folder (D-226).
    #[test]
    fn citations_beyond_the_source_need_a_reach_inside_a_code_folder() {
        let mut desk = SourceDesk::new(
            PathBuf::from("/Users/me"),
            vec![PathBuf::from("/code")],
            None,
        );
        let in_repo = |path: &str, repo: &str| ValidSource {
            reach: Some(PathBuf::from(repo)),
            ..valid(path, SourceKind::CodeFolder)
        };
        let allowed = desk.register(&in_repo("/code/db/crates/alloc", "/code/db"));
        let elsewhere = desk.register(&in_repo("/other/db/crates/alloc", "/other/db"));
        let alone = desk.register(&valid("/code/loose", SourceKind::CodeFolder));
        let loose_file = desk.register(&ValidSource {
            reach: None,
            ..valid("/Users/me/todo.py", SourceKind::CodeFile)
        });
        for id in [allowed.id, elsewhere.id, alone.id, loose_file.id] {
            assert!(desk.share(id));
        }
        assert_eq!(
            desk.cited_beyond(allowed.id),
            Some(PathBuf::from("/code/db"))
        );
        assert_eq!(desk.cited_beyond(elsewhere.id), None, "not a code folder");
        assert_eq!(desk.cited_beyond(alone.id), None, "nothing beyond itself");
        assert_eq!(
            desk.workspace(elsewhere.id)
                .map(|workspace| (workspace.reach, workspace.answer)),
            Some((PathBuf::from("/other/db"), Answer::Brief)),
            "the harness still reads the whole repository"
        );
        assert!(desk.workspace(loose_file.id).is_none());
        assert!(desk.has_no_reach(loose_file.id));
    }

    #[test]
    fn a_path_keeps_its_id_and_reads_only_once_shared() {
        let mut desk = SourceDesk::new(PathBuf::from("/Users/me"), Vec::new(), None);
        let first = desk.register(&valid("/src/inf-alloc", SourceKind::CodeFolder));
        let again = desk.register(&valid("/src/inf-alloc", SourceKind::CodeFolder));
        assert_eq!(first.id, again.id);
        assert!(
            desk.workspace(first.id).is_none(),
            "registered, not readable"
        );
        assert!(desk.share(first.id));
        assert_eq!(
            desk.workspace(first.id).map(|workspace| workspace.folder),
            Some(PathBuf::from("/src/inf-alloc"))
        );
        desk.start_session();
        assert!(desk.path(first.id).is_none());
        let next = desk.register(&valid("/src/inf-alloc", SourceKind::CodeFolder));
        assert_ne!(next.id, first.id, "ids are never reused within a run");
    }

    #[test]
    fn the_cache_forgets_the_least_recently_written() {
        let mut desk = SourceDesk::new(PathBuf::from("/Users/me"), Vec::new(), None);
        let brief = |text: &str| CachedBrief {
            markdown: text.to_owned(),
            stamp: Stamp::stored("s"),
            written: "2026-10-16".to_owned(),
        };
        for index in 0..=crate::limits::SESSION_BRIEFS_CACHED_MAX {
            desk.cache(
                PathBuf::from(format!("/src/{index}")),
                "overview".into(),
                brief("b"),
            );
        }
        assert!(desk.cached(&PathBuf::from("/src/0"), "overview").is_none());
        assert!(desk.cached(&PathBuf::from("/src/1"), "overview").is_some());
        desk.evict(&PathBuf::from("/src/1"), "overview");
        assert!(desk.cached(&PathBuf::from("/src/1"), "overview").is_none());
    }

    fn tray(ids: &[SourceId]) -> SourcesTray {
        SourcesTray {
            open: false,
            session: ids
                .iter()
                .map(|id| SourceRow {
                    id: *id,
                    kind: SourceKind::CodeFolder,
                    title: "inf-alloc".into(),
                    path: "~/src/inf-alloc".into(),
                    state: SourceState::Ready {
                        detail: String::new(),
                    },
                })
                .collect(),
            project: None,
            talker: "the model".into(),
            harness: None,
            reading: Vec::new(),
        }
    }

    /// Finding 12: the same path keeps its id, so the core may discard an id it still holds (a
    /// candidate or a kept line that was a shared source too). The desk forgets a source only
    /// once the core's tray no longer shows it.
    #[test]
    fn a_source_the_core_still_shows_is_never_forgotten() {
        let mut desk = SourceDesk::new(PathBuf::from("/Users/me"), Vec::new(), None);
        let shared = desk.register(&valid("/src/inf-alloc", SourceKind::CodeFolder));
        assert!(desk.share(shared.id));
        let other = desk.register(&valid("/src/other/inf-alloc", SourceKind::CodeFolder));
        assert!(desk.tray_seen(&tray(&[shared.id])).is_empty());
        // "Use the inf-alloc crate": both paths are candidates, the shared one by its own id;
        // the core picks neither and discards both.
        assert_eq!(desk.discard(&[shared.id, other.id]), [other.id]);
        assert!(desk.is_readable(shared.id), "the core still holds it");
        assert!(desk.tray_seen(&tray(&[shared.id])).is_empty());
        assert!(desk.is_readable(shared.id));
        // Removed from the tray: forgotten once the tray says so.
        assert!(desk.discard(&[shared.id]).is_empty());
        assert_eq!(desk.tray_seen(&tray(&[])), [shared.id]);
        assert!(desk.path(shared.id).is_none());
    }

    /// I10: "don't remember this session" hands back what the session wrote to a project, and
    /// what it kept is no longer kept, so a brief that arrives later isn't saved there.
    #[test]
    fn forgetting_the_session_unkeeps_what_it_kept() {
        let mut desk = SourceDesk::new(PathBuf::from("/Users/me"), Vec::new(), None);
        let kept = desk.register(&valid("/Users/me/src/inf-alloc", SourceKind::CodeFolder));
        assert!(desk.share(kept.id));
        let project = Project {
            id: ProjectId::parse("infinitydb").expect("an id"),
            name: "infinitydb".into(),
        };
        desk.set_project(project.clone());
        desk.set_kept(kept.id, true);
        desk.wrote(Written::Kept {
            project: project.id.clone(),
            path: "~/src/inf-alloc".into(),
        });
        assert!(!desk.is_forgotten());
        let written = desk.forget_session();
        assert_eq!(written.len(), 1);
        assert!(!desk.is_kept(kept.id));
        assert!(desk.is_forgotten());
        assert!(desk.forget_session().is_empty(), "undone once");
        desk.start_session();
        assert!(!desk.is_forgotten(), "the next session is remembered");
    }
}
