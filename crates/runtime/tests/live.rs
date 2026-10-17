//! Real-provider smoke tests. Never part of `just check`: every test is ignored and also needs
//! `BOARDAI_LIVE=1`; run them with `just live`.
//!
//! - Ollama: a running server with `BOARDAI_LIVE_OLLAMA_MODEL` (default `qwen3.5:2b`).
//! - Anthropic: `ANTHROPIC_API_KEY` in the environment.
//! - OpenRouter: `OPEN_ROUTER_KEY` (or `OPENROUTER_API_KEY`); the presets on their route, and
//!   the first token cold, on a warm connection, and after the session warm-up (D-154).
//! - Claude Code and pi: `BOARDAI_LIVE_CLAUDE=1` / `BOARDAI_LIVE_PI=1`, since they use your
//!   account. `BOARDAI_LIVE_CAPTURE=<file>` saves Claude Code's raw stream-json.
#![allow(clippy::expect_used, reason = "smoke tests fail loudly")]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use boardai_core::harness::{HarnessEvent, HarnessKind, HarnessTask};
use boardai_core::history::{Conversation, Message, UserPart};
use boardai_core::tools::ToolCall;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::MonotonicClock;
use boardai_runtime::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, CodingHarness, DEFAULT_RESEARCH_MODEL, DefaultFolder,
    PiConfig, PiHarness, discover_from_environment,
};
use boardai_runtime::llm::anthropic::{AnthropicOptions, AnthropicProvider};
use boardai_runtime::llm::openai::{
    Dialect, OLLAMA_BASE_URL, OPENROUTER_BASE_URL, OpenAiCompatibleProvider, OpenAiOptions,
};
use boardai_runtime::llm::openrouter::{PRESETS, Preset};
use boardai_runtime::llm::{
    ChatEvent, ChatStream, Effort, LlmProvider, LlmRole, TalkerConfig, ToolSpec,
};
use boardai_runtime::session::talker_from_settings;
use boardai_runtime::settings::{LlmKind, LlmSettings, MemorySecretStore, Secret, SecretName};
use futures_util::StreamExt;
use serde_json::json;
use support::TempFolder;

fn enabled(flag: &str) -> bool {
    let on = std::env::var("BOARDAI_LIVE").is_ok_and(|value| value == "1")
        && (flag == "BOARDAI_LIVE" || std::env::var(flag).is_ok_and(|value| value == "1"));
    if !on {
        println!("skipped: set BOARDAI_LIVE=1 and {flag}=1");
    }
    on
}

fn ask(text: &str) -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text { text: text.into() }],
    });
    conversation
}

fn read_file_tool() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: "Read a file from the open repository. Call it whenever the user asks \
                      about a file's contents."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Path in the repo"}},
            "required": ["path"],
        }),
    }
}

/// Streams to the end, printing time to first event and total time.
async fn measure(label: &str, stream: ChatStream) -> Vec<ChatEvent> {
    let started = Instant::now();
    let mut first: Option<Duration> = None;
    let mut events = Vec::new();
    let mut stream = stream;
    while let Some(item) = stream.next().await {
        first.get_or_insert_with(|| started.elapsed());
        events.push(item.unwrap_or_else(|error| panic!("{label}: {error}")));
    }
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ChatEvent::TextDelta(text) => Some(text.as_str()),
            ChatEvent::ToolCall(_) | ChatEvent::Done { .. } => None,
        })
        .collect();
    println!(
        "{label}: first event {:?}, total {:?}, text {text:?}, last {:?}",
        first.unwrap_or_default(),
        started.elapsed(),
        events.last()
    );
    events
}

