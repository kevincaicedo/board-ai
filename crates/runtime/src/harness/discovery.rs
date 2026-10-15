//! Finding the harness CLIs at startup (LLD §3.5).
//!
//! An app opened from Finder gets a minimal `PATH` (no `~/.local/bin`, no Homebrew), so the
//! usual install folders are searched after `PATH`.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Where each harness was found, if anywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HarnessDiscovery {
    pub claude_code: Option<PathBuf>,
    pub pi: Option<PathBuf>,
}

/// Discovery from this process's `PATH` and `HOME`.
#[must_use]
pub fn discover_from_environment() -> HarnessDiscovery {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    discover(std::env::var_os("PATH").as_deref(), home.as_deref())
}

#[must_use]
pub fn discover(path_variable: Option<&OsStr>, home: Option<&Path>) -> HarnessDiscovery {
    let folders = search_folders(path_variable, home);
    HarnessDiscovery {
        claude_code: find_program("claude", &folders),
        pi: find_program("pi", &folders),
    }
}

fn search_folders(path_variable: Option<&OsStr>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = path_variable
        .map(|value| std::env::split_paths(value).collect())
        .unwrap_or_default();
    if let Some(home) = home {
        folders.push(home.join(".local/bin"));
        folders.push(home.join(".npm-global/bin"));
    }
    folders.push(PathBuf::from("/opt/homebrew/bin"));
    folders.push(PathBuf::from("/usr/local/bin"));
    let mut unique = Vec::with_capacity(folders.len());
    for folder in folders {
        if folder.is_absolute() && !unique.contains(&folder) {
            unique.push(folder);
        }
    }
    unique
}

fn find_program(name: &str, folders: &[PathBuf]) -> Option<PathBuf> {
    folders
        .iter()
        .map(|folder| folder.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}
