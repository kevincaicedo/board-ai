//! File operations on the vault. Every write is atomic (a temporary file in the same folder,
//! fsync, rename, then a best-effort fsync of the folder) and takes [`Redacted`] text only.
//! These run on blocking threads; the store actor never calls them on an async thread.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::redact::Redacted;

const TEMPORARY_PREFIX: &str = ".boardai-";
const TEMPORARY_SUFFIX: &str = ".tmp";

/// What reading a vault file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileRead {
    Missing,
    /// Larger than the caller's bound; not read.
    TooLarge {
        size_bytes: u64,
    },
    /// The text, with invalid UTF-8 replaced when `lossy` (such a file is indexed but never
    /// rewritten, so a hand-made file is never corrupted by an edit).
    Text {
        text: String,
        lossy: bool,
    },
}

/// Replaces `path` with `contents` atomically: a reader sees the old file or the new one,
/// never a mix, and a crash leaves at worst a temporary file that the next start removes.
pub(crate) fn write_atomic(path: &Path, contents: &Redacted) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let folder = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent folder"))?;
    fs::create_dir_all(folder)?;
    let temporary = folder.join(format!(
        "{TEMPORARY_PREFIX}{}-{}{TEMPORARY_SUFFIX}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents.as_str().as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    sync_folder(folder);
    Ok(())
}

/// Reads a whole file of at most `max_bytes`.
pub(crate) fn read_text(path: &Path, max_bytes: u64) -> io::Result<FileRead> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FileRead::Missing),
        Err(error) => return Err(error),
    };
    let size_bytes = file.metadata()?.len();
    if size_bytes > max_bytes {
        return Ok(FileRead::TooLarge { size_bytes });
    }
    let mut bytes = Vec::new();
    // The file may grow between the size check and the read; the bound still holds.
    Read::take(&mut file, max_bytes).read_to_end(&mut bytes)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => FileRead::Text { text, lossy: false },
        Err(error) => FileRead::Text {
            text: String::from_utf8_lossy(error.as_bytes()).into_owned(),
            lossy: true,
        },
    })
}

/// Removes a file; a missing file is not an error. Returns whether it existed.
pub(crate) fn remove_file(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(folder) = path.parent() {
                sync_folder(folder);
            }
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Creates and removes a probe file, so an unwritable vault fails at startup, not mid-session.
pub(super) fn probe_writable(root: &Path) -> io::Result<()> {
    let probe = root.join(format!(
        "{TEMPORARY_PREFIX}probe-{}{TEMPORARY_SUFFIX}",
        std::process::id()
    ));
    File::create(&probe)?;
    fs::remove_file(&probe)
}

/// Removes temporary files left by a crash mid-write, in the vault's folders and session
/// folders. Failures are ignored: a leftover temporary file is harmless and is never indexed.
pub(super) fn remove_stale_temporaries(root: &Path) {
    let mut folders = vec![root.to_path_buf()];
    for folder in [
        super::SESSIONS,
        super::NOTES,
        super::PROJECTS,
        super::MEMORY,
    ] {
        folders.push(root.join(folder));
    }
    if let Ok(sessions) = fs::read_dir(root.join(super::SESSIONS)) {
        folders.extend(sessions.flatten().map(|entry| entry.path()));
    }
    for folder in folders {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(TEMPORARY_PREFIX) && name.ends_with(TEMPORARY_SUFFIX) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Makes a rename or removal durable. Best effort: some file systems refuse to sync a folder,
/// and the file itself is already synced.
pub(crate) fn sync_folder(folder: &Path) {
    if let Ok(handle) = File::open(folder) {
        let _ = handle.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{FileRead, read_text, remove_file, write_atomic};
    use crate::store::redact::redact_text;

    #[test]
    fn writes_are_atomic_replacements_and_leave_no_temporaries() {
        let folder = std::env::temp_dir().join(format!("boardai-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let path = folder.join("nested/file.md");
        assert!(write_atomic(&path, &redact_text("first\n")).is_ok());
        assert!(write_atomic(&path, &redact_text("second\n")).is_ok());
        assert_eq!(
            read_text(&path, 1024).ok(),
            Some(FileRead::Text {
                text: "second\n".to_owned(),
                lossy: false
            })
        );
        assert_eq!(
            read_text(&path, 3).ok(),
            Some(FileRead::TooLarge { size_bytes: 7 })
        );
        let names: Vec<_> = std::fs::read_dir(folder.join("nested"))
            .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default();
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(remove_file(&path).ok(), Some(true));
        assert_eq!(remove_file(&path).ok(), Some(false));
        assert_eq!(read_text(&path, 10).ok(), Some(FileRead::Missing));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
