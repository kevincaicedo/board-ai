//! Decodes the Messages API event stream into chat events, and keeps the assistant content
//! as returned so it can be echoed back unchanged.
//!
//! Tool calls are held until `message_stop`: only then is the stop reason known, and a call in
//! a turn that stopped on `max_tokens` or `refusal` must not run. Holding them costs nothing,
//! since a tool call is the last block of its turn.

use std::collections::VecDeque;

use boardai_core::history::{ProviderFamily, RawAssistant};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::{InvalidToolInput, ToolCall, ToolInvocation};
use serde::Deserialize;
use serde_json::{Value, json};

use super::request::input_object;
use crate::llm::error::bounded_detail;
use crate::llm::session::{Decoder, Flow};
use crate::llm::sse::SseEvent;
use crate::llm::{ChatEvent, ProviderError, Usage};

#[derive(Debug, Default)]
pub(super) struct AnthropicDecoder {
    blocks: Vec<Block>,
    stop: Option<String>,
    usage: Usage,
}

#[derive(Debug)]
enum Block {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        /// Streamed `partial_json`, concatenated.
        input: String,
        /// The `input` of `content_block_start`, used when no fragment streamed.
        initial_input: Value,
        closed: bool,
    },
    /// A server-side fallback switch point; content before the last one is echoed back
    /// without its model-internal blocks.
    Fallback(Value),
    /// `redacted_thinking` and block types we don't know: kept as they arrived.
    Opaque(Value),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    MessageStart {
        message: WireMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: Value,
    },
    ContentBlockDelta {
        index: usize,
        delta: WireDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: WireMessageDelta,
        #[serde(default)]
        usage: Option<WireUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: WireError,
    },
    /// New event types may be added to the API; they are skipped.
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireMessageDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: String,
}

