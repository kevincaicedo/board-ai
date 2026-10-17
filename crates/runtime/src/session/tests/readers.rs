//! Document readers through the orchestrator (review 3): a document shared while as many readers
//! run as may waits its turn and is read (finding 7: nothing started it, yet BoardAI said
//! "Reading …"); a partial digest is read again when shared again (finding 9); a session ended
//! while the harness reads a document leaves nothing behind (finding 22).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use boardai_core::protocol::UiEvent;
use boardai_core::sources::{DocumentFormat, SourceState};

use super::{ScriptedTalker, close, run_until, start_configured};
use crate::cancel::CancelToken;
use crate::session::{SessionParts, SourceParts};
use crate::sources::{ReaderEvent, ReaderStream, SourceReader, validate};

/// Answers each document after a short read, counting the reads it was asked for.
struct SlowReader(Arc<AtomicUsize>);

impl SourceReader for SlowReader {
    fn name(&self) -> &str {
        "BoardAI"
    }
    fn reads(&self, format: DocumentFormat) -> bool {
        format == DocumentFormat::Pdf
    }
    fn read(&self, _path: std::path::PathBuf, _cancel: CancelToken) -> ReaderStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures_util::stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            ReaderEvent::Ready {
                detail: "2 pages · text read".into(),
                digest: "# Digest: Notes\nRead: 2 pages; text only\n## Sections\n### Logs (p. 1)\nIt logs.\n".into(),
            }
        }))
    }
}

fn states(events: &[UiEvent]) -> Vec<SourceState> {
    events
        .iter()
        .rev()
        .find_map(|event| {
            let UiEvent::Sources { tray } = event else {
                return None;
            };
            Some(tray.session.iter().map(|row| row.state.clone()).collect())
        })
        .unwrap_or_default()
}

/// The first session row's id in the latest tray.
fn first_row(events: &[UiEvent]) -> Option<boardai_core::sources::SourceId> {
    events.iter().rev().find_map(|event| {
        let UiEvent::Sources { tray } = event else {
            return None;
        };
        Some(tray.session.first().map(|row| row.id))
    })?
}

/// A session whose one reader is `reader`, with `home` as the home folder.
fn start_reading(reader: Arc<dyn SourceReader>, home: &std::path::Path) -> super::Harness {
    let talker = Arc::new(ScriptedTalker::default());
    let configure_home = home.to_path_buf();
    start_configured(talker, false, None, move |parts: &mut SessionParts| {
        parts.sources = SourceParts {
            readers: vec![reader],
            ..SourceParts::in_memory(configure_home)
        };
    })
}

#[tokio::test]
async fn three_documents_dropped_at_once_are_all_read() {
    let root = std::env::temp_dir().join(format!("boardai-review3-readers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(home.join("papers")).expect("a folder");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/documents/small-text.pdf");
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        std::fs::copy(&fixture, home.join("papers").join(name)).expect("a copy");
    }
    let readings = Arc::new(AtomicUsize::new(0));
    let reader: Arc<dyn SourceReader> = Arc::new(SlowReader(Arc::clone(&readings)));
    let talker = Arc::new(ScriptedTalker::default());
    let configure_home = home.clone();
    let harness = start_configured(
        Arc::clone(&talker),
        false,
        None,
        move |parts: &mut SessionParts| {
            parts.sources = SourceParts {
                readers: vec![reader],
                ..SourceParts::in_memory(configure_home)
            };
        },
    );
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        let paper = validate(&home.join("papers").join(name), &home).expect("a document");
        assert!(harness.session.share(paper, false).is_ok());
    }
    let recorder = Arc::clone(&harness.recorder);
    let all_ready = run_until(5_000, || {
        let states = states(&recorder.events());
        states.len() == 3
            && states
                .iter()
                .all(|state| matches!(state, SourceState::Ready { .. }))
    })
    .await;
    let seen = states(&recorder.events());
    let asked = readings.load(Ordering::SeqCst);
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&root);
    assert!(all_ready, "reads asked: {asked}; tray: {seen:?}");
}

/// A reader whose model call failed: its text-only digest, figures unread.
struct FailedFiguresReader(Arc<AtomicUsize>);

impl SourceReader for FailedFiguresReader {
    fn name(&self) -> &str {
        "BoardAI"
    }
    fn reads(&self, format: DocumentFormat) -> bool {
        format == DocumentFormat::Pdf
    }
    fn read(&self, _path: std::path::PathBuf, _cancel: CancelToken) -> ReaderStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures_util::stream::iter([ReaderEvent::Partial {
            reason: "Text read · 1 figure needs a model that reads files".into(),
            digest: "# Digest: Notes\nRead: 2 pages; text only\n## Sections\n### Logs (p. 1)\nIt logs.\n## Figures\n### Figure 1 (p. 1): The log.\n".into(),
        }]))
    }
}

