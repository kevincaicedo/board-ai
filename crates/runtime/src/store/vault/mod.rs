//! The vault (LLD §3.6): a folder of plain files you own, and the source of truth.
//!
//! ```text
//! <vault>/
//!   sessions/<date>-<slug>/{events.jsonl, board.excalidraw, digest.md}
//!   notes/<slug>.md
//!   projects/<name>.md
//!   memory/about-you.md
//!   memory/facts.md
//! ```
//!
//! Paths inside the vault are [`VaultPath`]s: relative, `/`-separated and made of safe
//! components, so nothing the store writes can land outside the vault.

mod files;
pub(crate) mod markdown;

use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::error::VaultError;
use crate::limits::VAULT_NAME_MAX_BYTES;

pub(crate) use files::{FileRead, read_text, remove_file, sync_folder, write_atomic};

pub(crate) const SESSIONS: &str = "sessions";
pub(crate) const NOTES: &str = "notes";
pub(crate) const PROJECTS: &str = "projects";
pub(crate) const MEMORY: &str = "memory";
pub(crate) const EVENTS_FILE: &str = "events.jsonl";
pub(crate) const DIGEST_FILE: &str = "digest.md";
pub(crate) const BOARD_FILE: &str = "board.excalidraw";
pub(crate) const ABOUT_YOU_FILE: &str = "about-you.md";
pub(crate) const FACTS_FILE: &str = "facts.md";
/// Tasks without a project go here.
pub(crate) const INBOX_PROJECT: &str = "inbox";

/// The validated vault folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Validates the configured path, creates it and its layout folders if needed, and
    /// removes temporary files a crash left behind. `home` is your home folder: the vault may
    /// not be it or one of its parents.
    pub fn open(path: &Path, home: Option<&Path>) -> Result<Self, VaultError> {
        if !path.is_absolute() {
            return Err(VaultError::NotAbsolute);
        }
        if path.exists() && !path.is_dir() {
            return Err(VaultError::NotAFolder);
        }
        std::fs::create_dir_all(path).map_err(|error| VaultError::Unwritable {
            detail: error.kind().to_string(),
        })?;
        let root = path
            .canonicalize()
            .map_err(|error| VaultError::Unwritable {
                detail: error.kind().to_string(),
            })?;
        let home = home.and_then(|home| home.canonicalize().ok());
        let too_broad =
            root.parent().is_none() || home.as_deref().is_some_and(|home| home.starts_with(&root));
        if too_broad {
            return Err(VaultError::TooBroad);
        }
        for folder in [SESSIONS, NOTES, PROJECTS, MEMORY] {
            std::fs::create_dir_all(root.join(folder)).map_err(|error| VaultError::Unwritable {
                detail: error.kind().to_string(),
            })?;
        }
        files::probe_writable(&root).map_err(|error| VaultError::Unwritable {
            detail: error.kind().to_string(),
        })?;
        files::remove_stale_temporaries(&root);
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of a vault path.
    #[must_use]
    pub fn absolute(&self, path: &VaultPath) -> PathBuf {
        path.0
            .split('/')
            .fold(self.root.clone(), |absolute, part| absolute.join(part))
    }
}

/// A path inside the vault: relative, `/`-separated, every component non-empty, not hidden,
/// and free of separators and `..`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VaultPath(String);

impl VaultPath {
    /// Joins components, each of which must be safe on its own.
    #[must_use]
    pub fn join(components: &[&str]) -> Option<Self> {
        let safe = |component: &&str| {
            !component.is_empty()
                && !component.starts_with('.')
                && component.len() <= VAULT_NAME_MAX_BYTES
                && !component.contains(['/', '\\', '\0'])
        };
        (!components.is_empty() && components.iter().all(safe)).then(|| Self(components.join("/")))
    }

