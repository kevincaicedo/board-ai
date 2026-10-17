//! pi in RPC mode (LLD §3.5, D-010): `pi --mode rpc`, strict LF-delimited JSON lines.
//!
//! The protocol follows pi's `docs/rpc.md` and `docs/json.md` (earendil-works/pi, formerly
//! badlogic/pi-mono, v1.3.0): one `prompt` command, then events until `agent_settled`, which
//! is the end of a run. stdin closes only after that, because closing it mid-run aborts pi.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitStatus;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use serde::Deserialize;
use serde_json::{Value, json};

use super::claude::BRIEF_RESEARCH_PROMPT;
use super::confine::reach_line;
use super::process::{self, Output, Preflight, ProcessSpec, Protocol, failed, failed_stream};
use super::text::{bounded_summary, citations, progress_note, relative_to};
use super::{Answer, CodingHarness, HarnessStream, Workspace};
use crate::cancel::CancelToken;
use crate::limits::HARNESS_FILES_TOUCHED_MAX;

const READ_ONLY_BRIEF: &str = "You are researching a codebase for BoardAI, a voice tutor that \
explains code out loud. Do not modify any file. End with a short, plain answer that can be \
spoken, and cite every claim's source as path:line.";

const CHANGE_BRIEF: &str = "You are making a change for BoardAI, which the user approved out \
loud. Work only on the current git branch: do not commit, push or switch branches. End with a \
short summary of what changed that can be spoken, citing each edit as path:line.";

/// The id our one command carries; responses repeat it.
const PROMPT_ID: &str = "boardai-prompt";

#[derive(Clone, Debug)]
pub struct PiConfig {
    pub program: PathBuf,
    /// As for Claude Code: the default folder, `None` when no code folder is set.
    pub repo: Option<PathBuf>,
    pub environment: Vec<(OsString, OsString)>,
}

#[derive(Clone, Debug)]
pub struct PiHarness {
    config: PiConfig,
}

impl PiHarness {
    #[must_use]
    pub fn new(config: PiConfig) -> Self {
        Self { config }
    }
}

impl CodingHarness for PiHarness {
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream {
        let Some(folder) = self.config.repo.clone() else {
            return failed_stream(HarnessFailure::NotInstalled, "no code folder is open");
        };
        let workspace = Workspace {
            folder,
            answer: Answer::Spoken,
        };
        self.run_in(task, workspace, cancel)
    }

    fn run_in(
        &self,
        task: HarnessTask,
        workspace: Workspace,
        cancel: CancelToken,
    ) -> HarnessStream {
        let preflight = match &task.kind {
            HarnessKind::Research | HarnessKind::Review => Preflight::None,
            HarnessKind::Change { branch } => Preflight::Branch {
                branch: branch.clone(),
            },
        };
        let spec = ProcessSpec {
            program: self.config.program.clone(),
            arguments: arguments(&task.kind, &workspace),
            working_directory: workspace.folder.clone(),
            environment: self.config.environment.clone(),
        };
        let repo = std::fs::canonicalize(&workspace.folder).unwrap_or(workspace.folder);
        let protocol = PiProtocol {
            brief: task.brief,
            repo,
            touched: BTreeSet::new(),
            last_text: String::new(),
            last_stop: None,
            last_error: None,
            run_ended: false,
        };
        process::run(spec, preflight, protocol, cancel)
    }
}

/// pi has no working-directory flag; the process runs in the repo. Extensions are off because
/// an extension dialog would wait for an answer we never give. pi has no permission rules, so a
/// brief's reach (D-226) is its read-only tools, its working folder and the prompt
/// (`confine`); it keeps its own model.
#[must_use]
pub fn arguments(kind: &HarnessKind, workspace: &Workspace) -> Vec<OsString> {
    let mut command_line: Vec<String> =
        ["--mode", "rpc", "--no-extensions", "--append-system-prompt"]
            .map(str::to_owned)
            .to_vec();
    match kind {
        HarnessKind::Research | HarnessKind::Review => {
            let prompt = match &workspace.answer {
                Answer::Spoken => READ_ONLY_BRIEF.to_owned(),
                Answer::Brief { reach } => format!(
                    "{BRIEF_RESEARCH_PROMPT} {}",
                    reach_line(&workspace.folder, reach)
                ),
            };
            command_line.extend([prompt, "--tools".to_owned(), "read,grep,find,ls".to_owned()]);
        }
        HarnessKind::Change { .. } => command_line.push(CHANGE_BRIEF.to_owned()),
    }
    command_line.into_iter().map(OsString::from).collect()
}