/// A voice talker on a local thinking model runs with thinking off (`Effort::None`): at `low`,
/// `qwen3.5:2b` spends a 400-token budget reasoning and never answers.
fn ollama(tools: Vec<ToolSpec>) -> OpenAiCompatibleProvider {
    let model = std::env::var("BOARDAI_LIVE_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3.5:2b".into());
    let config = TalkerConfig {
        role: LlmRole::Talker,
        model,
        effort: Effort::None,
        max_tokens: 400,
        system_prompt: "You are a concise tutor. Answer in one short sentence.".into(),
        tools,
    };
    OpenAiCompatibleProvider::new(
        &config,
        OpenAiOptions {
            base_url: OLLAMA_BASE_URL.into(),
            api_key: None,
            clock: Arc::new(MonotonicClock::start()),
            policy: None,
            dialect: Dialect::Standard,
        },
    )
    .expect("provider builds")
}

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn ollama_streams_a_short_answer() {
    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let provider = ollama(Vec::new());
    let stream = provider.stream(&ask("What is a load balancer?"), CancelToken::new());
    let events = measure("ollama text", stream).await;
    assert!(matches!(events.last(), Some(ChatEvent::Done { .. })));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ChatEvent::TextDelta(_)))
    );
}

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn ollama_calls_a_tool() {
    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let provider = ollama(vec![read_file_tool()]);
    let stream = provider.stream(
        &ask("Use the read_file tool to read src/main.rs."),
        CancelToken::new(),
    );
    let events = measure("ollama tool", stream).await;
    let call = events.iter().find_map(|event| match event {
        ChatEvent::ToolCall(invocation) => Some(invocation.call.clone()),
        ChatEvent::TextDelta(_) | ChatEvent::Done { .. } => None,
    });
    assert!(
        matches!(call, Some(Ok(ToolCall::ReadFile { ref path, .. })) if path.ends_with("main.rs")),
        "{call:?}"
    );
}

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn anthropic_streams_a_short_answer() {
    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
        println!("skipped: ANTHROPIC_API_KEY is not set");
        return;
    };
    let config = TalkerConfig::talker(
        "You are a concise tutor. Answer in one short sentence.".into(),
        vec![read_file_tool()],
    );
    let provider = AnthropicProvider::new(
        &config,
        AnthropicOptions {
            base_url: None,
            api_key: Secret::new(key),
            clock: Arc::new(MonotonicClock::start()),
            policy: None,
        },
    )
    .expect("provider builds");
    let stream = provider.stream(&ask("What is a load balancer?"), CancelToken::new());
    let events = measure("anthropic text", stream).await;
    assert!(matches!(events.last(), Some(ChatEvent::Done { .. })));
}

/// A throwaway git repo with one file to ask about.
fn scratch_repo() -> TempFolder {
    let folder = TempFolder::new("live-repo");
    folder.write(
        "src/lib.rs",
        b"pub fn greet() -> &'static str {\n    \"hello\"\n}\n",
    );
    for arguments in [
        &["init", "--quiet"][..],
        &["add", "."],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "--quiet",
            "-m",
            "init",
        ],
    ] {
        let status = Command::new("git")
            .arg("-C")
            .arg(&folder.path)
            .args(arguments)
            .status();
        assert!(
            status.is_ok_and(|status| status.success()),
            "git {arguments:?}"
        );
    }
    folder
}

