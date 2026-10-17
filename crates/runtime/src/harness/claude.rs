//! Claude Code in headless mode (LLD §3.5, D-010): `claude -p` with stream-json in and out.
//!
//! The brief goes in as one user message on stdin; stdin closes once the result arrives. We
//! are the "host" of a stream-json session, and we answer no permission prompts, so
//! `--permission-prompts none` makes anything that would prompt fail instead of hanging.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use serde::Deserialize;
use serde_json::{Value, json};

use super::confine::{claude_rules, reach_line};
use super::process::{self, Output, Preflight, ProcessSpec, Protocol, failed, failed_stream};
use super::text::{bounded_summary, citations, new_session_id, progress_note, relative_to};
use super::{Answer, CodingHarness, DefaultFolder, HarnessStream, Workspace};
use crate::cancel::CancelToken;
use crate::limits::HARNESS_FILES_TOUCHED_MAX;

/// Appended to Claude Code's own system prompt for read-only work.
const READ_ONLY_BRIEF: &str = "You are researching a codebase for BoardAI, a voice tutor that \
explains code out loud. Do not modify any file. Read only what you need. End with a short, \
plain answer that can be spoken, and cite every claim's source as path:line.";

/// Appended for a brief request (D-185): the task message carries the shape.
pub(crate) const BRIEF_RESEARCH_PROMPT: &str = "You are researching code for BoardAI, a voice \
tutor that explains code while drawing it. Do not modify any file. Read what you need to answer \
well. Answer only in the Markdown shape the task gives, with paths relative to the working \
folder, and cite only lines you read, as path:line.";

/// The model a research brief runs on unless `[harness] research_model` names another (D-226):
/// Claude Code's alias for its current Sonnet, cheaper and faster than the default for reading.
pub const DEFAULT_RESEARCH_MODEL: &str = "sonnet";

/// Appended for a change task.
const CHANGE_BRIEF: &str = "You are making a change for BoardAI, which the user approved out \
loud. Work only on the current git branch: do not commit, push or switch branches. End with a \
short summary of what changed that can be spoken, citing each edit as path:line.";

#[derive(Clone, Debug)]
pub struct ClaudeCodeConfig {
    /// The `claude` executable, found on `PATH` at startup.
    pub program: PathBuf,
    /// The repo a task runs in unless it names its own folder: the first code folder and its
    /// reach, `None` when there is none (such a task fails at once).
    pub repo: Option<DefaultFolder>,
    /// Extra environment for the process (tests use it to steer a fake harness).
    pub environment: Vec<(OsString, OsString)>,
    /// `--model` for research briefs ([`DEFAULT_RESEARCH_MODEL`] unless settings name one).
    pub research_model: String,
    /// `--model` for spoken research, reviews and changes (`[harness] model`); `None` keeps
    /// Claude Code's own default.
    pub model: Option<String>,
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
        let Some(repo) = &self.config.repo else {
            return failed_stream(HarnessFailure::NotInstalled, "no code folder is open");
        };
        self.run_in(task, repo.spoken(), cancel)
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
            arguments: arguments(
                &task.kind,
                &new_session_id(),
                &workspace,
                Models {
                    research: &self.config.research_model,
                    other: self.config.model.as_deref(),
                },
            ),
            working_directory: workspace.folder.clone(),
            environment: self.config.environment.clone(),
        };
        // Claude Code reports paths under its resolved working directory.
        let repo = std::fs::canonicalize(&workspace.folder).unwrap_or(workspace.folder);
        let protocol = ClaudeProtocol {
            brief: task.brief,
            reach: match task.kind {
                HarnessKind::Research | HarnessKind::Review => Some(workspace.reach),
                HarnessKind::Change { .. } => None,
            },
            repo,
            touched: BTreeSet::new(),
            unreadable_lines: 0,
            held_note: None,
        };
        process::run(spec, preflight, protocol, cancel)
    }
}

/// The models a task may run on: briefs on the research model, everything else on
/// `[harness] model` when it names one.
#[derive(Clone, Copy, Debug)]
pub struct Models<'a> {
    pub research: &'a str,
    pub other: Option<&'a str>,
}

