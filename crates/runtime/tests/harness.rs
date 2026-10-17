//! Harness adapters against stream-json / RPC fixtures, driven through a fake harness script so
//! the real subprocess paths (stdin, stdout, signals, kill) run.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::harness::{
    Answer, ClaudeCodeConfig, ClaudeCodeHarness, CodingHarness, DEFAULT_RESEARCH_MODEL,
    HarnessPool, HarnessStream, HarnessTimeouts, PiConfig, PiHarness, Workspace, discover,
};
use futures_util::StreamExt;
use support::{TempFolder, fixture, fixture_path};

struct Fake {
    folder: TempFolder,
    program: PathBuf,
    repo: PathBuf,
}

impl Fake {
    fn new() -> Self {
        let folder = TempFolder::new("harness");
        let program = folder.write("bin/fake-harness", &fixture("harness/fake-harness.sh"));
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        folder.write(
            "repo/src/lib.rs",
            b"pub fn greet() -> &'static str {\n    \"hello\"\n}\n",
        );
        let repo = folder.path.join("repo");
        Self {
            folder,
            program,
            repo,
        }
    }

    fn environment(
        &self,
        fixture_name: &str,
        mode: &str,
    ) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        vec![
            ("FAKE_FOLDER".into(), self.folder.path.clone().into()),
            ("FAKE_FIXTURE".into(), fixture_path(fixture_name).into()),
            ("FAKE_MODE".into(), mode.into()),
        ]
    }

    fn claude(&self, fixture_name: &str, mode: &str) -> ClaudeCodeHarness {
        ClaudeCodeHarness::new(ClaudeCodeConfig {
            program: self.program.clone(),
            repo: Some(self.repo.clone()),
            environment: self.environment(fixture_name, mode),
            research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
        })
    }

    fn pi(&self, fixture_name: &str, mode: &str) -> PiHarness {
        PiHarness::new(PiConfig {
            program: self.program.clone(),
            repo: Some(self.repo.clone()),
            environment: self.environment(fixture_name, mode),
        })
    }

    fn recorded(&self, name: &str) -> String {
        std::fs::read_to_string(self.folder.path.join(name)).unwrap_or_default()
    }
}

fn research(brief: &str) -> HarnessTask {
    HarnessTask {
        kind: HarnessKind::Research,
        brief: brief.into(),
    }
}

async fn collect(stream: HarnessStream) -> Vec<HarnessEvent> {
    tokio::time::timeout(Duration::from_secs(15), stream.collect())
        .await
        .expect("the task ends")
}

fn last(events: &[HarnessEvent]) -> &HarnessEvent {
    events.last().expect("a terminal event")
}