async fn run_task(label: &str, harness: &dyn CodingHarness) -> Vec<HarnessEvent> {
    let task = HarnessTask {
        kind: HarnessKind::Research,
        brief: "What does greet() in src/lib.rs return? Answer in one sentence.".into(),
    };
    let started = Instant::now();
    let events: Vec<HarnessEvent> = tokio::time::timeout(
        Duration::from_secs(180),
        harness.run(task, CancelToken::new()).collect(),
    )
    .await
    .expect("the task ends");
    println!("{label}: {:?} for {events:#?}", started.elapsed());
    events
}

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn claude_code_answers_a_read_only_question() {
    if !enabled("BOARDAI_LIVE_CLAUDE") {
        return;
    }
    let claude = discover_from_environment()
        .claude_code
        .expect("claude on PATH");
    let repo = scratch_repo();
    let mut program = claude.clone();
    let mut environment = Vec::new();
    if let Ok(capture) = std::env::var("BOARDAI_LIVE_CAPTURE") {
        // A wrapper that tees the raw stream-json, so the run doubles as a fixture recording.
        let wrapper = repo.write(
            "../claude-tee",
            b"#!/bin/sh\n\"$REAL_CLAUDE\" \"$@\" | tee \"$CAPTURE\"\n",
        );
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        environment.push(("REAL_CLAUDE".into(), claude.into_os_string()));
        environment.push(("CAPTURE".into(), capture.into()));
        program = wrapper;
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME");
    let harness = ClaudeCodeHarness::new(ClaudeCodeConfig {
        program,
        // Spoken research is held to the folder's repository like a brief (D-243).
        repo: Some(DefaultFolder::open(repo.path.clone(), &home)),
        environment,
        research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
        model: None,
        sign_in: boardai_runtime::harness::ClaudeSignIn::read(std::path::Path::new(
            &std::env::var_os("HOME").unwrap_or_default(),
        )),
    });
    let events = run_task("claude code", &harness).await;
    assert!(
        matches!(events.last(), Some(HarnessEvent::Finished { .. })),
        "{events:?}"
    );
}

/// A brief request (D-185) through the real Claude Code, on a module of this repo: the answer
/// parses as a brief, its citations are read natively (those beyond the folder inside its
/// reach too, as when this repo is a code folder), and the run's time and shape are printed.
/// It runs as the app runs it: on the research model, held to the reach (D-226).
/// `BOARDAI_LIVE_BRIEF_FOLDER` picks the folder (default `crates/runtime/src/harness`),
/// `BOARDAI_LIVE_BRIEF_TOPIC` a component (default the overview); `BOARDAI_LIVE_CAPTURE` keeps
/// the raw stream-json (its `result` line has the run's cost).
#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn claude_code_writes_a_brief_on_a_module_of_this_repo() {
    if !enabled("BOARDAI_LIVE_CLAUDE") {
        return;
    }
    let live = live_brief().await;
    let brief = boardai_core::sources::brief::parse(&live.summary);
    assert!(brief.is_usable(), "{}", live.summary);
}

/// A live brief's diagram on the board (D-242): the brief Claude Code wrote, through the core
/// as a session would carry it (shared, prefetched, used by a scripted talker's research call,
/// staged, two parts revealed by the talker, the rest as its reply ends). Prints the parts
/// staged and revealed and the times.
#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn claude_code_briefs_diagram_is_staged_and_revealed() {
    if !enabled("BOARDAI_LIVE_CLAUDE") {
        return;
    }
    let live = live_brief().await;
    let staged = stage_live_brief(&live.title, &live.summary);
    println!("{staged}");
}

/// A live brief and how it ran.
struct LiveBrief {
    title: String,
    summary: String,
}

/// Runs a brief as the app does, prints its shape, and saves it under `target/live-briefs`.
async fn live_brief() -> LiveBrief {
    use boardai_core::sources::brief::request_prompt;
    use boardai_core::sources::{BriefTopic, SourceKind};
    use boardai_runtime::harness::{Answer, Workspace};

    let claude = discover_from_environment()
        .claude_code
        .expect("claude on PATH");
    let folder = std::env::var("BOARDAI_LIVE_BRIEF_FOLDER").map_or_else(
        |_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/harness"),
        std::path::PathBuf::from,
    );
    let folder = std::fs::canonicalize(&folder).expect("the folder");
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME");
    let reach =
        boardai_runtime::sources::reach(&folder, SourceKind::CodeFolder, &home).expect("a reach");
    let topic = std::env::var("BOARDAI_LIVE_BRIEF_TOPIC")
        .map_or(BriefTopic::Overview, |name| BriefTopic::component(&name));
    let title = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
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
    let task = HarnessTask {
        kind: HarnessKind::Research,
        brief: request_prompt(&title, SourceKind::CodeFolder, &topic, None),
    };
    let workspace = Workspace {
        folder: folder.clone(),
        reach: reach.clone(),
        answer: Answer::Brief,
    };
    let started = Instant::now();
    let events: Vec<HarnessEvent> = tokio::time::timeout(
        Duration::from_secs(600),
        harness
            .run_in(task, workspace, CancelToken::new())
            .collect(),
    )
    .await
    .expect("the task ends");
    let elapsed = started.elapsed();
    let touched = events
        .iter()
        .filter(|event| matches!(event, HarnessEvent::FileTouched { .. }))
        .count();
    let Some(HarnessEvent::Finished { summary, .. }) = events.last() else {
        panic!("no brief: {events:?}");
    };
    println!("reach: {}", reach.display());
    print_brief(&title, &topic, (&folder, &reach), elapsed, touched, summary);
    LiveBrief {
        title,
        summary: summary.clone(),
    }
}

