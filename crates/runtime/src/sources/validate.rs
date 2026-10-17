//! A path offered as a source is checked once, here, where it enters (D-184): dropped on the
//! window, picked in the macOS chooser, found by the resolver, or read back from a project's
//! `sources.md`. Nothing else makes a [`ValidSource`].
//!
//! The rules: absolute; within its length bound; no control character (a line break would add a
//! line to a project's `sources.md`); not a credential path (before the file system is asked,
//! and again where a symlink leads); it exists; a folder is a code folder unless it
//! is too broad (the disk, the home folder or above, `~/Library`: `tools::too_broad`); a file's
//! kind comes from its extension, within that kind's size bound; nothing inside `~/Library`.

use std::path::{Path, PathBuf};

use boardai_core::sources::{DocumentFormat, SourceKind};
use ignore::WalkBuilder;

use super::reach::reach;
use crate::limits::{
    SOURCE_CODE_FILE_MAX_BYTES, SOURCE_FILES_COUNTED_MAX, SOURCE_PATH_MAX_BYTES,
    SOURCE_PDF_MAX_BYTES, SOURCE_SHOWN_FULL_MAX_BYTES, SOURCE_TEXT_FILE_MAX_BYTES,
};
use crate::tools::{TooBroad, is_credential, too_broad};

/// A source that passed every check: its canonical path and what the core may know of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidSource {
    /// Canonical: symlinks resolved.
    pub path: PathBuf,
    pub kind: SourceKind,
    /// The folder or file name.
    pub title: String,
    /// `~/…` for the tray, never the whole absolute path.
    pub shown: String,
    /// For a code folder: the files a bounded walk counted.
    pub files_estimate: Option<u32>,
    /// Where a brief's harness may read (`sources::reach`, D-226), canonical; `None`: no
    /// harness reads around it.
    pub reach: Option<PathBuf>,
}

/// Why a path can't be a source, in words the user hears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotAbsolute,
    TooLong,
    /// A line break or another control character in its path.
    ControlCharacter,
    Missing,
    Unreadable,
    /// The disk, the home folder or above: every file of yours.
    HoldsEverything,
    /// `~/Library` or inside it, where apps keep their data and keychains.
    AppData,
    Credentials,
    /// A file BoardAI can't read: not code, Markdown, text or a PDF.
    NotSupported,
    TooLarge,
}

impl Refusal {
    #[must_use]
    pub const fn words(&self) -> &'static str {
        match self {
            Self::NotAbsolute | Self::Missing => "it isn't there any more.",
            Self::TooLong => "its path is too long.",
            Self::ControlCharacter => "its path has a line break or another control character.",
            Self::Unreadable => "it can't be read.",
            Self::HoldsEverything => {
                "that holds every file of yours, keys included. Share the folder your code is in."
            }
            Self::AppData => "that's where apps keep their data and keychains.",
            Self::Credentials => "that's where credentials live, so it's never read.",
            Self::NotSupported => "it isn't code, Markdown, text or a PDF.",
            Self::TooLarge => "it's too large to read.",
        }
    }
}

/// Source file extensions read as code (lowercase).
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "java", "kt", "kts", "swift", "c",
    "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "cs", "rb", "php", "scala", "clj", "ex",
    "exs", "erl", "hs", "ml", "mli", "fs", "lua", "sh", "bash", "zsh", "sql", "proto", "toml",
    "yaml", "yml", "json", "zig", "nim", "dart", "r", "jl", "sol", "vue", "svelte", "css", "scss",
    "html", "gradle", "cmake", "tf",
];

/// Source files known by their whole name.
const CODE_NAMES: &[&str] = &["Makefile", "Dockerfile", "Justfile", "justfile", "BUILD"];

/// Checks `path` (from a drop, the chooser, the resolver or a project's list).
pub fn validate(path: &Path, home: &Path) -> Result<ValidSource, Refusal> {
    if path.as_os_str().len() > SOURCE_PATH_MAX_BYTES {
        return Err(Refusal::TooLong);
    }
    if !path.is_absolute() {
        return Err(Refusal::NotAbsolute);
    }
    if has_control_character(path) {
        return Err(Refusal::ControlCharacter);
    }
    // Refused before the file system is asked whether it exists.
    if is_credential(path) {
        return Err(Refusal::Credentials);
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Refusal::Missing
        } else {
            Refusal::Unreadable
        }
    })?;
    if has_control_character(&canonical) {
        return Err(Refusal::ControlCharacter);
    }
    if is_credential(&canonical) {
        return Err(Refusal::Credentials);
    }
    let metadata = std::fs::metadata(&canonical).map_err(|_| Refusal::Unreadable)?;
    // Compared canonically, so a home folder behind a symlink still reads as `~`.
    let home = &std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let title = canonical
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| canonical.display().to_string());
    let (kind, files_estimate) = if metadata.is_dir() {
        match too_broad(&canonical, home) {
            Some(TooBroad::HomeOrAbove) => return Err(Refusal::HoldsEverything),
            Some(TooBroad::AppData) => return Err(Refusal::AppData),
            None => (SourceKind::CodeFolder, count_files(&canonical)),
        }
    } else {
        if canonical
            .parent()
            .and_then(|parent| too_broad(parent, home))
            == Some(TooBroad::AppData)
        {
            return Err(Refusal::AppData);
        }
        let kind = file_kind(&canonical).ok_or(Refusal::NotSupported)?;
        if metadata.len() > size_bound(kind) {
            return Err(Refusal::TooLarge);
        }
        (kind, None)
    };
    Ok(ValidSource {
        shown: shown_path(&canonical, home),
        reach: reach(&canonical, kind, home),
        path: canonical,
        kind,
        title,
        files_estimate,
    })
}

