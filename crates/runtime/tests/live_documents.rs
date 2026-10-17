//! Real-provider readings of a document (D-186, D-255), never part of `just check`: every test
//! is ignored and also needs `BOARDAI_LIVE=1`; `just live` runs them with the other smoke tests.
//!
//! - OpenRouter: `OPEN_ROUTER_KEY` (or `OPENROUTER_API_KEY`); Gemini 3.8 Flash unless
//!   `BOARDAI_LIVE_DOCUMENT_MODEL` names another model that reads files.
//! - Claude Code: `BOARDAI_LIVE_CLAUDE=1`, since it uses your account. `BOARDAI_LIVE_CAPTURE=<file>`
//!   keeps its raw stream-json, whose `result` line has the run's cost.
//! - `BOARDAI_LIVE_DOCUMENT` picks the document (default the generated paper fixture).
#![allow(clippy::expect_used, reason = "smoke tests fail loudly")]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use boardai_runtime::cancel::CancelToken;
use boardai_runtime::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, DEFAULT_RESEARCH_MODEL, discover_from_environment,
};
use boardai_runtime::llm::Effort;
use boardai_runtime::llm::openai::OPENROUTER_BASE_URL;
use boardai_runtime::llm::openrouter::PRESETS;
use boardai_runtime::settings::Secret;
use futures_util::StreamExt;
use support::TempFolder;

fn enabled(flag: &str) -> bool {
    let on = std::env::var("BOARDAI_LIVE").is_ok_and(|value| value == "1")
        && (flag == "BOARDAI_LIVE" || std::env::var(flag).is_ok_and(|value| value == "1"));
    if !on {
        println!("skipped: set BOARDAI_LIVE=1 and {flag}=1");
    }
    on
}

fn open_router_key() -> Option<Secret> {
    ["OPEN_ROUTER_KEY", "OPENROUTER_API_KEY"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|key| !key.trim().is_empty())
        })
        .map(|key| Secret::new(key.trim().to_owned()))
}

/// `claude` itself, or a wrapper that tees its stream-json to `BOARDAI_LIVE_CAPTURE`.
fn captured(
    claude: std::path::PathBuf,
) -> (
    std::path::PathBuf,
    Vec<(std::ffi::OsString, std::ffi::OsString)>,
) {
    let Ok(capture) = std::env::var("BOARDAI_LIVE_CAPTURE") else {
        return (claude, Vec::new());
    };
    let folder = TempFolder::new("claude-tee");
    let wrapper = folder.write(
        "claude-tee",
        b"#!/bin/sh\n\"$REAL_CLAUDE\" \"$@\" | tee \"$CAPTURE\"\n",
    );
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // The wrapper's folder must outlive the run: it is left for the OS to clean.
    std::mem::forget(folder);
    let environment = vec![
        ("REAL_CLAUDE".into(), claude.into_os_string()),
        ("CAPTURE".into(), capture.into()),
    ];
    (wrapper, environment)
}

/// The paper a document's live reading uses: `BOARDAI_LIVE_DOCUMENT`, else the generated
/// fixture (public domain: written for BoardAI), copied into a folder of its own so a harness's
/// reach is that folder alone (D-226).
fn live_document() -> (TempFolder, std::path::PathBuf) {
    let source = std::env::var("BOARDAI_LIVE_DOCUMENT").map_or_else(
        |_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/documents/paper.pdf")
        },
        std::path::PathBuf::from,
    );
    let folder = TempFolder::new("live-document");
    let name = source
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    let copy = folder.write(&name, &std::fs::read(&source).expect("the document"));
    let copy = std::fs::canonicalize(copy).expect("canonical");
    (folder, copy)
}