/// The brief through the core's simulator: what was staged, what the talker revealed, what
/// was revealed as its reply ended, and when.
fn stage_live_brief(title: &str, summary: &str) -> String {
    use boardai_core::ids::LayerId;
    use boardai_core::sim::talker::ScriptedReply;

    let brief = boardai_core::sources::brief::parse(summary);
    let ids: Vec<String> = brief
        .diagram
        .as_ref()
        .map(|diagram| {
            diagram
                .ids
                .iter()
                .take(2)
                .map(|id| id.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut sim = shared_with(title, summary);
    sim.talker
        .push(ScriptedReply::says("Let me check the brief.").calls(
            "research",
            r#"{"question":"How does it work?","depth":"quick"}"#,
        ));
    let mut narration = String::from("Here is the map. ");
    for id in &ids {
        narration.push_str(&format!(
            "<reveal id=\"{id}\" deep=\"true\"/>This is {id}. "
        ));
    }
    narration.push_str("And that is the rest of it.");
    sim.talker.push(ScriptedReply::says(&narration));
    sim.user_says(7_000, &format!("How does {title} work?"));
    sim.run_until(120_000);
    let mut lines = staging_lines(&sim);
    let staged = sim.core().board().staged(LayerId::MAIN);
    let entries: usize = staged.iter().map(|diagram| diagram.entries.len()).sum();
    let shown = sim.core().board().layer(LayerId::MAIN).map_or(0, |layer| {
        layer
            .elements()
            .iter()
            .filter(|element| element.state.is_shown())
            .count()
    });
    lines.push(format!(
        "staged {} diagram(s), {entries} parts and connectors; {shown} elements shown; talker \
         revealed {ids:?}; violations {:?}",
        staged.len(),
        sim.violations()
    ));
    lines.join("\n")
}

/// A simulated session with Claude Code, `title` shared, and its overview brief `summary`
/// prefetched.
fn shared_with(title: &str, summary: &str) -> boardai_core::sim::Sim {
    use boardai_core::input::Input;
    use boardai_core::sim::Sim;
    use boardai_core::sim::sources::code_folder;
    use boardai_core::sources::SourceInput;

    let mut sim = Sim::new(boardai_core::CoreConfig::default());
    sim.sources.default_brief = summary.to_owned();
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.share(100, &code_folder(1, title, &format!("~/{title}")), false);
    sim.run_until(6_000);
    sim
}

/// The staged diagram's headers and each reveal, with their times.
fn staging_lines(sim: &boardai_core::sim::Sim) -> Vec<String> {
    use boardai_core::protocol::{DrawOp, UiEvent};

    let mut lines = Vec::new();
    for (at, event) in sim.ui() {
        if let UiEvent::Diagram {
            layer,
            title,
            part,
            parts,
            provenance,
        } = event
        {
            lines.push(format!(
                "{} ms L{}: diagram \"{title}\" part {part} of {parts} {provenance:?}",
                at.as_ms(),
                layer.0
            ));
        } else if let UiEvent::OpStart {
            op: DrawOp::Reveal { parts, .. },
            budget_ms,
            ..
        } = event
        {
            let ids: Vec<&str> = parts.iter().map(|part| part.id().as_str()).collect();
            lines.push(format!(
                "{} ms: reveal of {} parts over {budget_ms} ms: {ids:?}",
                at.as_ms(),
                parts.len()
            ));
        }
    }
    lines
}

/// What a live brief came back as: its shape, its citations read natively, and the file it was
/// saved to (`target/live-briefs/<topic>.md`).
fn print_brief(
    title: &str,
    topic: &boardai_core::sources::BriefTopic,
    (folder, reach): (&std::path::Path, &std::path::Path),
    elapsed: Duration,
    touched: usize,
    summary: &str,
) {
    use boardai_runtime::sources::check_citations;

    let brief = boardai_core::sources::brief::parse(summary);
    let citations = brief.all_citations();
    let inside = check_citations(folder, None, None, &citations).read.len();
    let checked = check_citations(folder, None, Some(reach), &citations);
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/live-briefs");
    let _ = std::fs::create_dir_all(&target);
    let _ = std::fs::write(target.join(format!("{}.md", topic.key())), summary);
    println!(
        "brief on {} of {title}: {elapsed:?}, {touched} files read, {} bytes; {} components, {} \
         relations, {} flows, diagram ids {:?}, {} details; citations {} read ({inside} in the \
         folder), {} unverified; dropped {:?}",
        topic.spoken(),
        summary.len(),
        brief.components.len(),
        brief.relations.len(),
        brief.flows.len(),
        brief.diagram.as_ref().map(|diagram| diagram.ids.len()),
        brief.details.len(),
        checked.read.len(),
        checked.unverified.len(),
        brief.dropped
    );
}

/// The program to run: `claude` itself, or a wrapper that tees its stream-json to
/// `BOARDAI_LIVE_CAPTURE`.
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

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn pi_answers_a_read_only_question() {
    if !enabled("BOARDAI_LIVE_PI") {
        return;
    }
    let Some(pi) = discover_from_environment().pi else {
        println!("skipped: pi is not on PATH");
        return;
    };
    let repo = scratch_repo();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME");
    let harness = PiHarness::new(PiConfig {
        program: pi,
        repo: Some(DefaultFolder::open(repo.path.clone(), &home)),
        environment: Vec::new(),
    });
    let events = run_task("pi", &harness).await;
    assert!(
        matches!(events.last(), Some(HarnessEvent::Finished { .. })),
        "{events:?}"
    );
}

fn open_router_key() -> Option<Secret> {
    ["OPEN_ROUTER_KEY", "OPENROUTER_API_KEY"]
        .iter()
        .find_map(|variable| std::env::var(variable).ok().filter(|key| !key.is_empty()))
        .map(|key| Secret::new(key.trim().to_owned()))
}

/// The talker the app builds for a preset: BoardAI's real system prompt and tools.
fn open_router_talker(preset: Preset, key: &Secret) -> Arc<dyn LlmProvider> {
    let name = SecretName::parse("openrouter").expect("a name");
    let mut secrets = MemorySecretStore::new();
    secrets.insert(name.clone(), key.clone());
    let settings = LlmSettings {
        kind: LlmKind::OpenAiCompatible,
        model: preset.model.to_owned(),
        effort: preset.talker.effort,
        // OpenRouter holds back credit for every request's whole output budget while it runs;
        // these answers are only timed, so a short budget keeps a low balance from refusing.
        max_tokens: 600,
        base_url: Some(OPENROUTER_BASE_URL.to_owned()),
        secret: Some(name),
        route: preset.route(),
        early_start: true,
    };
    talker_from_settings(&settings, &secrets, Arc::new(MonotonicClock::start()))
        .expect("talker builds")
}

/// Time to the first text or tool call. The answer is read to its end: a dropped stream may
/// keep running (and holding credit) at a provider that can't cancel.
async fn first_token_ms(talker: &dyn LlmProvider, question: &str) -> u128 {
    let started = Instant::now();
    let mut first = None;
    let mut stream = talker.stream(&ask(question), CancelToken::new());
    while let Some(item) = stream.next().await {
        match item.unwrap_or_else(|error| panic!("{question}: {error}")) {
            ChatEvent::TextDelta(_) | ChatEvent::ToolCall(_) => {
                first.get_or_insert_with(|| started.elapsed().as_millis());
            }
            ChatEvent::Done { .. } => break,
        }
    }
    first.unwrap_or_else(|| panic!("{question}: no first token"))
}

#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn openrouter_presets_answer_and_call_tools() {
    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let Some(key) = open_router_key() else {
        println!("skipped: OPEN_ROUTER_KEY is not set");
        return;
    };
    for preset in PRESETS {
        let config = TalkerConfig {
            role: LlmRole::Talker,
            model: preset.model.to_owned(),
            effort: preset.talker.effort,
            max_tokens: 400,
            system_prompt: "You are a concise tutor. Answer in one short sentence.".into(),
            tools: vec![read_file_tool()],
        };
        let provider = OpenAiCompatibleProvider::new(
            &config,
            OpenAiOptions {
                base_url: OPENROUTER_BASE_URL.into(),
                api_key: Some(key.clone()),
                clock: Arc::new(MonotonicClock::start()),
                policy: None,
                dialect: Dialect::OpenRouter {
                    route: preset.route(),
                },
            },
        )
        .expect("provider builds");
        let label = format!("{} text", preset.model);
        let stream = provider.stream(&ask("What is a load balancer?"), CancelToken::new());
        let events = measure(&label, stream).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ChatEvent::TextDelta(_)))
        );
        let label = format!("{} tool", preset.model);
        let ask_tool = ask("Use the read_file tool to read src/main.rs.");
        let events = measure(&label, provider.stream(&ask_tool, CancelToken::new())).await;
        let call = events.iter().find_map(|event| match event {
            ChatEvent::ToolCall(invocation) => Some(invocation.call.clone()),
            ChatEvent::TextDelta(_) | ChatEvent::Done { .. } => None,
        });
        assert!(
            matches!(call, Some(Ok(ToolCall::ReadFile { ref path, .. })) if path.ends_with("main.rs")),
            "{call:?}"
        );
    }
}