fn failure(events: &[HarnessEvent]) -> (HarnessFailure, String) {
    match last(events) {
        HarnessEvent::Failed { reason, detail } => (*reason, detail.clone()),
        other @ (HarnessEvent::Progress { .. }
        | HarnessEvent::FileTouched { .. }
        | HarnessEvent::Finished { .. }) => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn claude_research_maps_stream_json_to_events() {
    let fake = Fake::new();
    let task = research("What does greet return?");
    let events = collect(
        fake.claude("claude/research.jsonl", "replay")
            .run(task, CancelToken::new()),
    )
    .await;
    assert_eq!(
        events[..2],
        [
            HarnessEvent::Progress {
                text: "I'll look at the library first.".into()
            },
            HarnessEvent::FileTouched {
                path: "src/lib.rs".into()
            },
        ]
    );
    let HarnessEvent::Finished { summary, citations } = last(&events) else {
        panic!("expected Finished, got {events:?}");
    };
    assert!(summary.starts_with("`greet()` returns \"hello\""));
    // The missing file is not cited, and the repeated citation appears once.
    assert_eq!(citations, &["src/lib.rs:2".to_owned()]);
    let arguments = fake.recorded("arguments");
    for expected in [
        "-p\n--input-format\nstream-json\n--output-format\nstream-json\n--verbose\n",
        "--include-partial-messages\n--session-id\n",
        "--permission-prompts\nnone\n",
        "--permission-mode\nplan\n--allowedTools\nRead,Grep,Glob\n",
    ] {
        assert!(
            arguments.contains(expected),
            "{expected:?} in {arguments:?}"
        );
    }
    let stdin: serde_json::Value =
        serde_json::from_str(&fake.recorded("stdin")).expect("json line");
    assert_eq!(stdin["type"], "user");
    assert_eq!(
        stdin["message"]["content"][0]["text"],
        "What does greet return?"
    );
}

/// A brief request (D-185) runs in the source's own folder with the brief's system prompt, and
/// its answer is the brief, whole, for the core to parse. It runs on the research model, held
/// to its reach by Claude Code's permission rules (D-226).
#[tokio::test]
async fn a_brief_runs_in_the_sources_folder_and_answers_the_shape() {
    let fake = Fake::new();
    let mono = fake.folder.path.join("mono");
    let source = mono.join("crates/greeter");
    std::fs::create_dir_all(source.join("src")).expect("a source folder");
    std::fs::create_dir_all(mono.join(".git")).expect("a repository");
    std::fs::write(source.join("src/lib.rs"), "pub fn greet() {}\n").expect("a file");
    let reach = std::fs::canonicalize(&mono).expect("canonical");
    let workspace = Workspace {
        folder: source.clone(),
        answer: Answer::Brief {
            reach: reach.clone(),
        },
    };
    let pool = HarnessPool::new(
        Arc::new(fake.claude("claude/brief.jsonl", "replay")),
        HarnessTimeouts::default(),
    );
    let events =
        collect(pool.run_in(research("Read the code"), workspace, CancelToken::new())).await;
    assert!(
        events.contains(&HarnessEvent::FileTouched {
            path: "src/lib.rs".into()
        }),
        "paths are relative to the source: {events:?}"
    );
    let HarnessEvent::Finished { summary, .. } = last(&events) else {
        panic!("expected Finished, got {events:?}");
    };
    assert!(
        summary.starts_with("# Brief: greeter overview\n## Overview"),
        "{summary}"
    );
    let arguments = fake.recorded("arguments");
    assert!(
        arguments.contains("Answer only in the Markdown shape the task gives"),
        "{arguments}"
    );
    assert!(!arguments.contains("short, plain answer"));
    for expected in [
        "\n--model\nsonnet\n",
        "\n--permission-mode\ndontAsk\n--tools\nRead,Grep,Glob\n--strict-mcp-config\n",
        "\n--setting-sources\nuser\n--settings\n",
    ] {
        assert!(arguments.contains(expected), "{expected:?} in {arguments}");
    }
    let rules = arguments
        .lines()
        .skip_while(|line| *line != "--settings")
        .nth(1)
        .expect("the rules");
    let rules: serde_json::Value = serde_json::from_str(rules).expect("json");
    assert_eq!(
        rules["permissions"]["allow"],
        serde_json::json!([format!("Read(/{}/**)", reach.display())])
    );
    assert_eq!(
        rules["permissions"]["blockReadsOutsideWorkingDirectories"],
        true
    );
}

/// Without a code folder, a task that names no workspace fails at once, out loud.
#[tokio::test]
async fn a_harness_without_a_folder_refuses_a_task_with_none() {
    let fake = Fake::new();
    let harness = ClaudeCodeHarness::new(ClaudeCodeConfig {
        program: fake.program.clone(),
        repo: None,
        environment: fake.environment("claude/research.jsonl", "replay"),
        research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
    });
    let events = collect(harness.run(research("q"), CancelToken::new())).await;
    assert_eq!(failure(&events).0, HarnessFailure::NotInstalled);
    assert!(fake.recorded("arguments").is_empty(), "nothing ran");
}

/// A real Claude Code 2.1.301 run (Opus 5.5, plan mode), recorded by `just live` and
/// sanitized: the same events the live run produced.
#[tokio::test]
async fn claude_live_recording_replays_to_the_same_events() {
    let fake = Fake::new();
    let events = collect(
        fake.claude("claude/live_research.jsonl", "replay")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    assert_eq!(
        events,
        [
            HarnessEvent::FileTouched {
                path: "src/lib.rs".into()
            },
            HarnessEvent::Finished {
                summary: "The `greet()` function returns a fixed piece of text, the string \"hello\" (src/lib.rs:1, src/lib.rs:2).".into(),
                citations: vec!["src/lib.rs:1".into(), "src/lib.rs:2".into()],
            },
        ]
    );
}

#[tokio::test]
async fn claude_errors_keep_their_kind() {
    let fake = Fake::new();
    let events = collect(
        fake.claude("claude/error_result.jsonl", "replay")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (reason, detail) = failure(&events);
    assert_eq!(reason, HarnessFailure::Crashed);
    assert!(detail.contains("error_max_turns"));

    let events = collect(
        fake.claude("claude/init_only.jsonl", "replay-exit")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (reason, detail) = failure(&events);
    assert_eq!(reason, HarnessFailure::BadOutput);
    assert!(detail.contains("done early"), "{detail}");

    let events = collect(
        fake.claude("claude/research.jsonl", "crash")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (reason, detail) = failure(&events);
    assert_eq!(reason, HarnessFailure::Crashed);
    assert!(detail.contains("boom: something broke"), "{detail}");
}

/// Regression: the exit used to win the race against the last stderr bytes, leaving the detail
/// empty under load.
#[tokio::test]
async fn a_crash_keeps_stderr_written_after_exit() {
    let fake = Fake::new();
    let events = collect(
        fake.claude("claude/research.jsonl", "crash-late")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (reason, detail) = failure(&events);
    assert_eq!(reason, HarnessFailure::Crashed);
    assert!(detail.contains("boom: said late"), "{detail}");
}

#[tokio::test]
async fn a_missing_program_is_not_installed() {
    let harness = ClaudeCodeHarness::new(ClaudeCodeConfig {
        program: "/nonexistent/claude".into(),
        repo: Some(std::env::temp_dir()),
        environment: Vec::new(),
        research_model: DEFAULT_RESEARCH_MODEL.to_owned(),
    });
    let events = collect(harness.run(research("q"), CancelToken::new())).await;
    assert_eq!(failure(&events).0, HarnessFailure::NotInstalled);
}

#[tokio::test]
async fn cancel_interrupts_the_process_group() {
    let fake = Fake::new();
    let cancel = CancelToken::new();
    let mut stream = fake
        .claude("claude/in_progress.jsonl", "hang")
        .run(research("q"), cancel.clone());
    assert!(matches!(
        stream.next().await,
        Some(HarnessEvent::Progress { .. })
    ));
    let started = Instant::now();
    cancel.cancel();
    let rest = collect(stream).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "SIGINT was honored"
    );
    assert_eq!(failure(&rest).0, HarnessFailure::Cancelled);
}

#[tokio::test]
async fn a_process_that_ignores_sigint_is_killed_after_the_grace_period() {
    let fake = Fake::new();
    let cancel = CancelToken::new();
    let mut stream = fake
        .claude("claude/in_progress.jsonl", "ignore-int")
        .run(research("q"), cancel.clone());
    assert!(matches!(
        stream.next().await,
        Some(HarnessEvent::Progress { .. })
    ));
    let started = Instant::now();
    cancel.cancel();
    let rest = collect(stream).await;
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(2_900) && waited < Duration::from_secs(6),
        "{waited:?}"
    );
    assert_eq!(failure(&rest).0, HarnessFailure::Cancelled);
}

fn is_alive(process_id: &str) -> bool {
    Command::new("/bin/kill")
        .args(["-0", process_id.trim()])
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test]
async fn dropping_the_stream_kills_the_process() {
    let fake = Fake::new();
    let mut stream = fake
        .claude("claude/in_progress.jsonl", "ignore-int")
        .run(research("q"), CancelToken::new());
    assert!(matches!(
        stream.next().await,
        Some(HarnessEvent::Progress { .. })
    ));
    let process_id = fake.recorded("process-id");
    assert!(is_alive(&process_id));
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(2);
    while is_alive(&process_id) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!is_alive(&process_id), "process {process_id} still running");
}

#[tokio::test]
async fn a_harness_that_lingers_after_its_result_is_stopped_before_the_result_is_delivered() {
    let fake = Fake::new();
    let started = Instant::now();
    let events = collect(
        fake.claude("claude/research.jsonl", "hang")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    assert!(matches!(last(&events), HarnessEvent::Finished { .. }));
    // stdin closed, one grace period, then SIGINT ends it.
    assert!(
        started.elapsed() >= Duration::from_millis(2_900),
        "{:?}",
        started.elapsed()
    );
    assert!(!is_alive(&fake.recorded("process-id")));
}

#[tokio::test]
async fn pi_research_maps_rpc_events_and_declines_dialogs() {
    let fake = Fake::new();
    let events = collect(
        fake.pi("pi/research.jsonl", "replay")
            .run(research("What does greet return?"), CancelToken::new()),
    )
    .await;
    assert_eq!(
        events[..2],
        [
            HarnessEvent::Progress {
                text: "Let me read the library.".into()
            },
            HarnessEvent::FileTouched {
                path: "src/lib.rs".into()
            },
        ]
    );
    assert_eq!(
        last(&events),
        &HarnessEvent::Finished {
            summary: "greet() returns \"hello\" (src/lib.rs:2).".into(),
            citations: vec!["src/lib.rs:2".into()],
        }
    );
    let arguments = fake.recorded("arguments");
    assert!(arguments.starts_with("--mode\nrpc\n--no-extensions\n--append-system-prompt\n"));
    assert!(arguments.contains("--tools\nread,grep,find,ls\n"));
    let prompt: serde_json::Value = serde_json::from_str(&fake.recorded("stdin")).expect("json");
    assert_eq!(
        prompt,
        serde_json::json!({"id": "boardai-prompt", "type": "prompt", "message": "What does greet return?"})
    );
    let reply: serde_json::Value =
        serde_json::from_str(fake.recorded("rest").trim()).expect("one reply");
    assert_eq!(
        reply,
        serde_json::json!({"type": "extension_ui_response", "id": "uuid-7", "cancelled": true})
    );
}

#[tokio::test]
async fn pi_failures_keep_their_kind() {
    let fake = Fake::new();
    let events = collect(
        fake.pi("pi/error.jsonl", "replay")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (reason, detail) = failure(&events);
    assert_eq!(reason, HarnessFailure::Crashed);
    assert!(detail.contains("401 invalid x-api-key"));

    let events = collect(
        fake.pi("pi/prompt_refused.jsonl", "replay")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    let (_, detail) = failure(&events);
    assert!(detail.contains("No API key found"));

    // An older pi without `agent_settled` still finishes when it exits after `agent_end`.
    let events = collect(
        fake.pi("pi/old_no_settled.jsonl", "replay-exit")
            .run(research("q"), CancelToken::new()),
    )
    .await;
    assert!(
        matches!(last(&events), HarnessEvent::Finished { summary, .. } if summary.starts_with("It returns hello"))
    );
}

fn git(repo: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn change(branch: &str) -> HarnessTask {
    HarnessTask {
        kind: HarnessKind::Change {
            branch: branch.into(),
        },
        brief: "Rename greet to welcome.".into(),
    }
}

#[tokio::test]
async fn a_change_runs_only_on_a_new_branch_of_a_clean_tree() {
    let fake = Fake::new();
    git(&fake.repo, &["init", "--quiet", "--initial-branch=main"]);
    git(&fake.repo, &["add", "."]);
    git(
        &fake.repo,
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
    );
    let harness = fake.claude("claude/research.jsonl", "replay");

    let events = collect(harness.run(change("main"), CancelToken::new())).await;
    assert!(
        failure(&events)
            .1
            .contains("not a valid boardai/<slug> branch")
    );

    std::fs::write(fake.repo.join("notes.txt"), "untracked").expect("write");
    let events = collect(harness.run(change("boardai/rename-greet"), CancelToken::new())).await;
    assert!(failure(&events).1.contains("uncommitted changes"));
    assert_eq!(
        git(&fake.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );

    std::fs::remove_file(fake.repo.join("notes.txt")).expect("remove");
    let events = collect(harness.run(change("boardai/rename-greet"), CancelToken::new())).await;
    assert!(
        matches!(last(&events), HarnessEvent::Finished { .. }),
        "{events:?}"
    );
    assert_eq!(
        git(&fake.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "boardai/rename-greet"
    );
    assert!(
        fake.recorded("arguments")
            .contains("--permission-mode\nacceptEdits\n")
    );
}

/// A harness that never ends until cancelled, and never spawns anything.
struct Idle;

impl CodingHarness for Idle {
    fn run_in(
        &self,
        task: HarnessTask,
        _workspace: boardai_runtime::harness::Workspace,
        cancel: CancelToken,
    ) -> HarnessStream {
        self.run(task, cancel)
    }

    fn run(&self, _task: HarnessTask, cancel: CancelToken) -> HarnessStream {
        Box::pin(futures_util::stream::once(async move {
            cancel.cancelled().await;
            HarnessEvent::Failed {
                reason: HarnessFailure::Cancelled,
                detail: "stopped".into(),
            }
        }))
    }
}

#[tokio::test]
async fn the_pool_runs_two_reads_queues_four_and_refuses_the_rest() {
    let pool = HarnessPool::new(Arc::new(Idle), HarnessTimeouts::default());
    let mut streams: Vec<HarnessStream> = (0..6)
        .map(|_| pool.run(research("q"), CancelToken::new()))
        .collect();
    let mut refused = pool.run(research("q"), CancelToken::new());
    let event = tokio::time::timeout(Duration::from_secs(1), refused.next())
        .await
        .expect("at once");
    assert!(matches!(
        event,
        Some(HarnessEvent::Failed {
            reason: HarnessFailure::QueueFull,
            ..
        })
    ));
    // A change has its own slot, so it is not refused by full read slots.
    let mut change_stream = pool.run(change("boardai/x"), CancelToken::new());
    let pending = tokio::time::timeout(Duration::from_millis(100), change_stream.next()).await;
    assert!(pending.is_err(), "the change is running, not refused");
    // Freeing a queued place lets the next task in line.
    streams.truncate(5);
    let mut late = pool.run(research("q"), CancelToken::new());
    let pending = tokio::time::timeout(Duration::from_millis(100), late.next()).await;
    assert!(pending.is_err(), "queued, not refused");
}

#[tokio::test]
async fn a_queued_task_starts_when_a_slot_frees() {
    let pool = HarnessPool::new(Arc::new(Idle), HarnessTimeouts::default());
    let first_cancel = CancelToken::new();
    let mut first = pool.run(change("boardai/a"), first_cancel.clone());
    let second_cancel = CancelToken::new();
    let mut second = pool.run(change("boardai/b"), second_cancel.clone());
    let first_task = tokio::spawn(async move { first.next().await });
    tokio::task::yield_now().await;
    first_cancel.cancel();
    assert!(matches!(
        first_task.await.expect("joins"),
        Some(HarnessEvent::Failed {
            reason: HarnessFailure::Cancelled,
            ..
        })
    ));
    second_cancel.cancel();
    let event = tokio::time::timeout(Duration::from_secs(1), second.next())
        .await
        .expect("runs");
    assert!(matches!(
        event,
        Some(HarnessEvent::Failed {
            reason: HarnessFailure::Cancelled,
            ..
        })
    ));
}

#[tokio::test]
async fn the_pool_times_tasks_out() {
    let fake = Fake::new();
    let timeouts = HarnessTimeouts {
        read_only: Duration::from_millis(300),
        change: Duration::from_millis(300),
    };
    let pool = HarnessPool::new(
        Arc::new(fake.claude("claude/in_progress.jsonl", "hang")),
        timeouts,
    );
    let events = collect(pool.run(research("q"), CancelToken::new())).await;
    assert_eq!(failure(&events).0, HarnessFailure::TimedOut);
}

#[test]
fn discovery_searches_path_then_the_usual_install_folders() {
    let folder = TempFolder::new("discovery");
    let first = folder.write("first/claude", b"#!/bin/sh\n");
    let second = folder.write("second/claude", b"#!/bin/sh\n");
    let not_executable = folder.write("first/pi", b"#!/bin/sh\n");
    for path in [&first, &second] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644))
        .expect("chmod");
    let path = std::env::join_paths([folder.path.join("first"), folder.path.join("second")])
        .expect("join");
    let found = discover(Some(&path), Some(&folder.path.join("home")));
    assert_eq!(found.claude_code, Some(first));
    assert!(
        found
            .pi
            .as_ref()
            .is_none_or(|pi| !pi.starts_with(&folder.path))
    );
}
