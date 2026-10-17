//! A source's freshness stamp (D-185): what its code looked like when a brief was written, so a
//! cached brief answers only while the code is the same. The stamp is the git HEAD (read from
//! `.git` natively: no subprocess, so no `git` on `PATH` is needed and no install prompt can
//! appear) and the newest modification time and count of the files under the source. A commit,
//! an edit, a new file and a deleted one each change it.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;

use super::reach::enclosing_repo;
use crate::limits::{STAMP_FILES_MAX, STAMP_GIT_FILE_MAX_BYTES};
use crate::tools::is_credential;

/// What a brief was written from. Compared whole; never parsed back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp(String);

impl Stamp {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A stamp as the vault stored it.
    #[must_use]
    pub fn stored(text: &str) -> Self {
        Self(text.to_owned())
    }
}

/// Stamps a source (a folder, or one file). Blocking: it reads `.git` and walks the files
/// (bounded by [`STAMP_FILES_MAX`]); call it on the blocking pool.
#[must_use]
pub fn stamp(source: &Path) -> Stamp {
    let head = git_head(source).unwrap_or_else(|| "none".to_owned());
    let (newest_ms, count) = newest_file(source);
    Stamp(format!("head={head};newest={newest_ms};files={count}"))
}

/// The newest modification time (ms since the epoch) among the files and folders under
/// `source`, and how many files there are. Folders count because deleting a file changes its
/// folder's time.
fn newest_file(source: &Path) -> (u128, u32) {
    let mut newest = 0_u128;
    let mut files = 0_u32;
    let walk = WalkBuilder::new(source)
        .hidden(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| !is_credential(entry.path()))
        .build();
    for entry in walk.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            files += 1;
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis());
        newest = newest.max(modified);
        if files >= STAMP_FILES_MAX {
            break;
        }
    }
    (newest, files)
}

/// The commit HEAD names, from the repository holding `source`: `.git` as a folder, or as a
/// file pointing at a worktree's folder (whose refs live in the common folder).
fn git_head(source: &Path) -> Option<String> {
    let folder = if source.is_dir() {
        source
    } else {
        source.parent()?
    };
    let repo = enclosing_repo(folder)?;
    head_of(&git_folder(&repo.join(".git"))?)
}

fn read_small(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > STAMP_GIT_FILE_MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// `.git` itself, or the folder a `.git` file names (`gitdir: …`).
fn git_folder(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let text = read_small(dot_git)?;
    let named = text.trim().strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(named);
    Some(if path.is_absolute() {
        path
    } else {
        dot_git.parent()?.join(path)
    })
}

fn head_of(git: &Path) -> Option<String> {
    let head = read_small(&git.join("HEAD"))?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:") else {
        return Some(head.to_owned());
    };
    let reference = reference.trim();
    if reference
        .split('/')
        .any(|part| part == ".." || part.is_empty())
    {
        return None;
    }
    // A worktree keeps its HEAD and shares the branches of the common folder.
    let common = read_small(&git.join("commondir"))
        .map(|text| git.join(text.trim()))
        .unwrap_or_else(|| git.to_path_buf());
    for folder in [git, common.as_path()] {
        if let Some(commit) = read_small(&folder.join(reference)) {
            return Some(commit.trim().to_owned());
        }
        if let Some(packed) = read_small(&folder.join("packed-refs")) {
            let commit = packed.lines().find_map(|line| {
                let (commit, name) = line.split_once(' ')?;
                (name.trim() == reference).then(|| commit.to_owned())
            });
            if commit.is_some() {
                return commit;
            }
        }
    }
    Some(format!("unborn:{reference}"))
}

#[cfg(test)]
mod tests {
    use super::stamp;

    /// Staleness (D-185): a commit, an edit, a new file and a removed one each change the
    /// stamp; reading changes nothing.
    #[test]
    fn the_stamp_follows_commits_edits_and_files() {
        let root = std::env::temp_dir().join(format!("boardai-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("repo/crates/inf-alloc");
        std::fs::create_dir_all(source.join("src")).expect("a folder");
        std::fs::create_dir_all(root.join("repo/.git/refs/heads")).expect("a .git");
        std::fs::write(root.join("repo/.git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        std::fs::write(root.join("repo/.git/refs/heads/main"), "aaaa\n").expect("a ref");
        std::fs::write(source.join("src/arena.rs"), "pub struct Arena;\n").expect("a file");
        let first = stamp(&source);
        assert!(first.as_str().starts_with("head=aaaa;"), "{first:?}");
        assert_eq!(stamp(&source), first, "reading changes nothing");
        std::fs::write(root.join("repo/.git/refs/heads/main"), "bbbb\n").expect("a commit");
        let committed = stamp(&source);
        assert_ne!(committed, first);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(source.join("src/arena.rs"), "pub struct Arena(u8);\n").expect("an edit");
        let edited = stamp(&source);
        assert_ne!(edited, committed);
        std::fs::write(source.join("src/slot.rs"), "\n").expect("a new file");
        let added = stamp(&source);
        assert_ne!(added, edited);
        std::fs::remove_file(source.join("src/slot.rs")).expect("removed");
        assert_ne!(stamp(&source), added);
        let _ = std::fs::remove_dir_all(&root);
    }
}
