//! Documents in the orchestrator (D-186, D-256, D-259): a shared document goes to its reader
//! unless a fresh digest is cached for it; a digest that arrives is stamped and cached (in the
//! project's vault folder when the document is kept there); the talker's page reads come from
//! the reader's text layer.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::input::Input;
use boardai_core::sources::digest;
use boardai_core::sources::{DocumentPage, SourceId, SourceInput, SourceKind};
use futures_util::FutureExt;

use super::Orchestrator;
use super::sources::SourceMessage;
use crate::limits::SOURCE_JOB_TIMEOUT_MS;
use crate::session::inbox::Envelope;
use crate::sources::{SourceReader, stamp};

/// The cache's topic for a document's digest, beside a code source's briefs (D-259).
pub(crate) const DIGEST_TOPIC: &str = "digest";

impl Orchestrator {
    /// The reader that takes this document's format.
    fn reader_for(&self, source: SourceId) -> Option<Arc<dyn SourceReader>> {
        let Some(SourceKind::Document { format }) = self.desk.kind(source) else {
            return None;
        };
        self.readers
            .iter()
            .find(|reader| reader.reads(format))
            .cloned()
    }

    /// A readable document: its cached digest when its stamp still matches the file, else its
    /// reader (none: it waits in the tray).
    pub(super) fn read_document(&mut self, source: SourceId) {
        let (Some(reader), Some(path)) = (
            self.reader_for(source),
            self.desk.path(source).map(std::path::Path::to_path_buf),
        ) else {
            return;
        };
        let Some(cached) = self.desk.cached(&path, DIGEST_TOPIC).cloned() else {
            self.run_reader(source, reader);
            return;
        };
        let job = async move {
            let now = tokio::task::spawn_blocking(move || stamp(&path)).await.ok();
            let fresh = now.as_ref() == Some(&cached.stamp);
            Some(Envelope::Source(SourceMessage::DigestChecked {
                source,
                markdown: fresh.then_some(cached.markdown),
            }))
        };
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
            self.run_reader(source, reader);
        }
    }

    fn run_reader(&mut self, source: SourceId, reader: Arc<dyn SourceReader>) {
        let Some(path) = self.desk.path(source).map(std::path::Path::to_path_buf) else {
            return;
        };
        let cancel = crate::cancel::CancelToken::new();
        let events = reader.read(path, cancel.clone());
        let name = reader.name().to_owned();
        if !self
            .work
            .run_reader(source, name, events, cancel, &self.inbox)
        {
            tracing::warn!("a document waits: as many readers run as may");
        }
    }

    /// A cached digest checked against the file: fresh, it answers at once (no model is asked
    /// again); stale or gone, the reader reads the document.
    pub(super) async fn on_digest_checked(&mut self, source: SourceId, markdown: Option<String>) {
        let Some(markdown) = markdown else {
            if let Some(path) = self.desk.path(source).map(std::path::Path::to_path_buf) {
                self.desk.evict(&path, DIGEST_TOPIC);
            }
            if let Some(reader) = self.reader_for(source) {
                self.run_reader(source, reader);
            }
            return;
        };
        let state = digest::parse(&markdown).tray_state();
        let read = SourceInput::DocumentRead {
            source,
            state,
            digest: Some(markdown),
        };
        self.step(Input::Source { source: read }).await;
    }

    /// A reader's digest: stamped and cached like a brief, and saved in the project when the
    /// document is kept there (D-259).
    pub(super) fn on_digested(&mut self, source: SourceId, markdown: String) {
        self.stamp_brief(source, DIGEST_TOPIC.to_owned(), markdown);
    }

    /// `ReadDocument`: pages from the reader's text layer, answered as `DocumentPages` within
    /// the source job's bound (past it, or with too many jobs, no pages).
    pub(super) fn read_pages(&mut self, request: u32, source: SourceId, first: u32, last: u32) {
        let empty = SourceInput::DocumentPages {
            request,
            pages: Vec::new(),
            page_count: None,
        };
        let (Some(reader), Some(path)) = (
            self.reader_for(source),
            self.desk.path(source).map(std::path::Path::to_path_buf),
        ) else {
            self.own_answers.push_back(Input::Source { source: empty });
            return;
        };
        let job = async move {
            // Inside the job's own bound, so the held read is always answered (with no pages
            // when the text couldn't be read in time).
            let bound = Duration::from_millis(SOURCE_JOB_TIMEOUT_MS.saturating_sub(500));
            let read = tokio::time::timeout(bound, reader.pages(path, first, last))
                .await
                .unwrap_or_default();
            let source = SourceInput::DocumentPages {
                request,
                pages: read
                    .pages
                    .into_iter()
                    .map(|(page, text)| DocumentPage { page, text })
                    .collect(),
                page_count: read.page_count,
            };
            Some(Envelope::Input(Input::Source { source }))
        };
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
            self.own_answers.push_back(Input::Source { source: empty });
        }
    }
}
