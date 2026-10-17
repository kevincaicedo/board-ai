//! Documents in the orchestrator (D-186, D-256, D-259): a shared document goes to its reader
//! unless a fresh whole digest is cached for it; a document shared while as many readers run as
//! may waits its turn, and starts when one ends; a whole digest that arrives is stamped and
//! cached (in the project's vault folder when the document is kept there); the talker's page
//! reads come from the reader, answered within the source job's bound.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::input::Input;
use boardai_core::sources::digest;
use boardai_core::sources::{DocumentPage, SourceId, SourceInput, SourceKind, SourceState};
use futures_util::FutureExt;

use super::Orchestrator;
use super::sources::SourceMessage;
use crate::limits::{DOCUMENTS_WAITING_MAX, SOURCE_JOB_TIMEOUT_MS};
use crate::session::inbox::Envelope;
use crate::session::work::ReaderRoom;
use crate::sources::{PagesRead, PagesUnread, SourceReader, stamp};

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
        let unchecked = Envelope::Source(SourceMessage::DigestChecked {
            source,
            markdown: None,
        });
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(unchecked), &self.inbox)
        {
            self.run_reader(source, reader);
        }
    }

    /// Starts the reader, or puts the document in line when as many read as may.
    fn run_reader(&mut self, source: SourceId, reader: Arc<dyn SourceReader>) {
        match self.work.reader_room(source) {
            ReaderRoom::Reading => {}
            ReaderRoom::Full => self.wait_for_reader(source),
            ReaderRoom::Free => {
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
                    self.wait_for_reader(source);
                }
            }
        }
    }

    /// The document waits for a reader to end, in the order shared.
    fn wait_for_reader(&mut self, source: SourceId) {
        if self.waiting_readers.contains(&source) {
            return;
        }
        if self.waiting_readers.len() >= DOCUMENTS_WAITING_MAX
            && let Some(dropped) = self.waiting_readers.pop_front()
        {
            tracing::warn!(
                source = dropped.0,
                "a waiting document was dropped from the line; it is read when shared again"
            );
        }
        self.waiting_readers.push_back(source);
    }

    /// A reader ended: the first waiting document still shared starts.
    pub(super) fn on_reader_ended(&mut self) {
        while self.work.has_reader_room() {
            let Some(source) = self.waiting_readers.pop_front() else {
                return;
            };
            if !self.desk.is_readable(source) {
                continue;
            }
            if let Some(reader) = self.reader_for(source) {
                self.run_reader(source, reader);
            }
        }
    }

    /// Discarded documents leave the line.
    pub(super) fn stop_waiting(&mut self, sources: &[SourceId]) {
        self.waiting_readers
            .retain(|waiting| !sources.contains(waiting));
    }

    /// A cached digest checked against the file: fresh and whole, it answers at once (no model
    /// is asked again); stale, gone, or partial (one cached before only whole ones were), the
    /// reader reads the document.
    pub(super) async fn on_digest_checked(&mut self, source: SourceId, markdown: Option<String>) {
        let fresh = markdown.and_then(|markdown| {
            let state = digest::parse(&markdown).tray_state();
            matches!(state, SourceState::Ready { .. }).then_some((state, markdown))
        });
        let Some((state, markdown)) = fresh else {
            if let Some(path) = self.desk.path(source).map(std::path::Path::to_path_buf) {
                self.desk.evict(&path, DIGEST_TOPIC);
            }
            if let Some(reader) = self.reader_for(source) {
                self.run_reader(source, reader);
            }
            return;
        };
        let read = SourceInput::DocumentRead {
            source,
            state,
            digest: Some(markdown),
        };
        self.step(Input::Source { source: read }).await;
    }

    /// A reader's whole digest: stamped and cached like a brief, and saved in the project when
    /// the document is kept there (D-259).
    pub(super) fn on_digested(&mut self, source: SourceId, markdown: String) {
        self.stamp_brief(source, DIGEST_TOPIC.to_owned(), markdown);
    }

    /// `ReadDocument`: pages from the reader, answered as `DocumentPages` within the source
    /// job's bound; past it, with too many jobs, or with no reader, the answer says the pages
    /// weren't read.
    pub(super) fn read_pages(&mut self, request: u32, source: SourceId, first: u32, last: u32) {
        let (Some(reader), Some(path)) = (
            self.reader_for(source),
            self.desk.path(source).map(std::path::Path::to_path_buf),
        ) else {
            let unread = document_pages(request, Err(PagesUnread::NoReader));
            self.own_answers.push_back(unread);
            return;
        };
        let job = async move {
            // Inside the job's own bound, so the reader's answer, not the fallback, says why.
            let bound = Duration::from_millis(SOURCE_JOB_TIMEOUT_MS.saturating_sub(500));
            let read = tokio::time::timeout(bound, reader.pages(path, first, last))
                .await
                .unwrap_or(Err(PagesUnread::TimedOut));
            Some(Envelope::Input(document_pages(request, read)))
        };
        let timed_out = Envelope::Input(document_pages(request, Err(PagesUnread::TimedOut)));
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(timed_out), &self.inbox)
        {
            let busy = document_pages(request, Err(PagesUnread::Busy));
            self.own_answers.push_back(busy);
        }
    }
}

/// The answer to `ReadDocument` as the core takes it. Pages that weren't read are not pages
/// with no text: the core's `DocumentPages` gains that outcome with harden-core-3 (finding 8);
/// until it does, an unread read answers no pages, and why is logged here.
fn document_pages(request: u32, read: Result<PagesRead, PagesUnread>) -> Input {
    let read = read.unwrap_or_else(|unread| {
        tracing::warn!(reason = %unread.reason(), "a page read was answered without its pages");
        PagesRead::default()
    });
    Input::Source {
        source: SourceInput::DocumentPages {
            request,
            pages: read
                .pages
                .into_iter()
                .map(|(page, text)| DocumentPage { page, text })
                .collect(),
            page_count: read.page_count,
        },
    }
}