/// A control character anywhere in the path's bytes (a newline, a tab, DEL…).
fn has_control_character(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().iter().any(u8::is_ascii_control)
}

fn file_kind(path: &Path) -> Option<SourceKind> {
    let name = path.file_name()?.to_string_lossy();
    if CODE_NAMES.contains(&name.as_ref()) {
        return Some(SourceKind::CodeFile);
    }
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let format = match extension.as_str() {
        "md" | "markdown" | "mdx" => DocumentFormat::Markdown,
        "txt" | "text" | "rst" | "adoc" => DocumentFormat::Text,
        "pdf" => DocumentFormat::Pdf,
        code if CODE_EXTENSIONS.contains(&code) => return Some(SourceKind::CodeFile),
        _ => return None,
    };
    Some(SourceKind::Document { format })
}

fn size_bound(kind: SourceKind) -> u64 {
    match kind {
        SourceKind::CodeFolder | SourceKind::CodeFile => SOURCE_CODE_FILE_MAX_BYTES,
        SourceKind::Document {
            format: DocumentFormat::Markdown | DocumentFormat::Text,
        } => SOURCE_TEXT_FILE_MAX_BYTES,
        SourceKind::Document {
            format: DocumentFormat::Pdf,
        } => SOURCE_PDF_MAX_BYTES,
    }
}

/// Files in a folder, `.gitignore` respected and credential folders skipped; `None` past
/// [`SOURCE_FILES_COUNTED_MAX`].
fn count_files(folder: &Path) -> Option<u32> {
    let mut count = 0_u32;
    let walk = WalkBuilder::new(folder)
        .hidden(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| !is_credential(entry.path()))
        .build();
    for entry in walk.flatten() {
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            count += 1;
            if count > SOURCE_FILES_COUNTED_MAX {
                return None;
            }
        }
    }
    Some(count)
}

/// `path` as the tray shows it: `~/…` under home, shortened to its last three parts after `…/`
/// when the whole is longer than [`SOURCE_SHOWN_FULL_MAX_BYTES`].
#[must_use]
pub fn shown_path(path: &Path, home: &Path) -> String {
    let (lead, rest) = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => return "~".to_owned(),
        Ok(rest) => ("~/", rest),
        Err(_) => ("/", path.strip_prefix("/").unwrap_or(path)),
    };
    let parts: Vec<String> = rest
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let whole = format!("{lead}{}", parts.join("/"));
    if whole.len() <= SOURCE_SHOWN_FULL_MAX_BYTES || parts.len() <= 3 {
        return whole;
    }
    format!("{lead}…/{}", parts[parts.len() - 3..].join("/"))
}

/// `path` as `sources.md` writes it: `~/…` under home (whole), else absolute.
#[must_use]
pub fn written_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A path as `sources.md` wrote it, back to an absolute one; `None` when it is neither absolute
/// nor under home.
#[must_use]
pub fn expanded_path(written: &str, home: &Path) -> Option<PathBuf> {
    if written == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = written.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    let path = PathBuf::from(written);
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Refusal, expanded_path, shown_path, validate, written_path};

    /// Finding 21: a newline in a shared path would add a line to a project's `sources.md`
    /// (`- code folder: /etc`): refused before the file system is asked, and where a symlink
    /// leads.
    #[test]
    fn a_path_with_a_control_character_is_refused() {
        let root = std::env::temp_dir().join(format!("boardai-control-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let named = home.join("notes\n- code folder: ~/.ssh");
        std::fs::create_dir_all(&named).expect("a folder with a newline in its name");
        for path in [
            named.clone(),
            home.join("tab\there"),
            home.join("del\u{7f}"),
        ] {
            assert_eq!(
                validate(&path, &home),
                Err(Refusal::ControlCharacter),
                "{path:?}"
            );
        }
        let link = home.join("plain");
        std::os::unix::fs::symlink(&named, &link).expect("a symlink");
        assert_eq!(validate(&link, &home), Err(Refusal::ControlCharacter));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn paths_are_shown_under_home_and_shortened() {
        let home = Path::new("/Users/me");
        assert_eq!(
            shown_path(Path::new("/Users/me/Papers/foundationdb.pdf"), home),
            "~/Papers/foundationdb.pdf"
        );
        assert_eq!(
            shown_path(
                Path::new("/Users/me/Documents/Projects/database/infinitydb/crates/inf-alloc"),
                home
            ),
            "~/…/infinitydb/crates/inf-alloc"
        );
        assert_eq!(shown_path(Path::new("/opt/src/x"), home), "/opt/src/x");
        assert_eq!(
            written_path(Path::new("/Users/me/code/a"), home),
            "~/code/a"
        );
        assert_eq!(
            expanded_path("~/code/a", home).as_deref(),
            Some(Path::new("/Users/me/code/a"))
        );
        assert_eq!(expanded_path("code/a", home), None);
    }
}
