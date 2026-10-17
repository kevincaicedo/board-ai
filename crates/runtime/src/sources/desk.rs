//! The session's sources in the runtime: every registered path by its id, which of them are
//! readable now, and the briefs cached for them (D-184, D-185). Paths live only here; the core
//! and the UI know a source by its id, title and `~/…` path.
//!
//! The orchestrator owns the desk (law 3) and asks it for the repo tools' roots and a harness
//! task's workspace. A new session starts it empty, keeping only the id counter, so a late
//! answer about a source of the last session names nothing.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use boardai_core::sources::{Project, ProjectId, SourceId, SourceInfo, SourceKind};

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
        }
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

    pub fn discard(&mut self, ids: &[SourceId]) {
        for id in ids {
            self.entries.remove(id);
        }
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

    /// Where a brief about a readable source runs: its folder, or a single file's folder (the
    /// task message points the harness at that file), reading only inside its reach (D-226).
    /// `None` when it isn't readable or has no reach.
    #[must_use]
    pub fn workspace(&self, id: SourceId) -> Option<Workspace> {
        let entry = self.entries.get(&id).filter(|entry| entry.readable)?;
        let folder = match entry.kind {
            SourceKind::CodeFolder => entry.path.clone(),
            SourceKind::CodeFile | SourceKind::Document { .. } => {
                entry.path.parent()?.to_path_buf()
            }
        };
        Some(Workspace {
            folder,
            answer: Answer::Brief {
                reach: entry.reach.clone()?,
            },
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
            match entry.kind {
                SourceKind::CodeFolder if !inside => folders.push(entry.path.clone()),
                SourceKind::CodeFile if !inside => files.push(entry.path.clone()),
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

    /// What to undo when the session is forgotten.
    pub fn take_written(&mut self) -> Vec<Written> {
        std::mem::take(&mut self.written)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use boardai_core::sources::SourceKind;

    use super::{CachedBrief, SourceDesk};
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
                .map(|workspace| workspace.answer),
            Some(Answer::Brief {
                reach: PathBuf::from("/other/db")
            }),
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
}
