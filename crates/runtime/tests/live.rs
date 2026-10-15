//! Real-provider smoke tests. Never part of `just check`: every test is ignored and also needs
//! `BOARDAI_LIVE=1`; run them with `just live`.
//!
//! - Ollama: a running server with `BOARDAI_LIVE_OLLAMA_MODEL` (default `qwen3.5:2b`).
//! - Anthropic: `ANTHROPIC_API_KEY` in the environment.
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
    ClaudeCodeConfig, ClaudeCodeHarness, CodingHarness, PiConfig, PiHarness,
    discover_from_environment,
};
use boardai_runtime::llm::anthropic::{AnthropicOptions, AnthropicProvider};
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OpenAiCompatibleProvider, OpenAiOptions};
use boardai_runtime::llm::{
    ChatEvent, ChatStream, Effort, LlmProvider, LlmRole, TalkerConfig, ToolSpec,
};
use boardai_runtime::settings::Secret;
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
    let harness = ClaudeCodeHarness::new(ClaudeCodeConfig {
        program,
        repo: repo.path.clone(),
        environment,
    });
    let events = run_task("claude code", &harness).await;
    assert!(
        matches!(events.last(), Some(HarnessEvent::Finished { .. })),
        "{events:?}"
    );
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
    let harness = PiHarness::new(PiConfig {
        program: pi,
        repo: repo.path.clone(),
        environment: Vec::new(),
    });
    let events = run_task("pi", &harness).await;
    assert!(
        matches!(events.last(), Some(HarnessEvent::Finished { .. })),
        "{events:?}"
    );
}
