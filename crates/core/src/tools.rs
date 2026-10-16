//! The talker's tools (LLD §3.3). The set is fixed per session. A call is parsed here, once,
//! from the provider's JSON into a [`ToolCall`]; inner code never sees unchecked input.

use serde::{Deserialize, Serialize};

use crate::ids::{TaskId, ToolCallId};
use crate::limits::{TOOL_ARGUMENT_MAX_BYTES, TOOL_INPUT_MAX_BYTES};
use crate::memory::Subject;
use crate::views::View;

mod schema;
pub use schema::{ToolSchema, schemas};

/// A validated tool call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolCall {
    ReadFile {
        path: String,
        start_line: Option<u32>,
        end_line: Option<u32>,
    },
    SearchCode {
        query: String,
        glob: Option<String>,
    },
    ListDir {
        path: String,
    },
    Research {
        question: String,
        depth: ResearchDepth,
    },
    Review {
        scope: String,
    },
    ProposeChange {
        instruction: String,
    },
    Remember {
        kind: RememberKind,
        text: String,
        /// Who it is about; it picks the memory file (notes ignore it).
        about: Subject,
        links: Vec<String>,
    },
    AddTask {
        title: String,
        project: Option<String>,
        links: Vec<String>,
    },
    Recall {
        query: String,
    },
    OpenSession {
        id: String,
    },
    /// Opens the library of past sessions.
    ShowLibrary,
    /// Opens the Now list: open tasks for one project, or all.
    ShowTasks {
        project: Option<String>,
    },
    /// Opens memory: what BoardAI keeps about the user and their systems, and recent notes.
    ShowMemory,
    /// Replays a past session like a film. `id` is a session id or words from its title.
    PlaySession {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchDepth {
    Quick,
    Deep,
}

/// What the talker asks to remember. The memory policy decides what happens to it (LLD §2.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RememberKind {
    /// Something you said about yourself or your systems.
    Stated,
    /// Something the talker concluded; proposed, never kept without a yes.
    Inferred,
    /// "Note that…".
    Note,
}

/// Every tool name the talker may call, in declaration order.
pub const TOOL_NAMES: [&str; 14] = [
    "read_file",
    "search_code",
    "list_dir",
    "research",
    "review",
    "propose_change",
    "remember",
    "add_task",
    "recall",
    "open_session",
    "show_library",
    "show_tasks",
    "show_memory",
    "play_session",
];

/// Why a tool call was rejected at the boundary. The talker gets this back as an error result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvalidToolInput {
    UnknownTool { name: String },
    TooLarge { size_bytes: usize },
    Malformed { reason: String },
}

/// A tool call as the talker sent it: the raw input is kept for the append-only history, and
/// `call` holds the parse result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: ToolCallId,
    pub name: String,
    pub input_json: String,
    pub call: Result<ToolCall, InvalidToolInput>,
}

impl ToolCall {
    /// Parses a tool call from its name and JSON input. Unknown fields, empty required strings
    /// and oversized input are rejected.
    pub fn parse(name: &str, input_json: &str) -> Result<Self, InvalidToolInput> {
        if input_json.len() > TOOL_INPUT_MAX_BYTES {
            return Err(InvalidToolInput::TooLarge {
                size_bytes: input_json.len(),
            });
        }
        if !TOOL_NAMES.contains(&name) {
            return Err(InvalidToolInput::UnknownTool {
                name: name.to_owned(),
            });
        }
        let tagged = tag_input(name, input_json)?;
        let call: Self =
            serde_json::from_value(tagged).map_err(|error| InvalidToolInput::Malformed {
                reason: error.to_string(),
            })?;
        call.check_arguments()?;
        Ok(call)
    }

    fn check_arguments(&self) -> Result<(), InvalidToolInput> {
        let required: &[&str] = match self {
            Self::ReadFile { path, .. } | Self::ListDir { path } => &[path],
            Self::SearchCode { query, .. } | Self::Recall { query } => &[query],
            Self::Research { question, .. } => &[question],
            Self::Review { scope } => &[scope],
            Self::ProposeChange { instruction } => &[instruction],
            Self::Remember { text, .. } => &[text],
            Self::AddTask { title, .. } => &[title],
            Self::OpenSession { id } | Self::PlaySession { id } => &[id],
            Self::ShowLibrary | Self::ShowTasks { .. } | Self::ShowMemory => &[],
        };
        for argument in required {
            if argument.trim().is_empty() {
                return Err(malformed("a required argument is empty"));
            }
            if argument.len() > TOOL_ARGUMENT_MAX_BYTES {
                return Err(malformed("an argument is too long"));
            }
        }
        if let Self::ReadFile {
            start_line: Some(start),
            end_line: Some(end),
            ..
        } = self
            && (*start == 0 || end < start)
        {
            return Err(malformed("lines are 1-based and start_line <= end_line"));
        }
        Ok(())
    }
}

fn malformed(reason: &str) -> InvalidToolInput {
    InvalidToolInput::Malformed {
        reason: reason.to_owned(),
    }
}