/// The command line, checked against `claude --help` (2.1.302). Multi-value flags get one
/// comma-separated argument so they cannot swallow the next flag. Research and reviews, spoken
/// or as a brief, are held to the workspace's reach by Claude Code's permission rules
/// (`confine`, D-226, D-243); a change edits its branch.
#[must_use]
pub fn arguments(
    kind: &HarnessKind,
    session_id: &str,
    workspace: &Workspace,
    models: Models<'_>,
) -> Vec<OsString> {
    let mut command_line: Vec<String> = [
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
    ]
    .map(str::to_owned)
    .to_vec();
    let reach = reach_line(&workspace.folder, &workspace.reach);
    let (prompt, model) = match (kind, workspace.answer) {
        (HarnessKind::Research | HarnessKind::Review, Answer::Brief) => (
            format!("{BRIEF_RESEARCH_PROMPT} {reach}"),
            Some(models.research),
        ),
        (HarnessKind::Research | HarnessKind::Review, Answer::Spoken) => {
            (format!("{READ_ONLY_BRIEF} {reach}"), models.other)
        }
        (HarnessKind::Change { .. }, Answer::Spoken | Answer::Brief) => {
            (CHANGE_BRIEF.to_owned(), models.other)
        }
    };
    command_line.push(prompt);
    if let Some(model) = model {
        command_line.extend(["--model".to_owned(), model.to_owned()]);
    }
    match kind {
        HarnessKind::Research | HarnessKind::Review => {
            let rules = claude_rules(&workspace.folder, &workspace.reach);
            command_line.extend(
                [
                    "--permission-mode",
                    "dontAsk",
                    "--tools",
                    "Read,Grep,Glob",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "user",
                    "--settings",
                    rules.as_str(),
                ]
                .map(str::to_owned),
            );
        }
        HarnessKind::Change { .. } => {
            command_line.extend(["--permission-mode", "acceptEdits"].map(str::to_owned));
        }
    }
    command_line.into_iter().map(OsString::from).collect()
}