    /// Parses a stored path (from the index) back into a vault path.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::join(&text.split('/').collect::<Vec<_>>())
    }

    /// The vault path of an absolute path under `vault`, if it is one.
    #[must_use]
    pub fn of(vault: &Vault, absolute: &Path) -> Option<Self> {
        let relative = absolute.strip_prefix(vault.root()).ok()?;
        let mut components = Vec::new();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return None;
            };
            components.push(part.to_str()?);
        }
        Self::join(&components)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn session_folder(folder: &str) -> Option<Self> {
        Self::join(&[SESSIONS, folder])
    }

    pub(crate) fn session_file(folder: &str, file: &str) -> Option<Self> {
        Self::join(&[SESSIONS, folder, file])
    }

    pub(crate) fn note(slug: &str) -> Option<Self> {
        Self::join(&[NOTES, &format!("{slug}.md")])
    }

    pub(crate) fn project(slug: &str) -> Option<Self> {
        Self::join(&[PROJECTS, &format!("{slug}.md")])
    }

    pub(crate) fn memory(file: MemoryFile) -> Self {
        Self(format!("{MEMORY}/{}", file.file_name()))
    }

    /// The components of this path.
    pub(crate) fn parts(&self) -> Vec<&str> {
        self.0.split('/').collect()
    }

    /// The file name without its extension.
    pub(crate) fn stem(&self) -> &str {
        let name = self.0.rsplit('/').next().unwrap_or(&self.0);
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }
}

impl fmt::Display for VaultPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The two memory files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryFile {
    /// Preferences and the learner model.
    AboutYou,
    /// Facts about your systems.
    Facts,
}

impl MemoryFile {
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::AboutYou => ABOUT_YOU_FILE,
            Self::Facts => FACTS_FILE,
        }
    }

    pub(crate) fn of_path(path: &VaultPath) -> Option<Self> {
        match path.parts().as_slice() {
            [MEMORY, ABOUT_YOU_FILE] => Some(Self::AboutYou),
            [MEMORY, FACTS_FILE] => Some(Self::Facts),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Vault, VaultPath};
    use crate::store::error::VaultError;

    #[test]
    fn the_vault_path_is_validated_at_startup() {
        let root = std::env::temp_dir().join(format!("boardai-vault-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let vault = home.join("BoardAI");
        let opened = Vault::open(&vault, Some(&home));
        let Ok(opened) = opened else {
            panic!("a fresh vault opens: {opened:?}")
        };
        for folder in ["sessions", "notes", "projects", "memory"] {
            assert!(opened.root().join(folder).is_dir(), "{folder}");
        }
        let stale = opened.root().join("notes/.boardai-1-2.tmp");
        assert!(std::fs::write(&stale, b"half").is_ok());
        assert!(Vault::open(&vault, Some(&home)).is_ok());
        assert!(!stale.exists(), "a crash's temporary file is removed");

        assert_eq!(
            Vault::open(Path::new("BoardAI"), None),
            Err(VaultError::NotAbsolute)
        );
        assert_eq!(Vault::open(&home, Some(&home)), Err(VaultError::TooBroad));
        assert_eq!(Vault::open(&root, Some(&home)), Err(VaultError::TooBroad));
        assert_eq!(Vault::open(Path::new("/"), None), Err(VaultError::TooBroad));
        let file = root.join("a-file");
        assert!(std::fs::write(&file, b"x").is_ok());
        assert_eq!(Vault::open(&file, None), Err(VaultError::NotAFolder));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn vault_paths_cannot_escape() {
        assert_eq!(
            VaultPath::join(&["notes", "a.md"]).map(|path| path.0),
            Some("notes/a.md".to_owned())
        );
        for bad in [
            &["notes", ".."][..],
            &["..", "x"],
            &["notes", ""],
            &["notes", "a/b.md"],
            &["notes", ".hidden.md"],
            &[],
        ] {
            assert!(VaultPath::join(bad).is_none(), "{bad:?}");
        }
        assert!(VaultPath::parse("notes/../../etc").is_none());
        assert!(VaultPath::parse("/etc/passwd").is_none());
        assert_eq!(
            VaultPath::parse("sessions/2026-10-15-x/digest.md").map(|path| path.stem().to_owned()),
            Some("digest".to_owned())
        );
    }
}
