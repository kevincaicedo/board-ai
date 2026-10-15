//! Decodes `chat.completion.chunk` events into chat events.
//!
//! Tool call arguments stream as fragments keyed by `index`; a call is complete only when the
//! response finishes, so calls are delivered then, after the text.

use std::collections::VecDeque;

use boardai_core::history::{ProviderFamily, RawAssistant};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::{InvalidToolInput, ToolCall, ToolInvocation};
use serde::Deserialize;

use super::request::{assistant_message, tool_call};
use crate::llm::error::bounded_detail;
use crate::llm::session::{Decoder, Flow};
use crate::llm::sse::SseEvent;
use crate::llm::{ChatEvent, ProviderError, Usage};

#[derive(Debug, Default)]
pub(super) struct OpenAiDecoder {
    text: String,
    calls: Vec<PendingCall>,
    finish: Option<String>,
    refused: bool,
    usage: Usage,
}

#[derive(Debug)]
struct PendingCall {
    index: u32,
    id: String,
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<WireUsage>,
    #[serde(default)]
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Option<Delta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Unknown fields (Ollama's `reasoning`, OpenRouter's `reasoning_details`) are ignored:
/// reasoning is never spoken.
#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

impl Decoder for OpenAiDecoder {
    fn on_event(
        &mut self,
        event: SseEvent,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<Flow, ProviderError> {
        if event.data.trim() == "[DONE]" {
            self.complete(out)?;
            return Ok(Flow::Done);
        }
        let chunk: Chunk = serde_json::from_str(&event.data)
            .map_err(|error| bad(&format!("a chunk is not valid: {error}")))?;
        if let Some(error) = chunk.error {
            let kind = error.kind.as_deref().unwrap_or("provider_error");
            return Err(ProviderError::from_error_type(
                kind,
                bounded_detail(&error.message),
            ));
        }
        if let Some(usage) = chunk.usage {
            self.usage.input_tokens = usage.prompt_tokens;
            self.usage.output_tokens = usage.completion_tokens;
        }
        for choice in chunk.choices {
            if let Some(delta) = choice.delta {
                self.apply(delta, out)?;
            }
            if let Some(finish) = choice.finish_reason {
                self.finish = Some(finish);
            }
        }
        Ok(Flow::Continue)
    }

    /// Some servers close the stream without `[DONE]`; a stream that already said why it
    /// finished is complete.
    fn on_end_of_body(&mut self, out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        if self.finish.is_none() {
            return Err(ProviderError::Network {
                detail: "the stream ended before the response was complete".to_owned(),
            });
        }
        self.complete(out)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl OpenAiDecoder {
    fn apply(&mut self, delta: Delta, out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        for text in [delta.content, delta.refusal.clone()].into_iter().flatten() {
            if !text.is_empty() {
                self.text.push_str(&text);
                out.push_back(ChatEvent::TextDelta(text));
            }
        }
        if delta.refusal.is_some_and(|refusal| !refusal.is_empty()) {
            self.refused = true;
        }
        for fragment in delta.tool_calls.into_iter().flatten() {
            let position = self
                .calls
                .iter()
                .position(|call| call.index == fragment.index);
            let call = match position {
                Some(position) => &mut self.calls[position],
                None => {
                    self.calls.push(PendingCall {
                        index: fragment.index,
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });
                    let last = self.calls.len() - 1;
                    &mut self.calls[last]
                }
            };
            if let Some(id) = fragment.id {
                call.id = id;
            }
            if let Some(function) = fragment.function {
                call.name.push_str(&function.name.unwrap_or_default());
                call.arguments
                    .push_str(&function.arguments.unwrap_or_default());
            }
        }
        Ok(())
    }

    fn complete(&mut self, out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        let stop = self.stop_reason()?;
        let mut wire_calls = Vec::with_capacity(self.calls.len());
        for call in &self.calls {
            let id = ToolCallId::parse(&call.id)
                .ok_or_else(|| bad("a tool call has a missing or invalid id"))?;
            let arguments = if call.arguments.is_empty() {
                "{}".to_owned()
            } else {
                call.arguments.clone()
            };
            let parsed = match stop {
                StopReason::MaxTokens => Err(InvalidToolInput::Malformed {
                    reason: "the response hit its token limit, so this call was not run".into(),
                }),
                StopReason::Refusal => Err(InvalidToolInput::Malformed {
                    reason: "the response was refused, so this call was not run".into(),
                }),
                StopReason::EndTurn | StopReason::ToolUse | StopReason::Failed { .. } => {
                    ToolCall::parse(&call.name, &arguments)
                }
            };
            wire_calls.push(tool_call(id.as_str(), &call.name, &arguments));
            out.push_back(ChatEvent::ToolCall(ToolInvocation {
                id,
                name: call.name.clone(),
                input_json: arguments,
                call: parsed,
            }));
        }
        let raw = assistant_message(&self.text, wire_calls).map(|message| RawAssistant {
            provider: ProviderFamily::OpenAiCompatible,
            content_json: message.to_string(),
        });
        out.push_back(ChatEvent::Done {
            stop,
            usage: self.usage,
            raw,
        });
        Ok(())
    }

    fn stop_reason(&self) -> Result<StopReason, ProviderError> {
        if self.refused {
            return Ok(StopReason::Refusal);
        }
        match self.finish.as_deref() {
            // Some servers (older Ollama) report `stop` even after calling tools.
            Some("stop") if !self.calls.is_empty() => Ok(StopReason::ToolUse),
            Some("stop") => Ok(StopReason::EndTurn),
            Some("tool_calls" | "function_call") => Ok(StopReason::ToolUse),
            Some("length") => Ok(StopReason::MaxTokens),
            Some("content_filter") => Ok(StopReason::Refusal),
            Some(other) => Err(bad(&format!(
                "unexpected finish reason {}",
                bounded_detail(other)
            ))),
            None => Err(bad("the response ended without a finish reason")),
        }
    }
}

fn bad(detail: &str) -> ProviderError {
    ProviderError::BadOutput {
        detail: detail.to_owned(),
    }
}