struct ClaudeProtocol {
    brief: String,
    /// A brief's reach (D-226): a file it names outside the reach was refused by the rules,
    /// and isn't shown as read.
    reach: Option<PathBuf>,
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
                    && let Some(path) = self.shown_touched(path)
                    && self.touched.insert(path.clone())
                {
                    output.events.push(HarnessEvent::FileTouched { path });
                }
            }
            ContentBlock::Other => {}
        }
    }

    /// How a file the harness named is shown as read: relative to its working folder. A
    /// brief's file outside its reach was refused by the rules and isn't shown (no absolute
    /// path reaches the tray); one inside the reach but outside the folder is `../…`.
    fn shown_touched(&self, path: &str) -> Option<String> {
        let Some(reach) = &self.reach else {
            return Some(relative_to(&self.repo, path));
        };
        let file = lexical(&self.repo.join(path));
        if let Ok(inside) = file.strip_prefix(&self.repo) {
            return Some(inside.to_string_lossy().into_owned());
        }
        let in_reach = file.strip_prefix(reach).ok()?;
        let depth = self.repo.strip_prefix(reach).ok()?.components().count();
        let mut shown = "../".repeat(depth);
        shown.push_str(&in_reach.to_string_lossy());
        Some(shown)
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

/// `path` with `.` and `..` folded away, without asking the file system.
fn lexical(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => normal.push(part),
        }
    }
    normal
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use boardai_core::harness::{HarnessEvent, HarnessKind};
    use serde_json::Value;

    use super::{ClaudeProtocol, DEFAULT_RESEARCH_MODEL, Models, arguments};
    use crate::harness::process::{Output, Protocol};
    use crate::harness::{Answer, Workspace};

    const DEFAULT_MODELS: Models<'static> = Models {
        research: DEFAULT_RESEARCH_MODEL,
        other: None,
    };

    fn brief_workspace() -> Workspace {
        Workspace {
            folder: PathBuf::from("/code/db/crates/alloc"),
            reach: PathBuf::from("/code/db"),
            answer: Answer::Brief,
        }
    }

    /// The opened code folder, in a repository it is the root of.
    fn spoken_workspace() -> Workspace {
        Workspace {
            folder: PathBuf::from("/code/db"),
            reach: PathBuf::from("/code/db"),
            answer: Answer::Spoken,
        }
    }

    fn after<'a>(command_line: &'a [OsString], flag: &str) -> Option<&'a str> {
        let index = command_line.iter().position(|part| part == flag)?;
        command_line.get(index + 1)?.to_str()
    }

    /// What a read-only command line gets wrong about confinement (D-226, D-243): every way it
    /// could read outside `reach`, run another tool or load the repository's own settings.
    fn unconfined(command_line: &[OsString], reach: &str) -> Vec<String> {
        let mut wrong = Vec::new();
        if command_line.iter().any(|part| part == "--allowedTools") {
            wrong.push("an unscoped --allowedTools".to_owned());
        }
        let mut expect = |flag: &str, value: &str| {
            let found = after(command_line, flag);
            if found != Some(value) {
                wrong.push(format!("{flag} is {found:?}, not {value}"));
            }
        };
        expect("--permission-mode", "dontAsk");
        expect("--tools", "Read,Grep,Glob");
        expect("--setting-sources", "user");
        if !command_line
            .iter()
            .any(|part| part == "--strict-mcp-config")
        {
            wrong.push("MCP servers may load".to_owned());
        }
        let rules: Option<Value> =
            after(command_line, "--settings").and_then(|rules| serde_json::from_str(rules).ok());
        let Some(rules) = rules else {
            wrong.push("no permission rules".to_owned());
            return wrong;
        };
        let permissions = &rules["permissions"];
        if permissions["allow"] != serde_json::json!([format!("Read(/{reach}/**)")]) {
            wrong.push(format!("the allow rule is {}", permissions["allow"]));
        }
        if permissions["blockReadsOutsideWorkingDirectories"] != true {
            wrong.push("reads outside the working directories aren't blocked".to_owned());
        }
        let deny = permissions["deny"].to_string();
        for tool in [
            "\"Bash\"",
            "\"Edit\"",
            "\"Write\"",
            "\"WebFetch\"",
            "\"mcp__*\"",
        ] {
            if !deny.contains(tool) {
                wrong.push(format!("{tool} isn't denied"));
            }
        }
        wrong
    }

    /// D-226: a brief runs on the research model, with only the read tools, and with
    /// Claude Code's permission rules holding it to its reach; anything else is denied.
    #[test]
    fn a_brief_command_line_carries_the_reach_rules() {
        let command_line = arguments(
            &HarnessKind::Research,
            "id",
            &brief_workspace(),
            DEFAULT_MODELS,
        );
        assert_eq!(after(&command_line, "--model"), Some("sonnet"));
        assert_eq!(unconfined(&command_line, "/code/db"), Vec::<String>::new());
        let rules: Value =
            serde_json::from_str(after(&command_line, "--settings").expect("--settings"))
                .expect("json");
        assert_eq!(rules["permissions"]["additionalDirectories"][0], "/code/db");
        let prompt = after(&command_line, "--append-system-prompt").expect("a prompt");
        assert!(prompt.contains("(../.. from here)"), "{prompt}");
    }

    /// D-243: spoken research and reviews are held to the opened folder's repository the same
    /// way. The command line spoken research had before (plan mode, an unscoped read allow) is
    /// the planted violation: the check finds everything wrong with it.
    #[test]
    fn spoken_research_and_reviews_are_confined_to_the_reach() {
        let nested = Workspace {
            folder: PathBuf::from("/code/db/crates/alloc"),
            ..spoken_workspace()
        };
        for kind in [HarnessKind::Research, HarnessKind::Review] {
            for workspace in [spoken_workspace(), nested.clone()] {
                let command_line = arguments(&kind, "id", &workspace, DEFAULT_MODELS);
                assert_eq!(
                    unconfined(&command_line, "/code/db"),
                    Vec::<String>::new(),
                    "{kind:?} in {}",
                    workspace.folder.display()
                );
                let prompt = after(&command_line, "--append-system-prompt").expect("a prompt");
                assert!(prompt.contains("short, plain answer"), "{prompt}");
                assert!(prompt.contains("Read only inside"), "{prompt}");
            }
        }
        let planted: Vec<OsString> = [
            "-p",
            "--append-system-prompt",
            "x",
            "--permission-mode",
            "plan",
            "--allowedTools",
            "Read,Grep,Glob",
        ]
        .map(OsString::from)
        .to_vec();
        let wrong = unconfined(&planted, "/code/db");
        assert!(
            wrong.iter().any(|why| why.contains("--allowedTools"))
                && wrong.iter().any(|why| why.contains("--permission-mode"))
                && wrong.iter().any(|why| why.contains("no permission rules")),
            "{wrong:?}"
        );
        let elsewhere = arguments(
            &HarnessKind::Research,
            "id",
            &spoken_workspace(),
            DEFAULT_MODELS,
        );
        assert!(
            !unconfined(&elsewhere, "/code/other").is_empty(),
            "a rule for another folder is caught"
        );
    }

    /// `[harness] model` runs spoken research, reviews and changes; briefs keep the research
    /// model (D-226). Without it, Claude Code's default runs them. A change is never confined:
    /// it edits its branch, asked first (I9).
    #[test]
    fn the_harness_model_runs_everything_but_briefs() {
        let models = Models {
            research: DEFAULT_RESEARCH_MODEL,
            other: Some("opus"),
        };
        let change = HarnessKind::Change {
            branch: "boardai/x".into(),
        };
        for kind in [HarnessKind::Research, HarnessKind::Review, change.clone()] {
            let command_line = arguments(&kind, "id", &spoken_workspace(), models);
            assert_eq!(after(&command_line, "--model"), Some("opus"), "{kind:?}");
            let default = arguments(&kind, "id", &spoken_workspace(), DEFAULT_MODELS);
            assert_eq!(after(&default, "--model"), None, "{kind:?}");
        }
        let brief = arguments(&HarnessKind::Research, "id", &brief_workspace(), models);
        assert_eq!(after(&brief, "--model"), Some("sonnet"));
        let custom = Models {
            research: "opus",
            other: None,
        };
        let brief = arguments(&HarnessKind::Research, "id", &brief_workspace(), custom);
        assert_eq!(
            after(&brief, "--model"),
            Some("opus"),
            "research_model wins"
        );
        let change = arguments(&change, "id", &spoken_workspace(), DEFAULT_MODELS);
        assert_eq!(after(&change, "--permission-mode"), Some("acceptEdits"));
        assert_eq!(after(&change, "--settings"), None);
    }

    /// A brief's reads show in the tray by their path from its folder; one outside its reach
    /// was refused and shows nothing, never an absolute path.
    #[test]
    fn a_briefs_reads_show_only_inside_its_reach() {
        let mut protocol = ClaudeProtocol {
            brief: String::new(),
            reach: Some(PathBuf::from("/code/db")),
            repo: PathBuf::from("/code/db/crates/alloc"),
            touched: BTreeSet::new(),
            unreadable_lines: 0,
            held_note: None,
        };
        let mut output = Output {
            events: Vec::new(),
            replies: Vec::new(),
        };
        for path in [
            "/code/db/crates/alloc/src/arena.rs",
            "/code/db/crates/alloc/../store/src/log.rs",
            "/Users/me/.ssh/id_ed25519",
            "../../../other/secret.rs",
        ] {
            let line = serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "Read", "input": {"file_path": path}}]},
            });
            protocol.on_line(line.to_string().as_bytes(), &mut output);
        }
        let shown: Vec<&str> = output
            .events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::FileTouched { path } => Some(path.as_str()),
                HarnessEvent::Progress { .. }
                | HarnessEvent::Finished { .. }
                | HarnessEvent::Failed { .. } => None,
            })
            .collect();
        assert_eq!(shown, ["src/arena.rs", "../../crates/store/src/log.rs"]);
        assert!(Path::new(shown[1]).is_relative());
    }
}
