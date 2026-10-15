//! Paths the talker names are parsed once, here, into a [`RepoPath`] that is canonical and sits
//! under a folder the user opened (LLD §9). Nothing outside those folders is read, and nothing
//! outside is even probed for existence.

use std::io;
use std::path::{Component, Path, PathBuf};

use boardai_core::tools::ToolFailure;

use crate::limits::TOOL_PATH_MAX_BYTES;

/// A tool failure on its way into `ToolOutput::Failed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    pub kind: ToolFailure,
    pub message: String,
}

impl ToolError {
    pub(crate) fn new(kind: ToolFailure, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The folders the user opened, canonicalized when opened.
#[derive(Clone, Debug, Default)]
pub struct RepoRoots {
    roots: Vec<Root>,
}

#[derive(Clone, Debug)]
struct Root {
    /// As opened, for matching absolute paths the talker copied from elsewhere.
    given: PathBuf,
    canonical: PathBuf,
    /// The folder's own name, shown as a prefix when several folders are open.
    name: String,
}

/// A canonical path inside an opened folder. Only [`RepoRoots::resolve`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoPath {
    absolute: PathBuf,
    display: String,
}

impl RepoPath {
    #[must_use]
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }

    /// The path as the talker sees it: relative to its folder, with `/` separators.
    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }
}

impl RepoRoots {
    /// Opens folders. Each must exist and be a directory.
    pub fn open(folders: &[PathBuf]) -> Result<Self, ToolError> {
        let mut roots = Vec::with_capacity(folders.len());
        for folder in folders {
            let canonical = std::fs::canonicalize(folder).map_err(|error| {
                ToolError::new(
                    ToolFailure::NotFound,
                    format!("cannot open {}: {error}", folder.display()),
                )
            })?;
            if !canonical.is_dir() {
                return Err(ToolError::new(
                    ToolFailure::InvalidInput,
                    format!("{} is not a folder", folder.display()),
                ));
            }
            let name = canonical
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            roots.push(Root {
                given: folder.clone(),
                canonical,
                name,
            });
        }
        Ok(Self { roots })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The canonical folders, in the order they were opened.
    pub fn folders(&self) -> impl Iterator<Item = &Path> {
        self.roots.iter().map(|root| root.canonical.as_path())
    }

    /// Parses a path the talker gave. Relative paths are tried against each folder in turn (or
    /// the folder they name first); `..` is refused outright; the canonical result, symlinks
    /// resolved, must be inside an opened folder.
    pub fn resolve(&self, input: &str) -> Result<RepoPath, ToolError> {
        if self.roots.is_empty() {
            return Err(ToolError::new(
                ToolFailure::OutsideRepo,
                "no folder is open",
            ));
        }
        if input.len() > TOOL_PATH_MAX_BYTES || input.contains('\0') {
            return Err(ToolError::new(
                ToolFailure::InvalidInput,
                "the path is not valid",
            ));
        }
        let path = Path::new(input);
        if path.components().any(|part| part == Component::ParentDir) {
            return Err(ToolError::new(
                ToolFailure::OutsideRepo,
                "`..` is not allowed in a path",
            ));
        }
        if path.is_absolute() && !self.lexically_inside(path) {
            // Refused without touching the file system, so the talker cannot probe what exists
            // outside the open folders.
            return Err(ToolError::new(
                ToolFailure::OutsideRepo,
                "the path is outside the open folders",
            ));
        }
        let mut not_found = true;
        for candidate in self.candidates(path) {
            match std::fs::canonicalize(&candidate) {
                Ok(canonical) => return self.contain(canonical),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    not_found = false;
                    tracing::debug!(%error, "a tool path could not be resolved");
                }
            }
        }
        if not_found {
            Err(ToolError::new(
                ToolFailure::NotFound,
                format!("{input} does not exist"),
            ))
        } else {
            Err(ToolError::new(
                ToolFailure::Io,
                format!("{input} could not be read"),
            ))
        }
    }

    fn lexically_inside(&self, path: &Path) -> bool {
        self.roots
            .iter()
            .any(|root| path.starts_with(&root.canonical) || path.starts_with(&root.given))
    }

    fn candidates(&self, path: &Path) -> Vec<PathBuf> {
        if path.is_absolute() {
            return vec![path.to_path_buf()];
        }
        let mut candidates = Vec::new();
        for root in &self.roots {
            if let Ok(rest) = path.strip_prefix(&root.name)
                && self.roots.len() > 1
            {
                candidates.push(root.canonical.join(rest));
            }
        }
        candidates.extend(self.roots.iter().map(|root| root.canonical.join(path)));
        candidates
    }

    fn contain(&self, canonical: PathBuf) -> Result<RepoPath, ToolError> {
        let Some(root) = self
            .roots
            .iter()
            .find(|root| canonical.starts_with(&root.canonical))
        else {
            return Err(ToolError::new(
                ToolFailure::OutsideRepo,
                "the path leads outside the open folders",
            ));
        };
        let display = self.display_in(root, &canonical);
        Ok(RepoPath {
            absolute: canonical,
            display,
        })
    }

    /// How a file found by walking a folder is shown to the talker.
    pub(crate) fn display_for(&self, absolute: &Path) -> String {
        self.roots
            .iter()
            .find(|root| absolute.starts_with(&root.canonical))
            .map_or_else(
                || absolute.to_string_lossy().into_owned(),
                |root| self.display_in(root, absolute),
            )
    }

    fn display_in(&self, root: &Root, absolute: &Path) -> String {
        let relative = absolute.strip_prefix(&root.canonical).unwrap_or(absolute);
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let relative = if parts.is_empty() {
            ".".to_owned()
        } else {
            parts.join("/")
        };
        if self.roots.len() > 1 {
            format!("{}/{relative}", root.name)
        } else {
            relative
        }
    }
}