/// What the session warm-up buys on OpenRouter (D-154): the first token of BoardAI's real
/// prompt from a fresh process-like talker (cold connection), from the same talker again (warm
/// connection, provider cache), and from a fresh talker after its warm-up.
#[tokio::test]
#[ignore = "live: run with `just live`"]
async fn openrouter_first_token_cold_warm_and_after_the_warm_up() {
    if !enabled("BOARDAI_LIVE") {
        return;
    }
    let Some(key) = open_router_key() else {
        println!("skipped: OPEN_ROUTER_KEY is not set");
        return;
    };
    // Shows who served each answer (`served_by`), to tell a slow Cerebras from a fallback.
    let _ = boardai_runtime::logging::init_stderr(tracing::Level::DEBUG);
    let rounds = std::env::var("BOARDAI_LIVE_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(3_usize);
    let question = "How does a request get from the browser to our database?";
    for preset in PRESETS {
        for round in 1..=rounds {
            let cold = open_router_talker(preset, &key);
            let cold_ms = first_token_ms(cold.as_ref(), question).await;
            let warm_ms = first_token_ms(cold.as_ref(), question).await;
            let warmed = open_router_talker(preset, &key);
            let started = Instant::now();
            let warm_up = warmed.warm_up().expect("OpenRouter warms up").await;
            let warm_up_ms = started.elapsed().as_millis();
            let after_ms = first_token_ms(warmed.as_ref(), question).await;
            println!(
                "{} round {round}: cold {cold_ms} ms, warm connection {warm_ms} ms, warm-up \
                 {warm_up_ms} ms ({warm_up:?}) then {after_ms} ms",
                preset.model
            );
        }
    }
}
