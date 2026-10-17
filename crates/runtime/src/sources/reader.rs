//! The document reader seam (D-186, T-031): what reads a shared Markdown, text or PDF document
//! and reports its progress and digest. BoardAI registers documents now and shows them waiting
//! for a reader; T-031 implements this trait and the session runs it for each document it
//! takes, so the tray follows its reading and the talker gets the digest (fenced, D-107).

use std::path::PathBuf;

use boardai_core::sources::DocumentFormat;
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

/// One report from a reader. A stream ends after `Ready`, `Partial` or `Failed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderEvent {
    /// Reading: `read` parts (pages, sections) so far, of `of` when known.
    Reading {
        read: u32,
        of: Option<u32>,
    },
    /// Read whole: what the tray says ("14 pages · text, 4 tables and 9 of 9 figures read"),
    /// and the digest in Markdown.
    Ready {
        detail: String,
        digest: String,
    },
    /// Read, but something wasn't ("3 figures unread"); the digest of what was.
    Partial {
        reason: String,
        digest: String,
    },
    Failed {
        reason: String,
    },
}

pub type ReaderStream = BoxStream<'static, ReaderEvent>;

pub trait SourceReader: Send + Sync {
    /// The reader's name as the tray says it ("BoardAI", "Claude Code").
    fn name(&self) -> &str;

    /// Whether it reads this format.
    fn reads(&self, format: DocumentFormat) -> bool;

    /// Reads the document at `path` (validated, canonical). Dropping the stream, or `cancel`,
    /// stops it.
    fn read(&self, path: PathBuf, cancel: CancelToken) -> ReaderStream;
}
