//! Renders the conversation as a native `/api/chat` request.

use std::collections::HashMap;

use boardai_core::history::{AssistantPart, Message, ProviderFamily, RawAssistant, UserPart};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::limits::{OLLAMA_CONTEXT_TOKENS, OLLAMA_KEEP_ALIVE_SECONDS};
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
    pub(super) fn new(config: &TalkerConfig) -> Self {
        let mut fields = Map::new();
        fields.insert("model".into(), json!(config.model));
        fields.insert("stream".into(), json!(true));
        fields.insert("keep_alive".into(), json!(OLLAMA_KEEP_ALIVE_SECONDS));
        fields.insert(
            "options".into(),
            json!({"num_ctx": OLLAMA_CONTEXT_TOKENS, "num_predict": config.max_tokens}),
        );
        // Effort `None` turns thinking off. Other efforts leave the model's default: asking a
        // model that cannot think to think is an error, and not every local model takes levels.
        if config.effort == Effort::None {
            fields.insert("think".into(), json!(false));
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
        }
        Self {
            fields,
            system_prompt: config.system_prompt.clone(),
        }
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

/// System context stays a system message. A tool result names its tool (`tool_name`), which
/// Ollama matches results by, and carries the call's id as well.
fn render_messages(messages: &[Message]) -> Result<Vec<Value>, ProviderError> {
    let mut out = Vec::with_capacity(messages.len());
    let mut tool_names: HashMap<String, String> = HashMap::new();
    for message in messages {
        match message {
            Message::System { text } if text.is_empty() => {}
            Message::System { text } => out.push(json!({"role": "system", "content": text})),
            Message::User { parts } => render_user(parts, &tool_names, &mut out),
            Message::Assistant { parts, raw } => {
                for part in parts {
                    if let AssistantPart::ToolUse { id, name, .. } = part {
                        tool_names.insert(id.as_str().to_owned(), name.clone());
                    }
                }
                if let Some(assistant) = render_assistant(parts, raw.as_ref())? {
                    out.push(assistant);
                }
            }
        }
    }
    Ok(out)
}

fn render_user(parts: &[UserPart], tool_names: &HashMap<String, String>, out: &mut Vec<Value>) {
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
                let name = tool_names.get(id.as_str()).map_or("", String::as_str);
                out.push(json!({
                    "role": "tool",
                    "content": content,
                    "tool_name": name,
                    "tool_call_id": id.as_str(),
                }));
            }
            UserPart::Text { text } if text.is_empty() => {}
            UserPart::Text { text } => texts.push(text.as_str()),
        }
    }
    if !texts.is_empty() {
        out.push(json!({"role": "user", "content": texts.join("\n\n")}));
    }
}

/// The stored native message when Ollama wrote it; otherwise rebuilt from the parts (a heard
/// prefix, or a turn from another provider).
fn render_assistant(
    parts: &[AssistantPart],
    raw: Option<&RawAssistant>,
) -> Result<Option<Value>, ProviderError> {
    if let Some(raw) = raw
        && raw.provider == ProviderFamily::Ollama
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
            } => calls.push(tool_call(id.as_str(), name, arguments(input_json))),
        }
    }
    Ok(assistant_message(&texts.concat(), calls))
}

/// Ollama takes arguments as an object. Input that is not one (a malformed call, answered with
/// an error) goes back empty.
fn arguments(input_json: &str) -> Value {
    match serde_json::from_str(input_json) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(_) | Err(_) => Value::Object(Map::new()),
    }
}

pub(super) fn tool_call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id": id, "function": {"name": name, "arguments": arguments}})
}

/// An assistant message, or `None` when it would be empty.
pub(super) fn assistant_message(text: &str, calls: Vec<Value>) -> Option<Value> {
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let mut message = json!({"role": "assistant", "content": text});
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