struct PiProtocol {
    brief: String,
    repo: PathBuf,
    touched: BTreeSet<String>,
    last_text: String,
    last_stop: Option<String>,
    last_error: Option<String>,
    /// `agent_end` without a retry pending: enough to finish on an older pi that never sends
    /// `agent_settled`.
    run_ended: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PiLine {
    Response {
        #[serde(default)]
        command: String,
        #[serde(default)]
        success: bool,
        #[serde(default)]
        error: Option<String>,
    },
    ToolExecutionStart {
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "args", default)]
        arguments: Value,
    },
    MessageEnd {
        message: PiMessage,
    },
    AgentEnd {
        #[serde(rename = "willRetry", default)]
        will_retry: bool,
    },
    AgentSettled {
        #[serde(default)]
        aborted: bool,
    },
    ExtensionUiRequest {
        id: String,
        #[serde(default)]
        method: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct PiMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Value,
    #[serde(rename = "stopReason", default)]
    stop_reason: Option<String>,
    #[serde(rename = "errorMessage", default)]
    error_message: Option<String>,
}

impl Protocol for PiProtocol {
    fn opening(&mut self) -> Vec<String> {
        vec![json!({"id": PROMPT_ID, "type": "prompt", "message": self.brief}).to_string()]
    }

    fn on_line(&mut self, line: &[u8], output: &mut Output) {
        let Ok(parsed) = serde_json::from_slice::<PiLine>(line) else {
            tracing::debug!(bytes = line.len(), "skipped an unreadable pi line");
            return;
        };
        match parsed {
            PiLine::Response {
                command,
                success,
                error,
            } => self.on_response(&command, success, error, output),
            PiLine::ToolExecutionStart {
                tool_name,
                arguments,
            } => {
                self.on_tool(&tool_name, &arguments, output);
            }
            PiLine::MessageEnd { message } => self.on_message(message, output),
            PiLine::AgentEnd { will_retry } => self.run_ended = !will_retry,
            PiLine::AgentSettled { aborted } => output.events.push(self.settle(aborted)),
            PiLine::ExtensionUiRequest { id, method } => {
                // Dialogs block until answered; we always decline.
                if matches!(method.as_str(), "select" | "confirm" | "input" | "editor") {
                    let reply =
                        json!({"type": "extension_ui_response", "id": id, "cancelled": true});
                    output.replies.push(reply.to_string());
                }
            }
            PiLine::Other => {}
        }
    }

    fn on_exit(&mut self, status: Option<ExitStatus>, stderr_tail: &str) -> HarnessEvent {
        if self.run_ended {
            return self.settle(false);
        }
        let status =
            status.map_or_else(|| "unknown status".to_owned(), |status| status.to_string());
        failed(
            HarnessFailure::Crashed,
            format!("pi ended ({status}) before the run settled: {stderr_tail}"),
        )
    }
}

impl PiProtocol {
    fn on_response(
        &self,
        command: &str,
        success: bool,
        error: Option<String>,
        output: &mut Output,
    ) {
        if success {
            return;
        }
        let detail = bounded_summary(&error.unwrap_or_default());
        let reason = if command == "parse" {
            HarnessFailure::BadOutput
        } else {
            HarnessFailure::Crashed
        };
        output
            .events
            .push(failed(reason, format!("pi refused `{command}`: {detail}")));
    }

    fn on_tool(&mut self, tool_name: &str, arguments: &Value, output: &mut Output) {
        if !matches!(tool_name, "read" | "edit" | "write") {
            return;
        }
        if let Some(path) = arguments.get("path").and_then(Value::as_str)
            && self.touched.len() < HARNESS_FILES_TOUCHED_MAX
        {
            let path = relative_to(&self.repo, path);
            if self.touched.insert(path.clone()) {
                output.events.push(HarnessEvent::FileTouched { path });
            }
        }
    }

    fn on_message(&mut self, message: PiMessage, output: &mut Output) {
        if message.role != "assistant" {
            return;
        }
        let text = message_text(&message.content);
        // Notes written between tool calls are progress; the last message is the answer.
        if message.stop_reason.as_deref() == Some("toolUse")
            && let Some(note) = progress_note(&text)
        {
            output.events.push(HarnessEvent::Progress { text: note });
        }
        if !text.trim().is_empty() {
            self.last_text = text;
        }
        self.last_stop = message.stop_reason;
        self.last_error = message.error_message;
    }

    fn settle(&self, aborted: bool) -> HarnessEvent {
        if aborted {
            return failed(HarnessFailure::Cancelled, "pi aborted the run".to_owned());
        }
        if self.last_stop.as_deref() == Some("error") {
            let detail = self.last_error.clone().unwrap_or_default();
            return failed(
                HarnessFailure::Crashed,
                bounded_summary(&format!("pi failed: {detail}")),
            );
        }
        HarnessEvent::Finished {
            citations: citations(&self.last_text, &self.repo),
            summary: bounded_summary(&self.last_text),
        }
    }
}

/// A message's text: a plain string, or its `text` blocks joined.
fn message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => String::new(),
    }
}
