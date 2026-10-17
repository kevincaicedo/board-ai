//! Sources through the orchestrator (D-184, D-185): a folder dropped on the window becomes
//! readable, its overview brief runs through a (fake) Claude Code in that folder, every cited
//! line is read natively, and the brief reaches the talker fenced with the next question.

use std::sync::Arc;

use boardai_core::history::{Message, UserPart};
use boardai_core::protocol::UiEvent;
use boardai_core::sources::SourceState;

use boardai_core::sources::DocumentFormat;

use super::{ScriptedTalker, close, run_until, start_configured};
use crate::cancel::CancelToken;
use crate::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, DEFAULT_RESEARCH_MODEL, HarnessPool, HarnessTimeouts,
};
use crate::session::{SessionParts, SourceParts, say_typed};
use crate::sources::{ReaderEvent, ReaderStream, SourceReader, validate};
use crate::tools::{RepoRoots, RepoTools};

fn fixture(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

fn states(events: &[UiEvent]) -> Vec<SourceState> {
    events
        .iter()
        .filter_map(|event| match event {
            UiEvent::Sources { tray } => tray.session.first().map(|row| row.state.clone()),
            UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Caption { .. }
            | UiEvent::Thread { .. }
            | UiEvent::OpStart { .. }
            | UiEvent::Freeze { .. }
            | UiEvent::Resume { .. }
            | UiEvent::LayerPush { .. }
            | UiEvent::LayerPop { .. }
            | UiEvent::Camera { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. }
            | UiEvent::Suggestion { .. }
            | UiEvent::SessionStart { .. }
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings
            | UiEvent::SessionEnd
            | UiEvent::DrawMode { .. }
            | UiEvent::UserShape { .. }
            | UiEvent::UserShapeRemove { .. }
            | UiEvent::LabelHint { .. }
            | UiEvent::PageTurn { .. }
            | UiEvent::Diagram { .. }
            | UiEvent::SourceDrop { .. } => None,
        })
        .collect()
}

/// A folder to share, a fake Claude Code that answers the brief fixture, and a session whose
/// code folder is elsewhere.
struct Setup {
    root: std::path::PathBuf,
    home: std::path::PathBuf,
    greeter: std::path::PathBuf,
    fake: std::path::PathBuf,
}

fn setup(label: &str) -> Setup {
    let root = std::env::temp_dir().join(format!("boardai-session-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let greeter = home.join("code/greeter");
    std::fs::create_dir_all(greeter.join("src")).expect("a folder");
    std::fs::write(
        greeter.join("src/lib.rs"),
        "pub fn greet() -> &'static str {\n    \"hello\"\n}\n",
    )
    .expect("a file");
    std::fs::write(greeter.join("README.md"), "# Greeter\n").expect("a file");
    std::fs::create_dir_all(home.join("code/other")).expect("a folder");
    let fake = root.join("fake");
    std::fs::create_dir_all(&fake).expect("a folder");
    Setup {
        root,
        home,
        greeter,
        fake,
    }
}

fn configure(setup: &Setup, readers: Vec<Arc<dyn SourceReader>>) -> impl FnOnce(&mut SessionParts) {
    let environment = vec![
        ("FAKE_FOLDER".into(), setup.fake.clone().into()),
        ("FAKE_FIXTURE".into(), fixture("claude/brief.jsonl").into()),
        ("FAKE_MODE".into(), "replay".into()),
    ];
    let other = setup.home.join("code/other");
    let home = setup.home.clone();
    move |parts| {
        let roots = RepoRoots::open(std::slice::from_ref(&other)).expect("a root");
        parts.tools = RepoTools::new(roots);
        let claude = ClaudeCodeHarness::new(ClaudeCodeConfig {
            program: fixture("harness/fake-harness.sh"),
            repo: None,
            environment,
            research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
        });
        parts.harness = Some(HarnessPool::new(
            Arc::new(claude),
            HarnessTimeouts::default(),
        ));
        parts.sources = SourceParts {
            harness_name: Some("Claude Code".into()),
            readers,
            ..SourceParts::in_memory(home)
        };
    }
}

/// Every user-side text of the first request the talker got.
fn first_request_texts(talker: &ScriptedTalker) -> Vec<String> {
    let request = talker.requests().remove(0);
    request
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { parts } => Some(parts.clone()),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .flatten()
        .filter_map(|part| match part {
            UserPart::Text { text } => Some(text),
            UserPart::ToolResult { .. } => None,
        })
        .collect()
}

/// Real time: the harness is a subprocess, which a paused clock would time out at once.
#[tokio::test]
async fn a_dropped_folder_is_researched_in_place_and_its_brief_reaches_the_talker() {
    let setup = setup("sources");
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("The greeter has one function.");
    let harness = start_configured(
        Arc::clone(&talker),
        false,
        None,
        configure(&setup, Vec::new()),
    );
    let source = validate(&setup.greeter, &setup.home).expect("a valid folder");
    assert!(harness.session.share(source, false).is_ok());
    let recorder = Arc::clone(&harness.recorder);
    let ready = run_until(15_000, || {
        states(&recorder.events()).iter().any(
            |state| matches!(state, SourceState::Ready { detail } if detail.starts_with("Overview ready")),
        )
    })
    .await;
    assert!(ready, "{:?}", states(&recorder.events()));
    // The log keeps every tray; the bridge may coalesce a fast one into the next frame's.
    assert!(
        states(&recorder.logged())
            .iter()
            .any(|state| matches!(state, SourceState::Reading { files_read: 1, .. })),
        "the tray followed the harness's reads"
    );
    let arguments = std::fs::read_to_string(setup.fake.join("arguments")).unwrap_or_default();
    assert!(
        arguments.contains("Answer only in the Markdown shape"),
        "{arguments}"
    );
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "How does the greeter work?").await.is_ok());
    assert!(run_until(5_000, || !talker.requests().is_empty()).await);
    let fenced = first_request_texts(&talker).iter().any(|text| {
        text.contains("<task-output>\n# Brief: greeter overview") && text.contains("src/lib.rs:1-3")
    });
    assert!(
        fenced,
        "the brief went to the talker fenced, its citation checked"
    );
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&setup.root);
}

