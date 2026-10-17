//! Decodes `/api/chat` stream lines into chat events.
//!
//! Each line carries a piece of the assistant message; tool calls arrive whole (arguments as a
//! JSON object) and are delivered when the response is done, after the text. The last line says
//! `done` with a reason and the token counts. `thinking` is never spoken and is ignored. When
//! the request offered tools, calls gemma4 wrote in its own syntax as text are read back into
//! calls and taken out of the text ([`NativeCalls`], D-191); they are delivered with the others,
//! in the order written, and stored in the native message as calls.

use std::collections::VecDeque;
use std::sync::Arc;

use boardai_core::history::{ProviderFamily, RawAssistant};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::{InvalidToolInput, ToolCall, ToolInvocation};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::native_calls::{NativeCall, NativeCalls};
use super::request::{assistant_message, tool_call};
use crate::llm::error::bounded_detail;
use crate::llm::session::{Decoder, Flow};
use crate::llm::sse::SseEvent;
use crate::llm::{ChatEvent, ProviderError, Usage};

#[derive(Debug)]
pub(super) struct OllamaDecoder {
    text: String,
    calls: Vec<WireToolCall>,
    /// Reads gemma4's calls written as text; only when the request offered tools.
    native: Option<NativeCalls>,
}

impl OllamaDecoder {
    /// `tools` are the names of the request's tools.
    pub(super) fn new(tools: &Arc<[String]>) -> Self {
        Self {
            text: String::new(),
            calls: Vec::new(),
            native: (!tools.is_empty()).then(|| NativeCalls::new(Arc::clone(tools))),
        }
    }
}

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    message: Option<WireMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: u64,
    #[serde(default)]
    eval_count: u64,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
    #[serde(default)]
    id: Option<String>,
    function: WireFunction,
}

#[derive(Debug, Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: Value,
}

impl Decoder for OllamaDecoder {
    fn on_event(
        &mut self,
        event: SseEvent,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<Flow, ProviderError> {
        let line: Line = serde_json::from_str(&event.data)
            .map_err(|error| bad(&format!("a line is not valid: {error}")))?;
        // An error inside a 200 stream comes from the server (a model that crashed, ran out of
        // memory, or was unloaded).
        if let Some(error) = line.error {
            return Err(ProviderError::Server {
                status: 500,
                detail: bounded_detail(&error),
            });
        }
        if let Some(message) = line.message {
            self.on_content(message.content, out);
            self.calls.extend(message.tool_calls);
        }
        if !line.done {
            return Ok(Flow::Continue);
        }
        self.finish_native(out);
        let usage = Usage {
            input_tokens: line.prompt_eval_count,
            output_tokens: line.eval_count,
            ..Usage::default()
        };
        self.complete(line.done_reason.as_deref(), usage, out)?;
        Ok(Flow::Done)
    }

    /// Every response ends with a `done` line; a body that ends without one was cut.
    fn on_end_of_body(&mut self, _out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        Err(ProviderError::Network {
            detail: "the stream ended before the response was complete".to_owned(),
        })
    }

    fn reset(&mut self) {
        self.text.clear();
        self.calls.clear();
        if let Some(native) = &mut self.native {
            native.reset();
        }
    }
}

impl OllamaDecoder {
    /// Speaks what is text; a call written in gemma4's syntax becomes a call instead.
    fn on_content(&mut self, content: String, out: &mut VecDeque<ChatEvent>) {
        let text = match &mut self.native {
            Some(native) => {
                let mut text = String::new();
                let mut calls = Vec::new();
                native.push(&content, &mut text, &mut calls);
                self.calls.extend(calls.into_iter().map(WireToolCall::from));
                text
            }
            None => content,
        };
        self.speak(text, out);
    }

    /// The response is done: a call still open is decided, and what was read is logged.
    fn finish_native(&mut self, out: &mut VecDeque<ChatEvent>) {
        let Some(native) = &mut self.native else {
            return;
        };
        let mut text = String::new();
        native.finish(&mut text);
        let (read, dropped) = (native.read(), native.dropped());
        self.speak(text, out);
        if dropped > 0 {
            tracing::warn!(
                read,
                dropped,
                "tool calls written in gemma4's syntax were dropped"
            );
        } else if read > 0 {
            tracing::info!(
                read,
                "tool calls written in gemma4's syntax were read as calls"
            );
        }
    }

    fn speak(&mut self, text: String, out: &mut VecDeque<ChatEvent>) {
        if !text.is_empty() {
            self.text.push_str(&text);
            out.push_back(ChatEvent::TextDelta(text));
        }
    }

    fn complete(
        &mut self,
        reason: Option<&str>,
        usage: Usage,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<(), ProviderError> {
        let stop = match reason {
            // Ollama says `stop` after tool calls too.
            Some("stop") | None if !self.calls.is_empty() => StopReason::ToolUse,
            Some("stop") | None => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some(other) => {
                return Err(bad(&format!(
                    "unexpected done reason {}",
                    bounded_detail(other)
                )));
            }
        };
        let mut wire_calls = Vec::with_capacity(self.calls.len());
        for (index, call) in self.calls.iter().enumerate() {
            // Older Ollama sends no ids; results are matched by order and tool name there.
            let id = call
                .id
                .as_deref()
                .and_then(ToolCallId::parse)
                .or_else(|| ToolCallId::parse(&format!("call_{index}")))
                .ok_or_else(|| bad("a tool call id could not be made"))?;
            let arguments = if call.function.arguments.is_null() {
                Value::Object(Map::new())
            } else {
                call.function.arguments.clone()
            };
            let input_json = arguments.to_string();
            let parsed = match stop {
                StopReason::MaxTokens => Err(InvalidToolInput::Malformed {
                    reason: "the response hit its token limit, so this call was not run".into(),
                }),
                StopReason::EndTurn
                | StopReason::ToolUse
                | StopReason::Refusal
                | StopReason::Failed { .. } => ToolCall::parse(&call.function.name, &input_json),
            };
            wire_calls.push(tool_call(id.as_str(), &call.function.name, arguments));
            out.push_back(ChatEvent::ToolCall(ToolInvocation {
                id,
                name: call.function.name.clone(),
                input_json,
                call: parsed,
            }));
        }
        let raw = assistant_message(&self.text, wire_calls).map(|message| RawAssistant {
            provider: ProviderFamily::Ollama,
            content_json: message.to_string(),
        });
        out.push_back(ChatEvent::Done { stop, usage, raw });
        Ok(())
    }
}

/// A call read from the text has no id; it gets one by its place, as Ollama's own calls
/// without one do.
impl From<NativeCall> for WireToolCall {
    fn from(call: NativeCall) -> Self {
        Self {
            id: None,
            function: WireFunction {
                name: call.name,
                arguments: Value::Object(call.arguments),
            },
        }
    }
}

fn bad(detail: &str) -> ProviderError {
    ProviderError::BadOutput {
        detail: detail.to_owned(),
    }
}