/// A partial digest (a model call that failed, or no model yet) is cached like a whole one, so
/// sharing the unchanged file again never asks the reader again: its figures stay unread.
#[tokio::test]
async fn a_partial_digest_is_not_reused_when_shared_again() {
    use boardai_core::protocol::UiCommand;
    use boardai_core::sources::tray::SourceAction;
    let root = std::env::temp_dir().join(format!("boardai-review3-partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(home.join("papers")).expect("a folder");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/documents/small-text.pdf");
    let path = home.join("papers/a.pdf");
    std::fs::copy(&fixture, &path).expect("a copy");
    let readings = Arc::new(AtomicUsize::new(0));
    let reader: Arc<dyn SourceReader> = Arc::new(FailedFiguresReader(Arc::clone(&readings)));
    let harness = start_reading(reader, &home);
    let recorder = Arc::clone(&harness.recorder);
    assert!(
        harness
            .session
            .share(validate(&path, &home).expect("valid"), false)
            .is_ok()
    );
    assert!(
        run_until(5_000, || matches!(
            states(&recorder.events()).first(),
            Some(SourceState::Partial { .. })
        ))
        .await
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let first = first_row(&recorder.events()).expect("a row");
    let remove = UiCommand::Sources {
        action: SourceAction::Remove { source: first },
    };
    assert!(harness.session.command(remove).is_ok());
    assert!(run_until(5_000, || first_row(&recorder.events()).is_none()).await);
    assert!(
        harness
            .session
            .share(validate(&path, &home).expect("valid"), false)
            .is_ok()
    );
    let _ = run_until(5_000, || {
        first_row(&recorder.events()).is_some_and(|id| id != first)
            && matches!(
                states(&recorder.events()).first(),
                Some(SourceState::Partial { .. })
            )
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let asked = readings.load(Ordering::SeqCst);
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        asked, 2,
        "the partial digest answered the second share: the figures were never asked again"
    );
}

/// A harness still reading a document, as Claude Code does for ~46 s: it records the folder it
/// was given and never answers.
struct StillReading(Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>);

impl crate::harness::CodingHarness for StillReading {
    fn run(
        &self,
        _task: boardai_core::harness::HarnessTask,
        _cancel: CancelToken,
    ) -> crate::harness::HarnessStream {
        Box::pin(futures_util::stream::pending())
    }

    fn run_in(
        &self,
        _task: boardai_core::harness::HarnessTask,
        workspace: crate::harness::Workspace,
        _cancel: CancelToken,
    ) -> crate::harness::HarnessStream {
        if let Ok(mut folders) = self.0.lock() {
            folders.push(workspace.folder);
        }
        Box::pin(futures_util::stream::pending())
    }
}

/// Finding 22: the harness's read of a document isn't a task the core counts, so ending the
/// session cancels it without a word. It leaves nothing behind: its private folder is removed
/// and no digest is cached or saved (only whole digests are, so the document is read again the
/// next time it is shared).
#[tokio::test]
async fn ending_the_session_mid_read_leaves_nothing_behind() {
    use crate::documents::pdfkit::PdfKit;
    use crate::documents::{DocumentReader, ReaderConfig};
    use crate::harness::{HarnessPool, HarnessTimeouts};
    use crate::session::shelf::{MemoryShelf, SourceShelf};

    let root = std::env::temp_dir().join(format!("boardai-review3-ended-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(home.join("papers")).expect("a folder");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/documents/paper.pdf");
    std::fs::copy(&fixture, home.join("papers/paper.pdf")).expect("a copy");
    let folders = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pool = HarnessPool::new(
        Arc::new(StillReading(Arc::clone(&folders))),
        HarnessTimeouts::default(),
    );
    let reader: Arc<dyn SourceReader> = Arc::new(DocumentReader::new(ReaderConfig {
        file_models: Vec::new(),
        harness: Some((pool, "Claude Code".into())),
        home: home.clone(),
        pdfkit: PdfKit::default(),
    }));
    let shelf = MemoryShelf::default();
    let talker = Arc::new(ScriptedTalker::default());
    let configure_home = home.clone();
    let configure_shelf = shelf.clone();
    let harness = start_configured(
        Arc::clone(&talker),
        false,
        None,
        move |parts: &mut SessionParts| {
            parts.sources = SourceParts {
                readers: vec![reader],
                shelf: Arc::new(configure_shelf),
                ..SourceParts::in_memory(configure_home)
            };
        },
    );
    let paper = validate(&home.join("papers/paper.pdf"), &home).expect("a document");
    assert!(harness.session.share(paper, true).is_ok());
    let started = run_until(15_000, || {
        folders.lock().is_ok_and(|folders| !folders.is_empty())
    })
    .await;
    let folder = folders
        .lock()
        .ok()
        .and_then(|folders| folders.first().cloned());
    let recorder = Arc::clone(&harness.recorder);
    let _ = close(harness).await;
    let settled = states(&recorder.events()).iter().any(|state| {
        matches!(
            state,
            SourceState::Ready { .. } | SourceState::Partial { .. }
        )
    });
    let project = boardai_core::sources::ProjectId::parse("paper").expect("a project id");
    let saved = shelf
        .briefs(project, "~/papers/paper.pdf".into())
        .await
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(started, "the harness was reading");
    let folder = folder.expect("its private folder");
    assert!(!folder.exists(), "the private folder went with the session");
    assert!(!settled, "no digest from a read cut short");
    assert!(saved.is_empty(), "nothing was saved: {saved:?}");
}
