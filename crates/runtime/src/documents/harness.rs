//! A document read by the coding harness (D-255): Claude Code reads the PDF itself, figures
//! included (its Read tool reads PDF pages), in the document's folder and held to the
//! document's reach like a brief (D-226), and answers in the digest's shape.

use std::path::{Path, PathBuf};

use boardai_core::harness::{HarnessKind, HarnessTask};
use boardai_core::sources::digest::{DigestScope, request_prompt};

use crate::cancel::CancelToken;
use crate::harness::{Answer, HarnessPool, HarnessStream, Workspace};
use crate::limits::DIGEST_WORDS_MAX;

/// Starts the harness on the document at `path`; `reach` is where it may read.
#[must_use]
pub fn start(
    pool: &HarnessPool,
    path: &Path,
    reach: PathBuf,
    cancel: CancelToken,
) -> HarnessStream {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = path
        .parent()
        .map_or_else(|| reach.clone(), Path::to_path_buf);
    let task = HarnessTask {
        kind: HarnessKind::Research,
        brief: request_prompt(
            &name,
            DigestScope::Whole {
                words_max: DIGEST_WORDS_MAX,
            },
        ),
    };
    pool.run_in(
        task,
        Workspace {
            folder,
            reach,
            answer: Answer::Brief,
        },
        cancel,
    )
}
