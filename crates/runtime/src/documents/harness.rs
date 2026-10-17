//! A document read by the coding harness (D-255): Claude Code reads the PDF itself, figures
//! included (its Read tool reads PDF pages), and answers in the digest's shape.
//!
//! It reads a private folder that holds only what sharing the document consents to (D-231):
//! the document, and the images a Markdown document links (canonical, inside its folder), each
//! at the path its link names. Never the document's own folder, so its neighbours stay unread
//! whatever the harness's rules allow inside its working folder. The folder is made under the
//! system's temporary folder (`0700`), its files hard-linked (copied when the volume won't link),
//! and removed when its guard is dropped: when the run ends, is cancelled, or the session ends.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use boardai_core::harness::{HarnessKind, HarnessTask};
use boardai_core::sources::digest::{DigestScope, request_prompt};

use super::layer::LocalImage;
use crate::cancel::CancelToken;
use crate::harness::{Answer, HarnessPool, HarnessStream, Workspace};
use crate::limits::DIGEST_WORDS_MAX;

/// Private folders made in this run, for unique names.
static PRIVATE_FOLDERS: AtomicU64 = AtomicU64::new(0);

/// The private folder a document's harness reads; removed when dropped.
#[derive(Debug)]
pub struct PrivateCopy {
    /// Canonical, as Claude Code resolves its working folder (`/private/var/…`), so its rules
    /// and the paths it reports name the same folder.
    folder: PathBuf,
}

impl PrivateCopy {
    /// Makes the folder and places the document and `images` in it, on the blocking pool. The
    /// guard exists before the first file is placed: a copy cut short, or one whose caller went
    /// away meanwhile (its result is dropped with the task), leaves nothing behind.
    pub async fn make(document: PathBuf, images: Vec<LocalImage>) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || Self::make_now(&document, &images))
            .await
            .map_err(|_| "the document's private copy wasn't made".to_owned())?
    }

    fn make_now(document: &Path, images: &[LocalImage]) -> Result<Self, String> {
        let index = PRIVATE_FOLDERS.fetch_add(1, Ordering::Relaxed);
        let folder =
            std::env::temp_dir().join(format!("boardai-document-{}-{index}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .map_err(|error| format!("its private folder wasn't made: {error}"))?;
        let copy = Self {
            folder: std::fs::canonicalize(&folder).unwrap_or(folder),
        };
        let name = document
            .file_name()
            .ok_or_else(|| "the document has no name".to_owned())?;
        place(document, &copy.folder.join(name))
            .map_err(|error| format!("the document wasn't placed in its folder: {error}"))?;
        for image in images {
            let target = copy.folder.join(&image.linked);
            let placed = target
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| place(&image.path, &target));
            if let Err(error) = placed {
                // The image stays unread, as one the model never got: the digest lists it.
                tracing::warn!(%error, "a linked image wasn't placed beside the document");
            }
        }
        Ok(copy)
    }
}

impl Drop for PrivateCopy {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.folder)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a document's private folder wasn't removed");
        }
    }
}

/// A hard link, or a copy when the file is on another volume (or the volume won't link).
fn place(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

/// Starts the harness on the document `name` in `copy`, held to that folder (D-226, D-231).
#[must_use]
pub fn start(
    pool: &HarnessPool,
    copy: &PrivateCopy,
    name: &str,
    cancel: CancelToken,
) -> HarnessStream {
    let task = HarnessTask {
        kind: HarnessKind::Research,
        brief: request_prompt(
            name,
            DigestScope::Whole {
                words_max: DIGEST_WORDS_MAX,
            },
        ),
    };
    pool.run_in(
        task,
        Workspace {
            folder: copy.folder.clone(),
            reach: copy.folder.clone(),
            answer: Answer::Brief,
        },
        cancel,
    )
}
