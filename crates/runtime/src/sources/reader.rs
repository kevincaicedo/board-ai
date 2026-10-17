//! The document reader seam (D-186, D-230): what reads a shared Markdown, text or PDF document,
//! reports its progress and digest, and reads its pages when the talker asks. The session runs a
//! reader for each document it takes, so the tray follows its reading and the talker gets the
//! digest (fenced, D-107). BoardAI's own is `documents::DocumentReader`.

use std::path::PathBuf;

use boardai_core::sources::{DocumentFormat, PagesUnread};
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

/// One report from a reader. A stream ends after `Ready`, `Partial` or `Failed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderEvent {
    /// Reading: `read` parts (pages, files) so far, of `of` when known; `by` is who reads now
    /// ("Gemini 3.8 Flash" while a model reads the figures), the reader itself when `None`.
    Reading {
        read: u32,
        of: Option<u32>,
        by: Option<String>,
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

/// Pages a reader read for the talker: `(page, text)` for each with text, in order, and the
/// document's length when known. A page left out has no text: a scan, or past the end. Pages it
/// couldn't read are the core's `PagesUnread` (D-274), never this with the pages left out, so
/// the talker isn't told an unread page is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PagesRead {
    pub pages: Vec<(u32, String)>,
    pub page_count: Option<u32>,
}

pub trait SourceReader: Send + Sync {
    /// The reader's name as the tray says it ("BoardAI").
    fn name(&self) -> &str;

    /// Whether it reads this format.
    fn reads(&self, format: DocumentFormat) -> bool;

    /// Reads the document at `path` (validated, canonical). Dropping the stream, or `cancel`,
    /// stops it.
    fn read(&self, path: PathBuf, cancel: CancelToken) -> ReaderStream;

    /// Pages `[first, last]` (1-based) of the PDF at `path`, from its extracted text (D-256).
    /// Bounded by the reader; the caller bounds it in time. A reader that keeps no text answers
    /// that it can't read pages.
    fn pages(
        &self,
        path: PathBuf,
        first: u32,
        last: u32,
    ) -> BoxFuture<'static, Result<PagesRead, PagesUnread>> {
        let _ = (path, first, last);
        Box::pin(std::future::ready(Err(PagesUnread::NoReader)))
    }
}