/// Adds the `tool` tag to the provider's input object so serde can pick the variant, and
/// rejects keys the tool doesn't declare.
fn tag_input(name: &str, input_json: &str) -> Result<serde_json::Value, InvalidToolInput> {
    let value: serde_json::Value =
        serde_json::from_str(input_json).map_err(|error| InvalidToolInput::Malformed {
            reason: error.to_string(),
        })?;
    let serde_json::Value::Object(mut fields) = value else {
        return Err(malformed("the input must be a JSON object"));
    };
    let allowed = declared_fields(name);
    if let Some(unknown) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(InvalidToolInput::Malformed {
            reason: format!("unknown field {unknown:?}"),
        });
    }
    fill_optional_lists(name, &mut fields);
    fields.insert(
        "tool".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    Ok(serde_json::Value::Object(fields))
}

/// Optional list arguments may be omitted by the model; they default to empty.
fn fill_optional_lists(name: &str, fields: &mut serde_json::Map<String, serde_json::Value>) {
    if matches!(name, "remember" | "add_task") && !fields.contains_key("links") {
        fields.insert("links".to_owned(), serde_json::Value::Array(Vec::new()));
    }
}

fn declared_fields(name: &str) -> &'static [&'static str] {
    match name {
        "read_file" => &["path", "start_line", "end_line"],
        "search_code" => &["query", "glob"],
        "list_dir" => &["path"],
        "research" => &["question", "depth"],
        "review" => &["scope"],
        "propose_change" => &["instruction"],
        "remember" => &["kind", "text", "about", "links"],
        "add_task" => &["title", "project", "links"],
        "recall" => &["query"],
        "open_session" | "play_session" => &["id"],
        "show_tasks" => &["project"],
        // `show_library`, `show_memory`, and names already refused as unknown.
        _ => &[],
    }
}

/// What a tool produced, structured so the core can use it (a code card needs the lines) and
/// render it for the talker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "output", rename_all = "snake_case")]
pub enum ToolOutput {
    /// Lines `[first_line, first_line + lines.len())` of a file, 1-based. `more_lines` counts
    /// lines cut by the result limit.
    File {
        path: String,
        first_line: u32,
        lines: Vec<String>,
        more_lines: u32,
    },
    Matches {
        hits: Vec<SearchHit>,
        more_hits: u32,
    },
    Listing {
        entries: Vec<DirEntry>,
        more_entries: u32,
    },
    /// A harness task started; its result arrives later as harness events.
    Started {
        task: TaskId,
    },
    /// Recall results, a stored memory's confirmation, and other plain answers.
    Text {
        text: String,
    },
    Failed {
        kind: ToolFailure,
        message: String,
    },
    /// A view read from the store, for the core to show and describe to the talker. `today`
    /// (`YYYY-MM-DD`, UTC) is the day it was read. Boxed: views are large, inputs are not.
    View {
        view: Box<View>,
        today: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailure {
    /// The path is outside the opened folders, or no repo is open.
    OutsideRepo,
    NotFound,
    /// The harness queue is full, or the tool is unavailable (no harness installed).
    Unavailable,
    Io,
    InvalidInput,
}

#[cfg(test)]
mod tests {
    use super::{InvalidToolInput, RememberKind, ToolCall};
    use crate::memory::Subject;

    #[test]
    fn parses_each_declared_tool() {
        let read = ToolCall::parse(
            "read_file",
            r#"{"path":"src/lb.rs","start_line":4,"end_line":9}"#,
        );
        assert_eq!(
            read,
            Ok(ToolCall::ReadFile {
                path: "src/lb.rs".into(),
                start_line: Some(4),
                end_line: Some(9)
            })
        );
        let remember = ToolCall::parse(
            "remember",
            r#"{"kind":"stated","text":"we use Kafka","about":"systems"}"#,
        );
        assert_eq!(
            remember,
            Ok(ToolCall::Remember {
                kind: RememberKind::Stated,
                text: "we use Kafka".into(),
                about: Subject::Systems,
                links: Vec::new()
            })
        );
        assert_eq!(
            ToolCall::parse("show_library", "{}"),
            Ok(ToolCall::ShowLibrary)
        );
        assert_eq!(
            ToolCall::parse("show_tasks", r#"{"project":"harness"}"#),
            Ok(ToolCall::ShowTasks {
                project: Some("harness".into())
            })
        );
        assert_eq!(
            ToolCall::parse("show_tasks", "{}"),
            Ok(ToolCall::ShowTasks { project: None })
        );
    }

    #[test]
    fn rejects_bad_input_at_the_boundary() {
        assert!(matches!(
            ToolCall::parse("rm_rf", "{}"),
            Err(InvalidToolInput::UnknownTool { .. })
        ));
        for (name, input) in [
            ("read_file", r#"{"path":""}"#),
            ("read_file", r#"{"path":"a","start_line":9,"end_line":4}"#),
            ("read_file", r#"{"path":"a","extra":1}"#),
            ("read_file", "[]"),
            ("read_file", "{"),
            ("research", r#"{"question":"why","depth":"forever"}"#),
            ("remember", r#"{"kind":"stated","text":"x"}"#),
            ("remember", r#"{"kind":"stated","text":"x","about":"them"}"#),
            ("show_library", r#"{"all":true}"#),
            ("play_session", r#"{"id":"  "}"#),
        ] {
            assert!(
                matches!(
                    ToolCall::parse(name, input),
                    Err(InvalidToolInput::Malformed { .. })
                ),
                "{name} {input}"
            );
        }
        let huge = format!(r#"{{"query":"{}"}}"#, "x".repeat(70_000));
        assert!(matches!(
            ToolCall::parse("recall", &huge),
            Err(InvalidToolInput::TooLarge { .. })
        ));
    }
}
