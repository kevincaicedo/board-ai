//! A scripted talker, scripted tools and a scripted coding harness for the simulator.

use std::collections::VecDeque;

use crate::effect::TalkRequest;
use crate::harness::{HarnessEvent, HarnessTask};
use crate::history::{ProviderFamily, RawAssistant};
use crate::ids::ToolCallId;
use crate::input::{Input, StopReason};
use crate::tools::{ToolCall, ToolInvocation, ToolOutput};

/// One scripted talker response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptedReply {
    pub text: String,
    /// `(tool name, input JSON)`, sent after the text.
    pub tool_calls: Vec<(String, String)>,
    pub stop: StopReason,
}

impl ScriptedReply {
    /// Narration that ends the turn.
    #[must_use]
    pub fn says(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            tool_calls: Vec::new(),
            stop: StopReason::EndTurn,
        }
    }

    /// Adds a tool call; the reply then stops with `ToolUse`.
    #[must_use]
    pub fn calls(mut self, name: &str, input_json: &str) -> Self {
        self.tool_calls
            .push((name.to_owned(), input_json.to_owned()));
        self.stop = StopReason::ToolUse;
        self
    }

    #[must_use]
    pub fn stops(mut self, stop: StopReason) -> Self {
        self.stop = stop;
        self
    }
}

/// Streams scripted replies, one per `Talk`. Inputs carry the stream's generation so the
/// simulator drops those of a cancelled stream, as the runtime's talker actor does.
#[derive(Clone, Debug)]
pub struct FakeTalker {
    replies: VecDeque<ScriptedReply>,
    pub first_token_ms: u64,
    pub chunk_bytes: usize,
    pub chunk_interval_ms: u64,
    generation: u64,
    next_call: u32,
    /// Every request the core sent, in order.
    pub requests: Vec<TalkRequest>,
    /// Every scripted text, for checking that committed text was really said.
    pub scripts: Vec<String>,
}

impl Default for FakeTalker {
    fn default() -> Self {
        Self {
            replies: VecDeque::new(),
            first_token_ms: 300,
            chunk_bytes: 16,
            chunk_interval_ms: 30,
            generation: 0,
            next_call: 0,
            requests: Vec::new(),
            scripts: Vec::new(),
        }
    }
}

impl FakeTalker {
    pub fn push(&mut self, reply: ScriptedReply) {
        self.scripts.push(reply.text.clone());
        self.replies.push_back(reply);
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cancel(&mut self) {
        self.generation += 1;
    }

    /// A `Talk`: returns the stream's inputs with their delays from now.
    pub fn talk(&mut self, request: TalkRequest) -> Vec<(u64, Input)> {
        self.requests.push(request);
        self.generation += 1;
        let reply = self.replies.pop_front().unwrap_or_else(|| {
            let fallback = ScriptedReply::says("Okay.");
            self.scripts.push(fallback.text.clone());
            fallback
        });
        let mut inputs = Vec::new();
        let mut at = self.first_token_ms;
        let mut rest = reply.text.as_str();
        while !rest.is_empty() {
            let mut cut = self.chunk_bytes.min(rest.len());
            while !rest.is_char_boundary(cut) {
                cut += 1;
            }
            inputs.push((
                at,
                Input::TalkerDelta {
                    text: rest[..cut].to_owned(),
                },
            ));
            rest = &rest[cut..];
            at += self.chunk_interval_ms;
        }
        for (name, input_json) in &reply.tool_calls {
            let id = ToolCallId::parse(&format!("toolu_{}", self.next_call));
            self.next_call += 1;
            if let Some(id) = id {
                let invocation = ToolInvocation {
                    id,
                    name: name.clone(),
                    input_json: input_json.clone(),
                    call: ToolCall::parse(name, input_json),
                };
                inputs.push((at, Input::TalkerToolCall { invocation }));
                at += self.chunk_interval_ms;
            }
        }
        let raw = match reply.stop {
            StopReason::Failed { .. } => None,
            StopReason::EndTurn
            | StopReason::ToolUse
            | StopReason::MaxTokens
            | StopReason::Refusal => Some(RawAssistant {
                provider: ProviderFamily::Anthropic,
                content_json: serde_json::json!([{ "type": "text", "text": reply.text }])
                    .to_string(),
            }),
        };
        inputs.push((
            at,
            Input::TalkerDone {
                stop: reply.stop,
                raw,
            },
        ));
        inputs
    }
}

/// Answers tool calls: a scripted output when one is queued for the tool, else a default.
#[derive(Clone, Debug, Default)]
pub struct FakeTools {
    scripted: Vec<(String, ToolOutput)>,
}

impl FakeTools {
    pub fn respond(&mut self, tool: &str, output: ToolOutput) {
        self.scripted.push((tool.to_owned(), output));
    }

    pub fn answer(&mut self, call: &ToolCall) -> ToolOutput {
        let name = tool_name(call);
        if let Some(index) = self.scripted.iter().position(|(tool, _)| tool == name) {
            return self.scripted.remove(index).1;
        }
        match call {
            ToolCall::ReadFile {
                path,
                start_line,
                end_line,
            } => {
                let first = start_line.unwrap_or(1);
                let last = end_line.unwrap_or(first + 19);
                ToolOutput::File {
                    path: path.clone(),
                    first_line: first,
                    lines: (first..=last)
                        .map(|line| format!("// {path} line {line}"))
                        .collect(),
                    more_lines: 0,
                }
            }
            ToolCall::SearchCode { .. } => ToolOutput::Matches {
                hits: Vec::new(),
                more_hits: 0,
            },
            ToolCall::ListDir { .. } => ToolOutput::Listing {
                entries: Vec::new(),
                more_entries: 0,
            },
            ToolCall::Research { .. }
            | ToolCall::Review { .. }
            | ToolCall::ProposeChange { .. }
            | ToolCall::Remember { .. }
            | ToolCall::AddTask { .. }
            | ToolCall::Recall { .. }
            | ToolCall::OpenSession { .. } => ToolOutput::Text {
                text: "Nothing found.".to_owned(),
            },
        }
    }
}

fn tool_name(call: &ToolCall) -> &'static str {
    match call {
        ToolCall::ReadFile { .. } => "read_file",
        ToolCall::SearchCode { .. } => "search_code",
        ToolCall::ListDir { .. } => "list_dir",
        ToolCall::Research { .. } => "research",
        ToolCall::Review { .. } => "review",
        ToolCall::ProposeChange { .. } => "propose_change",
        ToolCall::Remember { .. } => "remember",
        ToolCall::AddTask { .. } => "add_task",
        ToolCall::Recall { .. } => "recall",
        ToolCall::OpenSession { .. } => "open_session",
    }
}

/// A coding harness that finishes every task after `run_ms` with a fixed summary.
#[derive(Clone, Debug)]
pub struct FakeHarness {
    pub run_ms: u64,
    pub summary: String,
    pub started: Vec<HarnessTask>,
}

impl Default for FakeHarness {
    fn default() -> Self {
        Self {
            run_ms: 2_000,
            summary: "The cache is checked before the database.".to_owned(),
            started: Vec::new(),
        }
    }
}

impl FakeHarness {
    pub fn run(&mut self, spec: HarnessTask) -> (u64, HarnessEvent) {
        self.started.push(spec);
        (
            self.run_ms,
            HarnessEvent::Finished {
                summary: self.summary.clone(),
                citations: vec!["internal/user/service.go:42".to_owned()],
            },
        )
    }
}
