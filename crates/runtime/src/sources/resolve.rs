//! A name said by voice ("use the inf-alloc crate", D-184), found inside the allowed code folders
//! and the shared sources by a bounded walk. Breadth first, so the shallowest matches come
//! first; hidden folders, build output and credential folders are never entered. Every match is
//! validated like a drop before it can be offered.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use boardai_core::limits::RESOLVE_CANDIDATES_MAX;
use boardai_core::sources::{NameKind, name_key};

use super::validate::{ValidSource, validate};
use crate::limits::{RESOLVE_DEPTH_MAX, RESOLVE_FOLDERS_MAX};
use crate::tools::is_credential;

/// Folders the walk never enters: build output and dependencies, where a name matches a copy.
const SKIPPED_FOLDERS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "__pycache__",
];

/// Files that make a folder a package.
const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "pom.xml",
    "build.gradle",
    "Package.swift",
    "mix.exs",
];

/// What the walk found: the validated matches, best first, and how many folders it read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub candidates: Vec<ValidSource>,
    pub searched: u32,
}

/// Finds `name` under `roots`. Blocking: call it on the blocking pool.
#[must_use]
pub fn resolve(name: &str, kind: NameKind, roots: &[PathBuf], home: &Path) -> Found {
    let key = name_key(name);
    let mut found = Found::default();
    if key.is_empty() {
        return found;
    }
    let mut matches: Vec<(bool, usize, PathBuf)> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> =
        roots.iter().map(|root| (root.clone(), 0)).collect();
    // A root itself may be what was named ("use the infinitydb repo").
    for root in roots {
        if matches_name(root, &key, kind, true) {
            matches.push((has_manifest(root), 0, root.clone()));
        }
    }
    while let Some((folder, depth)) = queue.pop_front() {
        if found.searched >= RESOLVE_FOLDERS_MAX {
            break;
        }
        found.searched += 1;
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut names: Vec<(PathBuf, bool)> = entries
            .flatten()
            .filter_map(|entry| {
                let is_dir = entry.file_type().ok()?.is_dir();
                Some((entry.path(), is_dir))
            })
            .collect();
        names.sort();
        for (path, is_dir) in names {
            let Some(file_name) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if file_name.starts_with('.') || is_credential(&path) {
                continue;
            }
            if matches_name(&path, &key, kind, is_dir) {
                matches.push((is_dir && has_manifest(&path), depth + 1, path.clone()));
            }
            if is_dir
                && depth + 1 < RESOLVE_DEPTH_MAX
                && !SKIPPED_FOLDERS.contains(&file_name.as_str())
            {
                queue.push_back((path, depth + 1));
            }
        }
    }
    // A package with a manifest first, then the shallowest.
    matches.sort_by_key(|(manifest, depth, path)| (!manifest, *depth, path.clone()));
    for (_, _, path) in matches {
        if found.candidates.len() >= RESOLVE_CANDIDATES_MAX {
            break;
        }
        if let Ok(valid) = validate(&path, home)
            && !found
                .candidates
                .iter()
                .any(|known| known.path == valid.path)
        {
            found.candidates.push(valid);
        }
    }
    found
}

fn matches_name(path: &Path, key: &str, kind: NameKind, is_dir: bool) -> bool {
    let Some(file_name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return false;
    };
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name.as_str(), |(stem, _)| stem);
    let named = name_key(&file_name) == key || name_key(stem) == key;
    named
        && match kind {
            NameKind::Package | NameKind::Folder => is_dir,
            NameKind::Module => true,
            NameKind::File => !is_dir,
        }
}

fn has_manifest(folder: &Path) -> bool {
    MANIFESTS
        .iter()
        .any(|manifest| folder.join(manifest).is_file())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use boardai_core::sources::NameKind;

    use super::resolve;
    use crate::limits::RESOLVE_DEPTH_MAX;

    /// The bounded resolver: a crate is found by its letters, a package with a manifest comes
    /// first, build output and hidden folders are never entered, and the walk stops at its depth.
    #[test]
    fn names_are_found_inside_the_code_folders_within_bounds() {
        let root = std::env::temp_dir().join(format!("boardai-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let code = home.join("code/infinitydb");
        for folder in [
            "crates/inf-alloc/src",
            "vendor/inf_alloc",
            "target/debug/inf-alloc",
            ".cache/inf-alloc",
            "crates/storage/src",
        ] {
            std::fs::create_dir_all(code.join(folder)).expect("a folder");
        }
        std::fs::write(code.join("crates/inf-alloc/Cargo.toml"), "[package]\n").expect("a file");
        std::fs::write(code.join("crates/storage/src/storage.rs"), "\n").expect("a file");
        let mut deep = code.clone();
        for level in 0..=RESOLVE_DEPTH_MAX {
            deep = deep.join(format!("level{level}"));
        }
        std::fs::create_dir_all(deep.join("hidden-deep")).expect("a deep folder");
        let roots = vec![code.clone()];
        let found = resolve("INF Alloc", NameKind::Package, &roots, &home);
        let paths: Vec<PathBuf> = found
            .candidates
            .iter()
            .map(|valid| valid.path.clone())
            .collect();
        let canonical = |path: &str| std::fs::canonicalize(code.join(path)).expect("exists");
        assert_eq!(
            paths,
            vec![canonical("crates/inf-alloc"), canonical("vendor/inf_alloc")]
        );
        assert!(found.searched > 0);
        let module = resolve("storage", NameKind::Module, &roots, &home);
        assert_eq!(
            module.candidates.first().map(|valid| valid.title.as_str()),
            Some("storage")
        );
        let files = resolve("storage", NameKind::File, &roots, &home);
        assert_eq!(
            files.candidates.first().map(|valid| valid.title.as_str()),
            Some("storage.rs")
        );
        assert!(
            resolve("hidden-deep", NameKind::Folder, &roots, &home)
                .candidates
                .is_empty()
        );
        assert!(
            resolve("", NameKind::Folder, &roots, &home)
                .candidates
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