/// The reader seam (T-031 fills it): a document shared goes to the reader that takes its
/// format, the tray follows its reports, and its digest reaches the talker fenced.
struct FakeReader;

impl SourceReader for FakeReader {
    fn name(&self) -> &str {
        "the document reader"
    }

    fn reads(&self, format: DocumentFormat) -> bool {
        format == DocumentFormat::Markdown
    }

    fn read(&self, _path: std::path::PathBuf, _cancel: CancelToken) -> ReaderStream {
        Box::pin(futures_util::stream::iter([
            ReaderEvent::Reading {
                read: 1,
                of: Some(2),
            },
            ReaderEvent::Partial {
                reason: "1 figure unread".into(),
                digest: "# Greeter\nOne section.".into(),
            },
        ]))
    }
}

#[tokio::test]
async fn a_shared_document_goes_to_its_reader_and_the_tray_follows() {
    let setup = setup("reader");
    let talker = Arc::new(ScriptedTalker::default());
    talker.says("It has one section.");
    let readers: Vec<Arc<dyn SourceReader>> = vec![Arc::new(FakeReader)];
    let harness = start_configured(Arc::clone(&talker), false, None, configure(&setup, readers));
    let readme = validate(&setup.greeter.join("README.md"), &setup.home).expect("a document");
    assert!(harness.session.share(readme, false).is_ok());
    let recorder = Arc::clone(&harness.recorder);
    let partial = run_until(5_000, || {
        states(&recorder.events()).iter().any(
            |state| matches!(state, SourceState::Partial { reason } if reason == "1 figure unread"),
        )
    })
    .await;
    assert!(partial, "{:?}", states(&recorder.events()));
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "What does the README say?").await.is_ok());
    assert!(run_until(5_000, || !talker.requests().is_empty()).await);
    assert!(
        first_request_texts(&talker)
            .iter()
            .any(|text| text.contains("<task-output>\n# Greeter\nOne section.")),
        "the digest went to the talker fenced"
    );
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&setup.root);
}
