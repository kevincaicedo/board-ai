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
use crate::llm::openrouter::ProviderSlug;
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
    /// The provider a route asked for first; kept across retries.
    expected: Option<ProviderSlug>,
    /// OpenRouter named who served the response.
    served_by_seen: bool,
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
    /// OpenRouter: who served the response (`Cerebras`). Read loosely: other servers may use
    /// the name for something else, and it never fails a chunk.
    #[serde(default)]
    provider: Option<serde_json::Value>,
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
    /// OpenRouter: how much of the prompt came from the provider's cache.
    #[serde(default)]
    prompt_tokens_details: Option<PromptDetails>,
}

#[derive(Deserialize)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// OpenRouter sends an HTTP status (`502`) or a name (`server_error`) here instead of a
    /// `type`.
    #[serde(default)]
    code: Option<serde_json::Value>,
}

impl WireError {
    /// The error in its category, so an upstream failure before the first token is retried
    /// like the same failure as an HTTP status.
    fn into_provider_error(self) -> ProviderError {
        let detail = bounded_detail(&self.message);
        if let Some(kind) = self.kind.as_deref() {
            return ProviderError::from_error_type(kind, detail);
        }
        let status = self
            .code
            .as_ref()
            .and_then(serde_json::Value::as_u64)
            .and_then(|code| u16::try_from(code).ok());
        match (
            status,
            self.code.as_ref().and_then(serde_json::Value::as_str),
        ) {
            (Some(status), _) => ProviderError::from_status(status, None, detail),
            (None, Some(name)) => ProviderError::from_error_type(name, detail),
            (None, None) => ProviderError::from_error_type("provider_error", detail),
        }
    }
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
            return Err(error.into_provider_error());
        }
        if let Some(name) = chunk.provider.as_ref().and_then(serde_json::Value::as_str) {
            self.note_served_by(name);
        }
        if let Some(usage) = chunk.usage {
            let cached = usage
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0);
            // `input_tokens` is the uncached remainder, as Anthropic reports it.
            self.usage.input_tokens = usage.prompt_tokens.saturating_sub(cached);
            self.usage.cache_read_input_tokens = cached;
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
        *self = Self::new(self.expected.take());
    }
}

impl OpenAiDecoder {
    pub(super) fn new(expected: Option<ProviderSlug>) -> Self {
        Self {
            expected,
            ..Self::default()
        }
    }

    /// An answer from a later provider in the route is worth a warning: its latency is not
    /// what the route's first provider was chosen for.
    fn note_served_by(&mut self, name: &str) {
        if self.served_by_seen {
            return;
        }
        self.served_by_seen = true;
        let name = bounded_detail(name);
        match &self.expected {
            Some(expected) if !expected.names(&name) => tracing::warn!(
                served_by = %name,
                route = expected.as_str(),
                "OpenRouter served the response from another provider"
            ),
            Some(_) | None => tracing::debug!(served_by = %name, "OpenRouter provider"),
        }
    }

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
