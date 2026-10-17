//! Sources through the orchestrator (D-184, D-185): a folder dropped on the window becomes
//! readable, its overview brief runs through a (fake) Claude Code in that folder, every cited
//! line is read natively, and the brief reaches the talker fenced with the next question.

use std::sync::Arc;

use boardai_core::history::{Message, UserPart};
use boardai_core::protocol::UiEvent;
use boardai_core::sources::{ProjectId, SourceState};

use boardai_core::sources::DocumentFormat;

use super::{ScriptedTalker, close, run_until, start_configured};
use crate::cancel::CancelToken;
use crate::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, DEFAULT_RESEARCH_MODEL, HarnessPool, HarnessTimeouts,
};
use crate::session::shelf::MemoryShelf;
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
    configure_with(setup, readers, None, None)
}

/// [`configure`], with the fake harness answering after `delay` seconds, and a shelf to look in.
fn configure_with(
    setup: &Setup,
    readers: Vec<Arc<dyn SourceReader>>,
    delay: Option<&str>,
    shelf: Option<MemoryShelf>,
) -> impl FnOnce(&mut SessionParts) {
    let mut environment: Vec<(std::ffi::OsString, std::ffi::OsString)> = vec![
        ("FAKE_FOLDER".into(), setup.fake.clone().into()),
        ("FAKE_FIXTURE".into(), fixture("claude/brief.jsonl").into()),
        ("FAKE_MODE".into(), "replay".into()),
    ];
    if let Some(delay) = delay {
        environment.push(("FAKE_DELAY".into(), delay.into()));
    }
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
            model: None,
            sign_in: crate::harness::ClaudeSignIn::default(),
        });
        parts.harness = Some(HarnessPool::new(
            Arc::new(claude),
            HarnessTimeouts::default(),
        ));
        let mut sources = SourceParts {
            harness_name: Some("Claude Code".into()),
            readers,
            ..SourceParts::in_memory(home)
        };
        if let Some(shelf) = shelf {
            sources.shelf = Arc::new(shelf);
        }
        parts.sources = sources;
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

/// I10, the runtime's half (finding 3): a folder kept in a project, then "don't remember this
/// session" while its brief is still being read. The keep is undone, and the brief that arrives
/// afterwards is never saved in the project's folder, whatever the core asks.
#[tokio::test]
async fn a_brief_after_forgetting_the_session_is_not_saved_in_the_project() {
    use crate::session::shelf::SourceShelf;

    let setup = setup("forget");
    let talker = Arc::new(ScriptedTalker::default());
    let shelf = MemoryShelf::default();
    let configure = configure_with(&setup, Vec::new(), Some("2"), Some(shelf.clone()));
    let harness = start_configured(Arc::clone(&talker), false, None, configure);
    let source = validate(&setup.greeter, &setup.home).expect("a valid folder");
    assert!(harness.session.share(source, true).is_ok());
    let project = ProjectId::parse("greeter").expect("a project id");
    let mut kept = false;
    for _ in 0..100 {
        kept = !shelf
            .kept(project.clone())
            .await
            .unwrap_or_default()
            .is_empty();
        if kept {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(kept, "the folder was kept in its project");
    let link = harness.session.speech_in();
    assert!(
        say_typed(&link, "Don't remember this session.")
            .await
            .is_ok()
    );
    let recorder = Arc::clone(&harness.recorder);
    // A kept source's row is on the project's tab.
    let kept_states = |events: &[UiEvent]| -> Vec<SourceState> {
        events
            .iter()
            .filter_map(|event| {
                let UiEvent::Sources { tray } = event else {
                    return None;
                };
                tray.project.as_ref()
            })
            .flat_map(|project| project.sources.iter().map(|row| row.state.clone()))
            .collect()
    };
    let ready = run_until(15_000, || {
        kept_states(&recorder.events()).iter().any(
            |state| matches!(state, SourceState::Ready { detail } if detail.starts_with("Overview ready")),
        )
    })
    .await;
    assert!(ready, "{:?}", kept_states(&recorder.events()));
    // The brief is stamped and would be saved by jobs: give them their moment.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let briefs = shelf
        .briefs(project.clone(), "~/code/greeter".into())
        .await
        .unwrap_or_default();
    let lines = shelf.kept(project).await.unwrap_or_default();
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&setup.root);
    assert!(lines.is_empty(), "the keep was undone: {lines:?}");
    assert!(
        briefs.is_empty(),
        "no brief was saved after the forget: {briefs:?}"
    );
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
                by: None,
            },
            ReaderEvent::Partial {
                reason: "1 figure unread".into(),
                digest: "# Digest: Greeter\nRead: 2 lines; text only\n## Sections\n### Greeter \
                         (l. 1)\nOne section.\n## Figures\n### Figure 1 (l. 2): A picture.\n"
                    .into(),
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
        states(&recorder.events()).iter().any(|state| {
            matches!(state, SourceState::Partial { reason }
                if reason == "Text read · 1 image needs a model that reads files")
        })
    })
    .await;
    assert!(partial, "{:?}", states(&recorder.events()));
    let link = harness.session.speech_in();
    assert!(say_typed(&link, "What does the README say?").await.is_ok());
    assert!(run_until(5_000, || !talker.requests().is_empty()).await);
    assert!(
        first_request_texts(&talker)
            .iter()
            .any(|text| text.contains("<task-output>\n# Digest: Greeter")),
        "the digest went to the talker fenced"
    );
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&setup.root);
}