impl Decoder for AnthropicDecoder {
    fn on_event(
        &mut self,
        event: SseEvent,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<Flow, ProviderError> {
        let wire: WireEvent = serde_json::from_str(&event.data)
            .map_err(|error| bad(&format!("an event is not valid: {error}")))?;
        match wire {
            WireEvent::MessageStart { message } => {
                self.merge_usage(message.usage);
            }
            WireEvent::ContentBlockStart {
                index,
                content_block,
            } => self.start_block(index, content_block, out)?,
            WireEvent::ContentBlockDelta { index, delta } => self.apply_delta(index, delta, out)?,
            WireEvent::ContentBlockStop { index } => self.stop_block(index)?,
            WireEvent::MessageDelta { delta, usage } => {
                if let Some(stop) = delta.stop_reason {
                    self.stop = Some(stop);
                }
                self.merge_usage(usage);
            }
            WireEvent::MessageStop => {
                self.complete(out)?;
                return Ok(Flow::Done);
            }
            WireEvent::Ping | WireEvent::Unknown => {}
            WireEvent::Error { error } => {
                return Err(ProviderError::from_error_type(
                    &error.kind,
                    bounded_detail(&error.message),
                ));
            }
        }
        Ok(Flow::Continue)
    }

    fn on_end_of_body(&mut self, _out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        Err(ProviderError::Network {
            detail: "the stream ended before the response was complete".to_owned(),
        })
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl AnthropicDecoder {
    fn merge_usage(&mut self, usage: Option<WireUsage>) {
        let Some(usage) = usage else { return };
        let fields = [
            (&mut self.usage.input_tokens, usage.input_tokens),
            (&mut self.usage.output_tokens, usage.output_tokens),
            (
                &mut self.usage.cache_creation_input_tokens,
                usage.cache_creation_input_tokens,
            ),
            (
                &mut self.usage.cache_read_input_tokens,
                usage.cache_read_input_tokens,
            ),
        ];
        for (field, value) in fields {
            if let Some(value) = value {
                *field = value;
            }
        }
    }

    fn start_block(
        &mut self,
        index: usize,
        content_block: Value,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<(), ProviderError> {
        if index != self.blocks.len() {
            return Err(bad(&format!("content block {index} started out of order")));
        }
        let text_field = |name: &str| {
            content_block
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let block = match content_block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = text_field("text");
                if !text.is_empty() {
                    out.push_back(ChatEvent::TextDelta(text.clone()));
                }
                Block::Text(text)
            }
            Some("thinking") => Block::Thinking {
                thinking: text_field("thinking"),
                signature: text_field("signature"),
            },
            Some("tool_use") => {
                let id = text_field("id");
                let name = text_field("name");
                if id.is_empty() || name.is_empty() {
                    return Err(bad("a tool_use block has no id or name"));
                }
                let initial_input = content_block.get("input").cloned().unwrap_or(json!({}));
                Block::ToolUse {
                    id,
                    name,
                    input: String::new(),
                    initial_input,
                    closed: false,
                }
            }
            Some("fallback") => Block::Fallback(content_block),
            Some(_) | None => Block::Opaque(content_block),
        };
        self.blocks.push(block);
        Ok(())
    }

    fn apply_delta(
        &mut self,
        index: usize,
        delta: WireDelta,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<(), ProviderError> {
        let Some(block) = self.blocks.get_mut(index) else {
            return Err(bad(&format!(
                "a delta for block {index}, which never started"
            )));
        };
        let mismatch = || bad(&format!("a delta that does not fit block {index}"));
        match delta {
            WireDelta::TextDelta { text } => {
                let Block::Text(content) = block else {
                    return Err(mismatch());
                };
                content.push_str(&text);
                if !text.is_empty() {
                    out.push_back(ChatEvent::TextDelta(text));
                }
            }
            WireDelta::InputJsonDelta { partial_json } => {
                let Block::ToolUse {
                    input,
                    closed: false,
                    ..
                } = block
                else {
                    return Err(mismatch());
                };
                input.push_str(&partial_json);
            }
            WireDelta::ThinkingDelta { thinking: fragment } => {
                let Block::Thinking { thinking, .. } = block else {
                    return Err(mismatch());
                };
                thinking.push_str(&fragment);
            }
            WireDelta::SignatureDelta {
                signature: fragment,
            } => {
                let Block::Thinking { signature, .. } = block else {
                    return Err(mismatch());
                };
                signature.push_str(&fragment);
            }
            WireDelta::Unknown => {}
        }
        Ok(())
    }

    fn stop_block(&mut self, index: usize) -> Result<(), ProviderError> {
        let Some(block) = self.blocks.get_mut(index) else {
            return Err(bad(&format!("block {index} stopped but never started")));
        };
        if let Block::ToolUse { closed, .. } = block {
            *closed = true;
        }
        Ok(())
    }

    fn complete(&self, out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError> {
        let stop = map_stop(self.stop.as_deref())?;
        let boundary = self
            .blocks
            .iter()
            .rposition(|block| matches!(block, Block::Fallback(_)));
        let before_boundary = |index: usize| boundary.is_some_and(|last| index < last);
        for (index, block) in self.blocks.iter().enumerate() {
            if before_boundary(index) {
                continue;
            }
            if let Some(invocation) = invocation(block, &stop)? {
                out.push_back(ChatEvent::ToolCall(invocation));
            }
        }
        let content: Vec<Value> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| raw_block(block, before_boundary(index)))
            .collect();
        let content_json = serde_json::to_string(&content)
            .map_err(|error| bad(&format!("the content cannot be kept: {error}")))?;
        out.push_back(ChatEvent::Done {
            stop,
            usage: self.usage,
            raw: Some(RawAssistant {
                provider: ProviderFamily::Anthropic,
                content_json,
            }),
        });
        Ok(())
    }
}

fn invocation(block: &Block, stop: &StopReason) -> Result<Option<ToolInvocation>, ProviderError> {
    let Block::ToolUse {
        id,
        name,
        input,
        initial_input,
        closed,
    } = block
    else {
        return Ok(None);
    };
    let id = ToolCallId::parse(id).ok_or_else(|| bad("a tool call has an invalid id"))?;
    let input_json = if input.is_empty() {
        initial_input.to_string()
    } else {
        input.clone()
    };
    let not_run = |reason: &str| {
        Err(InvalidToolInput::Malformed {
            reason: reason.to_owned(),
        })
    };
    let call = match stop {
        StopReason::MaxTokens => {
            not_run("the response hit its token limit, so this call was not run")
        }
        StopReason::Refusal => not_run("the response was refused, so this call was not run"),
        StopReason::EndTurn | StopReason::ToolUse | StopReason::Failed { .. } if !closed => {
            not_run("the tool input was cut off before it was complete")
        }
        StopReason::EndTurn | StopReason::ToolUse | StopReason::Failed { .. } => {
            ToolCall::parse(name, &input_json)
        }
    };
    Ok(Some(ToolInvocation {
        id,
        name: name.clone(),
        input_json,
        call,
    }))
}

/// The block as the API returned it. Before the last fallback switch only text survives, as
/// the fallback docs require; empty text blocks are dropped because the API rejects them.
fn raw_block(block: &Block, before_boundary: bool) -> Option<Value> {
    match block {
        Block::Text(text) if text.is_empty() => None,
        Block::Text(text) => Some(json!({"type": "text", "text": text})),
        Block::Fallback(value) => Some(value.clone()),
        Block::Thinking { .. } | Block::ToolUse { .. } | Block::Opaque(_) if before_boundary => {
            None
        }
        Block::Thinking {
            thinking,
            signature,
        } => Some(json!({"type": "thinking", "thinking": thinking, "signature": signature})),
        Block::ToolUse {
            id,
            name,
            input,
            initial_input,
            ..
        } => {
            let input = if input.is_empty() {
                initial_input.clone()
            } else {
                input_object(input)
            };
            Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
        }
        Block::Opaque(value) => Some(value.clone()),
    }
}

fn map_stop(stop: Option<&str>) -> Result<StopReason, ProviderError> {
    match stop {
        Some("end_turn" | "stop_sequence") => Ok(StopReason::EndTurn),
        Some("tool_use") => Ok(StopReason::ToolUse),
        Some("max_tokens" | "model_context_window_exceeded") => Ok(StopReason::MaxTokens),
        Some("refusal") => Ok(StopReason::Refusal),
        // `pause_turn` only comes with server tools, which we never declare.
        Some(other) => Err(bad(&format!(
            "unexpected stop reason {}",
            bounded_detail(other)
        ))),
        None => Err(bad("the response ended without a stop reason")),
    }
}

fn bad(detail: &str) -> ProviderError {
    ProviderError::BadOutput {
        detail: detail.to_owned(),
    }
}
