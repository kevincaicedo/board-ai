//! Where a research brief's harness may read (D-226): the shared source's enclosing git
//! repository, so a brief can follow a call into a sibling crate but never wander into the
//! user's other folders. The source folder itself when it isn't in a repository; for a single
//! file, the file's repository or its folder. A repository that is too broad (a dotfiles repo
//! at `~`: `tools::too_broad`) is no reach: the source folder is, and a single file with no
//! folder but `~` around it has none, so no harness reads around it.
//!
//! The harness enforces the reach with its own rules where it has them (`harness::confine`).
//! Found once, where the source is validated (on the blocking pool).

use std::path::{Path, PathBuf};

use boardai_core::sources::SourceKind;

use crate::limits::STAMP_GIT_ANCESTORS_MAX;
use crate::tools::{is_credential, too_broad};

/// The folder whose `.git` (a folder, or a worktree's file) holds `start`, walking up at most
/// [`STAMP_GIT_ANCESTORS_MAX`] folders. `start` is a folder.
#[must_use]
pub(crate) fn enclosing_repo(start: &Path) -> Option<PathBuf> {
    let mut folder = start.to_path_buf();
    for _ in 0..STAMP_GIT_ANCESTORS_MAX {
        if folder.join(".git").exists() {
            return Some(folder);
        }
        folder = folder.parent()?.to_path_buf();
    }
    None
}

/// The reach of a source at canonical `path` (D-226), canonical; `None` when only `~` or
/// wider would hold it.
#[must_use]
pub fn reach(path: &Path, kind: SourceKind, home: &Path) -> Option<PathBuf> {
    let folder = match kind {
        SourceKind::CodeFolder => path.to_path_buf(),
        SourceKind::CodeFile | SourceKind::Document { .. } => path.parent()?.to_path_buf(),
    };
    let held = |candidate: &Path| too_broad(candidate, home).is_none() && !is_credential(candidate);
    enclosing_repo(&folder)
        .filter(|repo| held(repo))
        .map(|repo| std::fs::canonicalize(&repo).unwrap_or(repo))
        .or_else(|| held(&folder).then_some(folder))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use boardai_core::sources::{DocumentFormat, SourceKind};

    use super::reach;

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("boardai-reach-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a folder");
            Self(std::fs::canonicalize(&root).expect("canonical"))
        }

        fn make(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            if relative.ends_with('/') {
                std::fs::create_dir_all(&path).expect("a folder");
            } else {
                std::fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
                std::fs::write(&path, "x\n").expect("a file");
            }
            path
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn reach_of(path: &Path, kind: SourceKind, home: &Path) -> Option<PathBuf> {
        reach(path, kind, home)
    }

    #[test]
    fn a_source_reaches_its_repository_or_only_itself() {
        let root = Folder::new("repo");
        let home = root.make("home/");
        let repo = root.make("home/code/infinitydb/");
        root.make("home/code/infinitydb/.git/");
        let crate_folder = root.make("home/code/infinitydb/crates/inf-alloc/");
        let file = root.make("home/code/infinitydb/crates/inf-alloc/src/arena.rs");
        let loose = root.make("home/notes/scratch/");
        let loose_file = root.make("home/notes/scratch/plan.md");
        let at_home = root.make("home/todo.py");
        // A worktree's `.git` is a file.
        let worktree = root.make("home/code/wt/");
        root.make("home/code/wt/.git");
        let in_worktree = root.make("home/code/wt/src/");

        assert_eq!(
            reach_of(&crate_folder, SourceKind::CodeFolder, &home),
            Some(repo.clone())
        );
        assert_eq!(
            reach_of(&file, SourceKind::CodeFile, &home),
            Some(repo.clone())
        );
        assert_eq!(
            reach_of(&loose, SourceKind::CodeFolder, &home),
            Some(loose.clone()),
            "not in a repository: the folder itself"
        );
        let markdown = SourceKind::Document {
            format: DocumentFormat::Markdown,
        };
        assert_eq!(reach_of(&loose_file, markdown, &home), Some(loose));
        assert_eq!(
            reach_of(&in_worktree, SourceKind::CodeFolder, &home),
            Some(worktree)
        );
        assert_eq!(
            reach_of(&at_home, SourceKind::CodeFile, &home),
            None,
            "a file loose in the home folder has nothing to reach"
        );
    }

    /// A dotfiles repository at `~` would put every file of the user's in reach: the source
    /// folder is the reach instead.
    #[test]
    fn a_repository_at_home_is_no_reach() {
        let root = Folder::new("dotfiles");
        let home = root.make("home/");
        root.make("home/.git/");
        let project = root.make("home/projects/tool/");
        assert_eq!(
            reach_of(&project, SourceKind::CodeFolder, &home),
            Some(project.clone())
        );
        let script = root.make("home/projects/tool/run.sh");
        assert_eq!(
            reach_of(&script, SourceKind::CodeFile, &home),
            Some(project)
        );
    }
}
