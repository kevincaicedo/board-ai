//! Renders the conversation as `/v1/chat/completions` messages.

use boardai_core::history::{AssistantPart, Message, ProviderFamily, RawAssistant, UserPart};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::Dialect;
use crate::llm::openrouter;
use crate::llm::{Effort, ProviderError, TalkerConfig};

#[derive(Clone, Debug)]
pub(super) struct FrozenRequest {
    fields: Map<String, Value>,
    system_prompt: String,
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(flatten)]
    fields: &'a Map<String, Value>,
    messages: Vec<Value>,
}

impl FrozenRequest {
    pub(super) fn new(config: &TalkerConfig, dialect: &Dialect) -> Self {
        let mut fields = Map::new();
        fields.insert("model".into(), json!(config.model));
        fields.insert("max_tokens".into(), json!(config.max_tokens));
        fields.insert("stream".into(), json!(true));
        // Without it the final usage chunk is never sent.
        fields.insert("stream_options".into(), json!({"include_usage": true}));
        match dialect {
            Dialect::Standard => {
                fields.insert(
                    "reasoning_effort".into(),
                    json!(reasoning_effort(config.effort)),
                );
            }
            Dialect::OpenRouter { route } => open_router_fields(config.effort, route, &mut fields),
        }
        if !config.tools.is_empty() {
            let tools: Vec<Value> = config
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        },
                    })
                })
                .collect();
            fields.insert("tools".into(), Value::Array(tools));
            fields.insert("tool_choice".into(), json!("auto"));
        }
        Self {
            fields,
            system_prompt: config.system_prompt.clone(),
        }
    }

    /// A warm-up request (D-154): the frozen fields, the system prompt and a one-word user
    /// turn, answered with one token and not streamed.
    pub(super) fn warm_up_body(&self) -> Result<Vec<u8>, ProviderError> {
        let mut fields = self.fields.clone();
        fields.insert("stream".into(), json!(false));
        // OpenAI-compatible servers refuse `stream_options` on a request that doesn't stream.
        fields.remove("stream_options");
        fields.insert("max_tokens".into(), json!(1));
        let mut messages = Vec::with_capacity(2);
        if !self.system_prompt.is_empty() {
            messages.push(json!({"role": "system", "content": self.system_prompt}));
        }
        messages.push(json!({"role": "user", "content": "Hi."}));
        let body = Body {
            fields: &fields,
            messages,
        };
        serde_json::to_vec(&body).map_err(|error| ProviderError::InvalidRequest {
            status: None,
            detail: error.to_string(),
        })
    }

    pub(super) fn body(&self, messages: &[Message]) -> Result<Vec<u8>, ProviderError> {
        let mut rendered = Vec::with_capacity(messages.len() + 1);
        if !self.system_prompt.is_empty() {
            rendered.push(json!({"role": "system", "content": self.system_prompt}));
        }
        let conversation = render_messages(messages)?;
        if conversation.is_empty() {
            return Err(invalid("the conversation is empty"));
        }
        if conversation
            .last()
            .and_then(|last| last.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
        {
            return Err(invalid(
                "the conversation ends with an assistant turn, and prefill is not supported",
            ));
        }
        rendered.extend(conversation);
        let body = Body {
            fields: &self.fields,
            messages: rendered,
        };
        serde_json::to_vec(&body).map_err(|error| ProviderError::InvalidRequest {
            status: None,
            detail: error.to_string(),
        })
    }
}

/// OpenRouter's documented fields (D-152): `reasoning.effort`, with the reasoning text left
/// out of the stream (it is never spoken, and never echoed back); and for a route,
/// `provider.order` with no fallbacks, so the route's providers are the only ones tried. A
/// provider outside it can be ten times slower to the first token (Near AI against Cerebras
/// on gpt-oss-120b, 2026-10-16), and a voice is better told the route is down than slowed
/// silently. `require_parameters` keeps a routed provider from ignoring `reasoning` (Qwen
/// would think at its default, `xhigh`) or `tools`.
fn open_router_fields(
    effort: Effort,
    route: &[openrouter::ProviderSlug],
    fields: &mut Map<String, Value>,
) {
    let effort = openrouter::reasoning_effort(effort);
    fields.insert(
        "reasoning".into(),
        json!({"effort": effort, "exclude": true}),
    );
    if !route.is_empty() {
        let order: Vec<&str> = route.iter().map(openrouter::ProviderSlug::as_str).collect();
        fields.insert(
            "provider".into(),
            json!({"order": order, "allow_fallbacks": false, "require_parameters": true}),
        );
    }
}

/// The standard levels; `xhigh` and `max` are Claude-only, so they ask for the most there is.
fn reasoning_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::None => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::ExtraHigh | Effort::Max => "high",
    }
}

/// System context stays a system message (these servers accept one anywhere); tool results
/// become `tool` messages, which must directly follow the assistant turn that called them.
fn render_messages(messages: &[Message]) -> Result<Vec<Value>, ProviderError> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            Message::System { text } if text.is_empty() => {}
            Message::System { text } => out.push(json!({"role": "system", "content": text})),
            Message::User { parts } => render_user(parts, &mut out),
            Message::Assistant { parts, raw } => {
                if let Some(assistant) = render_assistant(parts, raw.as_ref())? {
                    out.push(assistant);
                }
            }
        }
    }
    Ok(out)
}

fn render_user(parts: &[UserPart], out: &mut Vec<Value>) {
    let mut texts = Vec::new();
    for part in parts {
        match part {
            UserPart::ToolResult {
                id,
                content,
                is_error,
            } => {
                // The format has no error flag, so the result says so in words.
                let content = if *is_error {
                    format!("Error: {content}")
                } else {
                    content.clone()
                };
                out.push(json!({"role": "tool", "tool_call_id": id.as_str(), "content": content}));
            }
            UserPart::Text { text } if text.is_empty() => {}
            UserPart::Text { text } => texts.push(text.as_str()),
        }
    }
    if !texts.is_empty() {
        out.push(json!({"role": "user", "content": texts.join("\n\n")}));
    }
}

fn render_assistant(
    parts: &[AssistantPart],
    raw: Option<&RawAssistant>,
) -> Result<Option<Value>, ProviderError> {
    if let Some(raw) = raw
        && raw.provider == ProviderFamily::OpenAiCompatible
    {
        return match serde_json::from_str(&raw.content_json) {
            Ok(Value::Object(message)) => Ok(Some(Value::Object(message))),
            Ok(_) | Err(_) => Err(invalid("a stored assistant turn is not a message object")),
        };
    }
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    for part in parts {
        match part {
            AssistantPart::Text { text } => texts.push(text.as_str()),
            AssistantPart::ToolUse {
                id,
                name,
                input_json,
            } => calls.push(tool_call(id.as_str(), name, input_json)),
        }
    }
    Ok(assistant_message(&texts.concat(), calls))
}

pub(super) fn tool_call(id: &str, name: &str, arguments: &str) -> Value {
    json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}})
}

/// An assistant message, or `None` when it would be empty.
pub(super) fn assistant_message(text: &str, calls: Vec<Value>) -> Option<Value> {
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let content = if text.is_empty() {
        Value::Null
    } else {
        json!(text)
    };
    let mut message = json!({"role": "assistant", "content": content});
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    Some(message)
}

fn invalid(detail: &str) -> ProviderError {
    ProviderError::InvalidRequest {
        status: None,
        detail: detail.to_owned(),
    }
}
