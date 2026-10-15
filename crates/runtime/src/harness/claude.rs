//! Claude Code in headless mode (LLD §3.5, D-010): `claude -p` with stream-json in and out.
//!
//! The brief goes in as one user message on stdin; stdin closes once the result arrives. We
//! are the "host" of a stream-json session, and we answer no permission prompts, so
//! `--permission-prompts none` makes anything that would prompt fail instead of hanging.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitStatus;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use serde::Deserialize;
use serde_json::{Value, json};

use super::process::{self, Output, Preflight, ProcessSpec, Protocol, failed};
use super::text::{bounded_summary, citations, new_session_id, progress_note, relative_to};
use super::{CodingHarness, HarnessStream};
use crate::cancel::CancelToken;
use crate::limits::HARNESS_FILES_TOUCHED_MAX;

/// Appended to Claude Code's own system prompt for read-only work.
const READ_ONLY_BRIEF: &str = "You are researching a codebase for BoardAI, a voice tutor that \
explains code out loud. Do not modify any file. Read only what you need. End with a short, \
plain answer that can be spoken, and cite every claim's source as path:line.";

/// Appended for a change task.
const CHANGE_BRIEF: &str = "You are making a change for BoardAI, which the user approved out \
loud. Work only on the current git branch: do not commit, push or switch branches. End with a \
short summary of what changed that can be spoken, citing each edit as path:line.";

#[derive(Clone, Debug)]
pub struct ClaudeCodeConfig {
    /// The `claude` executable, found on `PATH` at startup.
    pub program: PathBuf,
    /// The repo the task runs in.
    pub repo: PathBuf,
    /// Extra environment for the process (tests use it to steer a fake harness).
    pub environment: Vec<(OsString, OsString)>,
}

#[derive(Clone, Debug)]
pub struct ClaudeCodeHarness {
    config: ClaudeCodeConfig,
}

impl ClaudeCodeHarness {
    #[must_use]
    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }
}

impl CodingHarness for ClaudeCodeHarness {
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream {
        let preflight = match &task.kind {
            HarnessKind::Research | HarnessKind::Review => Preflight::None,
            HarnessKind::Change { branch } => Preflight::Branch {
                branch: branch.clone(),
            },
        };
        let spec = ProcessSpec {
            program: self.config.program.clone(),
            arguments: arguments(&task.kind, &new_session_id()),
            working_directory: self.config.repo.clone(),
            environment: self.config.environment.clone(),
        };
        // Claude Code reports paths under its resolved working directory.
        let repo =
            std::fs::canonicalize(&self.config.repo).unwrap_or_else(|_| self.config.repo.clone());
        let protocol = ClaudeProtocol {
            brief: task.brief,
            repo,
            touched: BTreeSet::new(),
            unreadable_lines: 0,
            held_note: None,
        };
        process::run(spec, preflight, protocol, cancel)
    }
}

/// The command line, checked against `claude --help` (2.1.301). Multi-value flags get one
/// comma-separated argument so they cannot swallow the next flag.
#[must_use]
pub fn arguments(kind: &HarnessKind, session_id: &str) -> Vec<OsString> {
    let mut command_line: Vec<&str> = vec![
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--session-id",
        session_id,
        "--permission-prompts",
        "none",
        "--append-system-prompt",
    ];
    match kind {
        HarnessKind::Research | HarnessKind::Review => command_line.extend([
            READ_ONLY_BRIEF,
            "--permission-mode",
            "plan",
            "--allowedTools",
            "Read,Grep,Glob",
        ]),
        HarnessKind::Change { .. } => {
            command_line.extend([CHANGE_BRIEF, "--permission-mode", "acceptEdits"]);
        }
    }
    command_line.into_iter().map(OsString::from).collect()
}

struct ClaudeProtocol {
    brief: String,
    repo: PathBuf,
    touched: BTreeSet<String>,
    unreadable_lines: u64,
    /// The latest text note, held until the next block: the text right before the result is
    /// the answer itself, which arrives as the summary, so it is not also a progress note.
    held_note: Option<String>,
}

/// The stream-json lines we read. Everything else (init, partial `stream_event`s, tool
/// results, rate-limit notices) is skipped.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamLine {
    Assistant {
        message: AssistantMessage,
    },
    Result {
        #[serde(default)]
        subtype: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        result: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Other,
}

impl Protocol for ClaudeProtocol {
    fn opening(&mut self) -> Vec<String> {
        let message = json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": self.brief}]},
        });
        vec![message.to_string()]
    }

    fn on_line(&mut self, line: &[u8], output: &mut Output) {
        let Ok(parsed) = serde_json::from_slice::<StreamLine>(line) else {
            self.unreadable_lines += 1;
            tracing::debug!(bytes = line.len(), "skipped an unreadable Claude Code line");
            return;
        };
        match parsed {
            StreamLine::Assistant { message } => {
                for block in message.content {
                    self.on_block(block, output);
                }
            }
            StreamLine::Result {
                subtype,
                is_error,
                result,
            } => output
                .events
                .push(self.on_result(&subtype, is_error, result)),
            StreamLine::Other => {}
        }
    }

    fn on_exit(&mut self, status: Option<ExitStatus>, stderr_tail: &str) -> HarnessEvent {
        let reason = if self.unreadable_lines > 0 {
            HarnessFailure::BadOutput
        } else {
            HarnessFailure::Crashed
        };
        let status =
            status.map_or_else(|| "unknown status".to_owned(), |status| status.to_string());
        failed(
            reason,
            format!("Claude Code ended ({status}) without a result: {stderr_tail}"),
        )
    }
}

impl ClaudeProtocol {
    fn on_block(&mut self, block: ContentBlock, output: &mut Output) {
        match block {
            ContentBlock::Text { text } => {
                if let Some(note) = progress_note(&text) {
                    self.release_note(output);
                    self.held_note = Some(note);
                }
            }
            ContentBlock::ToolUse { name, input } => {
                self.release_note(output);
                let path_field = match name.as_str() {
                    "Read" | "Edit" | "Write" | "MultiEdit" => "file_path",
                    "NotebookEdit" => "notebook_path",
                    _ => return,
                };
                if let Some(path) = input.get(path_field).and_then(Value::as_str)
                    && self.touched.len() < HARNESS_FILES_TOUCHED_MAX
                {
                    let path = relative_to(&self.repo, path);
                    if self.touched.insert(path.clone()) {
                        output.events.push(HarnessEvent::FileTouched { path });
                    }
                }
            }
            ContentBlock::Other => {}
        }
    }

    fn release_note(&mut self, output: &mut Output) {
        if let Some(text) = self.held_note.take() {
            output.events.push(HarnessEvent::Progress { text });
        }
    }

    fn on_result(&self, subtype: &str, is_error: bool, result: Option<String>) -> HarnessEvent {
        let text = result.unwrap_or_default();
        if subtype == "success" && !is_error {
            return HarnessEvent::Finished {
                citations: citations(&text, &self.repo),
                summary: bounded_summary(&text),
            };
        }
        failed(
            HarnessFailure::Crashed,
            bounded_summary(&format!("Claude Code reported {subtype}: {text}")),
        )
    }
}
