//! Paths the talker names are parsed once, here, into a [`RepoPath`] that is canonical and sits
//! under a folder the user opened (LLD §9). Nothing outside those folders is read, and nothing
//! outside is even probed for existence. Inside them, credential folders and files
//! ([`CREDENTIAL_PATHS`]) are never read, searched or listed.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use boardai_core::sources::source_title;
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

/// Where credentials live, as one or two path components matched anywhere in a canonical path:
/// the repo tools never read, search or list them, whatever folder is open (a home folder
/// opened by hand in `settings.toml`, a dotfiles repo). A symlink into one is refused by where
/// it leads. `.docker` and `.kube` themselves hold ordinary project files, so only their
/// credential files are listed.
pub const CREDENTIAL_PATHS: [&[&str]; 12] = [
    &[".ssh"],
    &[".aws"],
    &[".gnupg"],
    &[".azure"],
    &[".password-store"],
    &[".config", "gh"],
    &[".config", "gcloud"],
    &[".kube", "config"],
    &[".docker", "config.json"],
    &["Library", "Keychains"],
    &[".netrc"],
    &[".git-credentials"],
];

/// `path` is, or is inside, one of [`CREDENTIAL_PATHS`].
pub(crate) fn is_credential(path: &Path) -> bool {
    let parts: Vec<&OsStr> = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect();
    CREDENTIAL_PATHS.iter().any(|pattern| {
        parts.windows(pattern.len()).any(|window| {
            window
                .iter()
                .zip(pattern.iter())
                .all(|(part, wanted)| *part == OsStr::new(wanted))
        })
    })
}

/// Why a folder can't be opened for the repo tools: it would put far more than code in reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooBroad {
    /// The disk's root, the home folder, or a folder above it (`/Users`): every file of yours.
    HomeOrAbove,
    /// `~/Library` or inside it, where apps keep their data, keychains and caches.
    AppData,
}

/// Whether `folder` is too broad to open as a code folder, compared canonically where both
/// exist (a symlink to `~` is `~`). The settings page refuses it, and a session skips one a
/// hand-edited `settings.toml` names.
#[must_use]
pub fn too_broad(folder: &Path, home: &Path) -> Option<TooBroad> {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let (folder, home) = (canonical(folder), canonical(home));
    if home.starts_with(&folder) {
        return Some(TooBroad::HomeOrAbove);
    }
    folder
        .starts_with(home.join("Library"))
        .then_some(TooBroad::AppData)
}

fn credential_refused() -> ToolError {
    ToolError::new(
        ToolFailure::OutsideRepo,
        "credential folders and files are never read",
    )
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
    /// A single shared file (D-184): only the file itself is inside it, by its name.
    file: bool,
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
        Self::open_with_files(folders, &[])
    }

    /// Opens folders, and single files shared on their own (D-184): a file is readable by its
    /// name or its path, and nothing beside it is.
    pub fn open_with_files(folders: &[PathBuf], files: &[PathBuf]) -> Result<Self, ToolError> {
        let mut roots = Self::open_folders(folders)?;
        for file in files {
            let Ok(canonical) = std::fs::canonicalize(file) else {
                continue;
            };
            if !canonical.is_file() || is_credential(&canonical) {
                continue;
            }
            let name = canonical
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            roots.roots.push(Root {
                given: file.clone(),
                canonical,
                name,
                file: true,
            });
        }
        Ok(roots)
    }

    fn open_folders(folders: &[PathBuf]) -> Result<Self, ToolError> {
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
                file: false,
            });
        }
        Ok(Self { roots })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The canonical folders (and single files), in the order they were opened.
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
            if is_credential(&candidate) {
                // Refused before the file system is asked whether it exists.
                return Err(credential_refused());
            }
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
        // A single shared file answers to its own name only: as it is, then as BoardAI named it
        // to the talker (plain text, D-291: "notes [draft].md" is "notes (draft).md" there).
        candidates.extend(
            self.roots
                .iter()
                .filter(|root| root.file && path == Path::new(&root.name))
                .map(|root| root.canonical.clone()),
        );
        candidates.extend(
            self.roots
                .iter()
                .filter(|root| root.file && path == Path::new(source_title(&root.name).as_str()))
                .map(|root| root.canonical.clone()),
        );
        for root in self.roots.iter().filter(|root| !root.file) {
            if let Ok(rest) = path.strip_prefix(&root.name)
                && self.roots.len() > 1
            {
                candidates.push(root.canonical.join(rest));
            }
        }
        candidates.extend(
            self.roots
                .iter()
                .filter(|root| !root.file)
                .map(|root| root.canonical.join(path)),
        );
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
        if is_credential(&canonical) {
            return Err(credential_refused());
        }
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
        if root.file {
            return root.name.clone();
        }
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