/// What a live reading came back as: its time, its events, what it captured, and the digest
/// saved to `target/documents-live/<label>.md`.
fn print_reading(label: &str, elapsed: Duration, events: &[boardai_runtime::sources::ReaderEvent]) {
    use boardai_runtime::sources::ReaderEvent;

    let (state, digest) = match events.last() {
        Some(ReaderEvent::Ready { detail, digest }) => (format!("ready: {detail}"), digest.clone()),
        Some(ReaderEvent::Partial { reason, digest }) => {
            (format!("partial: {reason}"), digest.clone())
        }
        other => panic!("{label}: not settled: {other:?}"),
    };
    let parsed = boardai_core::sources::digest::parse(&digest);
    let target =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/documents-live");
    let _ = std::fs::create_dir_all(&target);
    let _ = std::fs::write(target.join(format!("{label}.md")), &digest);
    let diagrams = parsed
        .figures
        .iter()
        .filter(|figure| figure.diagram.is_some())
        .count();
    println!(
        "{label}: {elapsed:?}, {} reports; {state}; {} sections, {} of {} figures read ({diagrams} \
         with a diagram), {} tables, {} claims; dropped {:?}",
        events.len(),
        parsed.sections.len(),
        parsed.figures_read(),
        parsed.figures_total(),
        parsed.tables.len(),
        parsed.claims.len(),
        parsed.dropped
    );
}

/// D-186: a real paper read by a model that reads files through OpenRouter (Gemini 3.8 Flash
/// unless `BOARDAI_LIVE_DOCUMENT_MODEL` names another), as the app's reader reads it. The call's
/// tokens and cost are logged ("a digest call answered").
#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn openrouter_reads_a_paper_and_its_figures() {
    use boardai_runtime::documents::files::{FileModel, display_name};
    use boardai_runtime::documents::pdfkit::PdfKit;
    use boardai_runtime::documents::{DocumentReader, ReaderConfig};
    use boardai_runtime::sources::SourceReader;

    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let Some(key) = open_router_key() else {
        println!("skipped: OPEN_ROUTER_KEY is not set");
        return;
    };
    let _ = boardai_runtime::logging::init_stderr(tracing::Level::INFO);
    let model = std::env::var("BOARDAI_LIVE_DOCUMENT_MODEL")
        .unwrap_or_else(|_| "google/gemini-3.8-flash".to_owned());
    let route = PRESETS
        .iter()
        .find(|preset| preset.model == model)
        .map(|preset| preset.route.iter().map(|slug| (*slug).to_owned()).collect())
        .unwrap_or_default();
    let effort = if model.contains("gemini") {
        Effort::Low
    } else {
        Effort::None
    };
    let reader = DocumentReader::new(ReaderConfig {
        file_models: vec![FileModel {
            name: display_name(&model),
            base_url: OPENROUTER_BASE_URL.to_owned(),
            key,
            model: model.clone(),
            route,
            effort,
        }],
        harness: None,
        home: std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("HOME"),
        pdfkit: PdfKit::default(),
    });
    let (_folder, path) = live_document();
    let started = Instant::now();
    let events: Vec<_> = reader.read(path, CancelToken::new()).collect().await;
    print_reading(
        &format!("openrouter-{}", display_name(&model)),
        started.elapsed(),
        &events,
    );
}

/// D-186: the same paper read by Claude Code, as the app's reader asks it (the research model,
/// held to the document's folder). `BOARDAI_LIVE_CAPTURE` keeps the raw stream-json, whose
/// `result` line has the run's cost.
#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn claude_code_reads_a_paper_and_its_figures() {
    use boardai_runtime::documents::pdfkit::PdfKit;
    use boardai_runtime::documents::{DocumentReader, ReaderConfig};
    use boardai_runtime::harness::{HarnessPool, HarnessTimeouts};
    use boardai_runtime::sources::SourceReader;

    if !enabled("BOARDAI_LIVE_CLAUDE") {
        return;
    }
    let claude = discover_from_environment()
        .claude_code
        .expect("claude on PATH");
    let (program, environment) = captured(claude);
    let harness = ClaudeCodeHarness::new(ClaudeCodeConfig {
        program,
        repo: None,
        environment,
        research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
        model: None,
        sign_in: boardai_runtime::harness::ClaudeSignIn::read(std::path::Path::new(
            &std::env::var_os("HOME").unwrap_or_default(),
        )),
    });
    let pool = HarnessPool::new(Arc::new(harness), HarnessTimeouts::default());
    let reader = DocumentReader::new(ReaderConfig {
        file_models: Vec::new(),
        harness: Some((pool, "Claude Code".to_owned())),
        home: std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("HOME"),
        pdfkit: PdfKit::default(),
    });
    let (_folder, path) = live_document();
    let started = Instant::now();
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(900),
        reader.read(path, CancelToken::new()).collect(),
    )
    .await
    .expect("the reading ends");
    print_reading("claude-code", started.elapsed(), &events);
}