/// D-232's retry needs the repo reads offered: a session with no code folder offers none, and
/// sharing a code folder makes it readable, so the orchestrator tells the core (D-244). A
/// document adds no roots and changes nothing; letting the folder go turns them off again.
#[tokio::test]
async fn sharing_a_code_folder_with_none_open_offers_the_repo_reads() {
    use boardai_core::RepoReads;
    use boardai_core::input::Input;
    use boardai_core::sources::SourceInput;

    let setup = setup("repo-reads");
    let talker = Arc::new(ScriptedTalker::default());
    let home = setup.home.clone();
    let harness = start_configured(Arc::clone(&talker), false, None, move |parts| {
        parts.sources = SourceParts::in_memory(home);
    });
    let recorder = Arc::clone(&harness.recorder);
    let told = move || -> Vec<RepoReads> {
        recorder
            .inputs()
            .iter()
            .filter_map(|input| {
                if let Input::Source {
                    source: SourceInput::RepoReads { reads },
                } = input
                {
                    Some(*reads)
                } else {
                    None
                }
            })
            .collect()
    };
    let readme = validate(&setup.greeter.join("README.md"), &setup.home).expect("a document");
    assert!(harness.session.share(readme, false).is_ok());
    let folder = validate(&setup.greeter, &setup.home).expect("a valid folder");
    assert!(harness.session.share(folder, false).is_ok());
    let offered = {
        let told = told.clone();
        run_until(5_000, move || !told().is_empty()).await
    };
    assert!(offered, "the core was told");
    assert_eq!(told(), [RepoReads::Offered], "once, for the folder alone");
    let folder_id = harness
        .recorder
        .events()
        .iter()
        .rev()
        .find_map(|event| {
            let UiEvent::Sources { tray } = event else {
                return None;
            };
            tray.session
                .iter()
                .find(|row| row.title == "greeter")
                .map(|row| row.id)
        })
        .expect("the folder's row");
    let remove = boardai_core::protocol::UiCommand::Sources {
        action: boardai_core::sources::tray::SourceAction::Remove { source: folder_id },
    };
    assert!(harness.session.command(remove).is_ok());
    let off = {
        let told = told.clone();
        run_until(5_000, move || told().len() == 2).await
    };
    assert!(off, "{:?}", told());
    assert_eq!(told(), [RepoReads::Offered, RepoReads::NotOffered]);
    let _ = close(harness).await;
    let _ = std::fs::remove_dir_all(&setup.root);
}
