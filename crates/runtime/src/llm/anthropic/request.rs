//! Renders the append-only conversation into a Messages API request body.
//!
//! The render is a pure function of the conversation, so the bytes of every earlier turn are
//! identical from one request to the next: that keeps the prompt cache warm and satisfies the
//! preserved-thinking prefix check (D-014).

use boardai_core::history::{AssistantPart, Message, ProviderFamily, RawAssistant, UserPart};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::models::ModelTraits;
use crate::llm::{CONTEXT_CLOSE, CONTEXT_OPEN, Effort, ProviderError, TalkerConfig};

/// Everything but `messages`: fixed for the session, rendered once.
#[derive(Clone, Debug)]
pub(super) struct FrozenRequest {
    fields: Map<String, Value>,
    mid_conversation_system: bool,
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(flatten)]
    fields: &'a Map<String, Value>,
    messages: Vec<Value>,
}

impl FrozenRequest {
    pub(super) fn new(config: &TalkerConfig, traits: ModelTraits) -> Self {
        let mut fields = Map::new();
        fields.insert("model".into(), json!(config.model));
        fields.insert("max_tokens".into(), json!(config.max_tokens));
        fields.insert("stream".into(), json!(true));
        if !config.system_prompt.is_empty() {
            // The breakpoint on the last system block caches tools and system together (tools
            // render first).
            fields.insert(
                "system".into(),
                json!([{
                    "type": "text",
                    "text": config.system_prompt,
                    "cache_control": {"type": "ephemeral"},
                }]),
            );
        }
        if !config.tools.is_empty() {
            let tools: Vec<Value> = config
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                        // Inputs stream as generated; the core validates them before use.
                        "eager_input_streaming": true,
                    })
                })
                .collect();
            fields.insert("tools".into(), Value::Array(tools));
            // Forced tool choice is a 400 on current models.
            fields.insert("tool_choice".into(), json!({"type": "auto"}));
        }
        if traits.adaptive_thinking {
            fields.insert("thinking".into(), json!({"type": "adaptive"}));
        }
        if traits.effort {
            fields.insert(
                "output_config".into(),
                json!({"effort": claude_effort(config.effort)}),
            );
        }
        // Automatic caching of the growing conversation tail, on top of the system breakpoint.
        fields.insert("cache_control".into(), json!({"type": "ephemeral"}));
        if traits.server_side_fallback {
            fields.insert("fallbacks".into(), json!("default"));
        }
        Self {
            fields,
            mid_conversation_system: traits.mid_conversation_system,
        }
    }

    pub(super) fn body(&self, messages: &[Message]) -> Result<Vec<u8>, ProviderError> {
        let body = Body {
            fields: &self.fields,
            messages: render_messages(messages, self.mid_conversation_system)?,
        };
        serde_json::to_vec(&body).map_err(|error| ProviderError::InvalidRequest {
            status: None,
            detail: error.to_string(),
        })
    }
}

/// Claude's levels; "none" becomes its lowest, since current models can't turn thinking off.
fn claude_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::None | Effort::Low => "low",
        Effort::Medium | Effort::High | Effort::ExtraHigh | Effort::Max => effort.as_str(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    System,
}

struct Rendered {
    role: Role,
    content: Vec<Value>,
}

/// Renders messages. A run of `Message::System` becomes one `role: "system"` message only where
/// the API allows it: right after a user turn, and either last or followed by an assistant
/// turn. Anywhere else (or on a model without the feature) its text is folded into the next
/// user turn as a block.
pub(super) fn render_messages(
    messages: &[Message],
    system_role: bool,
) -> Result<Vec<Value>, ProviderError> {
    let rendered = render_turns(messages, system_role)?;
    match rendered.last().map(|last| last.role) {
        None => Err(invalid("the conversation is empty")),
        Some(Role::Assistant) => Err(invalid(
            "the conversation ends with an assistant turn, and prefill is not supported",
        )),
        Some(Role::User | Role::System) => Ok(rendered.into_iter().map(into_value).collect()),
    }
}

/// The rendering itself, with no check on how the conversation ends.
fn render_turns(messages: &[Message], system_role: bool) -> Result<Vec<Rendered>, ProviderError> {
    let mut out: Vec<Rendered> = Vec::new();
    let mut folded: Vec<String> = Vec::new();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        match message {
            Message::System { .. } => {
                let end = system_run_end(messages, index);
                let texts = messages[index..end]
                    .iter()
                    .filter_map(system_text)
                    .filter(|text| !text.is_empty());
                let follows_user =
                    folded.is_empty() && out.last().is_some_and(|last| last.role == Role::User);
                let then_assistant_or_end =
                    matches!(messages.get(end), None | Some(Message::Assistant { .. }));
                if system_role && follows_user && then_assistant_or_end {
                    let content: Vec<Value> = texts.map(text_block).collect();
                    if !content.is_empty() {
                        out.push(Rendered {
                            role: Role::System,
                            content,
                        });
                    }
                } else {
                    folded.extend(texts.map(str::to_owned));
                }
                index = end;
            }
            Message::User { parts } => {
                push_user(&mut out, parts, std::mem::take(&mut folded));
                index += 1;
            }
            Message::Assistant { parts, raw } => {
                if !folded.is_empty() {
                    push_user(&mut out, &[], std::mem::take(&mut folded));
                }
                push_assistant(&mut out, parts, raw.as_ref())?;
                index += 1;
            }
        }
    }
    if !folded.is_empty() {
        push_user(&mut out, &[], folded);
    }
    Ok(out)
}

fn system_run_end(messages: &[Message], begin: usize) -> usize {
    let run = messages[begin..]
        .iter()
        .take_while(|message| matches!(message, Message::System { .. }))
        .count();
    begin + run
}

fn system_text(message: &Message) -> Option<&str> {
    match message {
        Message::System { text } => Some(text.as_str()),
        Message::User { .. } | Message::Assistant { .. } => None,
    }
}

/// Tool results come first: the API requires them ahead of any text in their user turn.
fn push_user(out: &mut Vec<Rendered>, parts: &[UserPart], context: Vec<String>) {
    let mut content = Vec::new();
    for part in parts {
        if let UserPart::ToolResult {
            id,
            content: text,
            is_error,
        } = part
        {
            let mut block = json!({"type": "tool_result", "tool_use_id": id.as_str()});
            if !text.is_empty() {
                block["content"] = json!(text);
            }
            if *is_error {
                block["is_error"] = json!(true);
            }
            content.push(block);
        }
    }
    content.extend(
        context
            .iter()
            .map(|text| text_block(&format!("{CONTEXT_OPEN}{text}{CONTEXT_CLOSE}"))),
    );
    for part in parts {
        if let UserPart::Text { text } = part
            && !text.is_empty()
        {
            content.push(text_block(text));
        }
    }
    push(out, Role::User, content);
}

/// A completed turn is echoed exactly as the provider returned it (thinking blocks included);
/// an interrupted turn, or one from another provider, is rebuilt from its parts.
fn push_assistant(
    out: &mut Vec<Rendered>,
    parts: &[AssistantPart],
    raw: Option<&RawAssistant>,
) -> Result<(), ProviderError> {
    let content = match raw {
        Some(raw) if raw.provider == ProviderFamily::Anthropic => raw_content(raw)?,
        Some(_) | None => rebuild(parts),
    };
    push(out, Role::Assistant, content);
    Ok(())
}

fn raw_content(raw: &RawAssistant) -> Result<Vec<Value>, ProviderError> {
    match serde_json::from_str(&raw.content_json) {
        Ok(Value::Array(blocks)) => Ok(blocks),
        Ok(_) | Err(_) => Err(invalid("a stored assistant turn is not a content array")),
    }
}

fn rebuild(parts: &[AssistantPart]) -> Vec<Value> {
    parts
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Text { text } if text.is_empty() => None,
            AssistantPart::Text { text } => Some(text_block(text)),
            AssistantPart::ToolUse {
                id,
                name,
                input_json,
            } => Some(json!({
                "type": "tool_use",
                "id": id.as_str(),
                "name": name,
                "input": input_object(input_json),
            })),
        })
        .collect()
}

/// A tool input as the object the API expects; input that never parsed (it was answered with
/// an error result) goes back as an empty object.
pub(super) fn input_object(input_json: &str) -> Value {
    match serde_json::from_str(input_json) {
        Ok(Value::Object(fields)) => Value::Object(fields),
        Ok(_) | Err(_) => json!({}),
    }
}

/// Consecutive turns of one role are merged: the API would merge them anyway, and merging here
/// keeps the rendered roles alternating.
fn push(out: &mut Vec<Rendered>, role: Role, content: Vec<Value>) {
    if content.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.role == role && role != Role::System => last.content.extend(content),
        Some(_) | None => out.push(Rendered { role, content }),
    }
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn into_value(rendered: Rendered) -> Value {
    let role = match rendered.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };
    json!({"role": role, "content": rendered.content})
}

fn invalid(detail: &str) -> ProviderError {
    ProviderError::InvalidRequest {
        status: None,
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::history::{AssistantPart, Message, ProviderFamily, RawAssistant, UserPart};
    use boardai_core::ids::ToolCallId;
    use serde_json::{Value, json};

    use super::{into_value, render_messages, render_turns};

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![UserPart::Text { text: text.into() }],
        }
    }

    fn system(text: &str) -> Message {
        Message::System { text: text.into() }
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            parts: vec![AssistantPart::Text { text: text.into() }],
            raw: Some(RawAssistant {
                provider: ProviderFamily::Anthropic,
                content_json: json!([{"type": "text", "text": text}]).to_string(),
            }),
        }
    }

    fn roles(rendered: &[Value]) -> Vec<&str> {
        rendered
            .iter()
            .map(|message| message["role"].as_str().unwrap_or_default())
            .collect()
    }

    #[test]
    fn context_after_a_user_turn_is_a_system_message() {
        let rendered = render_messages(&[user("hi"), system("board: lb")], true);
        assert_eq!(rendered.as_deref().map(roles), Ok(vec!["user", "system"]));
    }

    #[test]
    fn context_that_cannot_be_a_system_message_is_folded_into_a_user_turn() {
        // After an assistant turn (a research result arriving while idle).
        let Ok(rendered) = render_messages(&[user("hi"), assistant("hello"), system("done")], true)
        else {
            panic!("renders");
        };
        assert_eq!(roles(&rendered), ["user", "assistant", "user"]);
        assert_eq!(
            rendered[2]["content"][0]["text"],
            "<system-reminder>\ndone\n</system-reminder>"
        );
        // Followed by a user turn: folded ahead of the user's words.
        let Ok(rendered) = render_messages(&[user("a"), system("ctx"), user("b")], true) else {
            panic!("renders");
        };
        assert_eq!(roles(&rendered), ["user"]);
        assert_eq!(rendered[0]["content"].as_array().map(Vec::len), Some(3));
        // A model without the feature.
        let Ok(rendered) = render_messages(&[user("a"), system("ctx")], false) else {
            panic!("renders");
        };
        assert_eq!(roles(&rendered), ["user"]);
    }

    #[test]
    fn tool_results_lead_their_user_turn() {
        let id = ToolCallId::parse("toolu_9");
        let Some(id) = id else { panic!("id") };
        let messages = [
            user("a"),
            assistant("calling"),
            system("ctx"),
            Message::User {
                parts: vec![
                    UserPart::Text { text: "and".into() },
                    UserPart::ToolResult {
                        id,
                        content: String::new(),
                        is_error: true,
                    },
                ],
            },
        ];
        let Ok(rendered) = render_messages(&messages, true) else {
            panic!("renders");
        };
        let kinds: Vec<&str> = rendered[2]["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["type"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(kinds, ["tool_result", "text", "text"]);
        assert_eq!(rendered[2]["content"][0]["is_error"], true);
    }

    /// Every assistant turn is echoed after exactly the request that produced it, which is what
    /// preserved thinking checks; and appending never rewrites any message but the last.
    #[test]
    fn earlier_turns_render_identically_as_the_conversation_grows() {
        let growth = [
            user("q1"),
            system("board"),
            assistant("a1"),
            system("research result"),
            assistant("a2"),
            user("q2"),
            system("thread"),
            system("board again"),
            assistant("a3"),
            user("q3"),
        ];
        for system_role in [true, false] {
            for end in 1..growth.len() {
                let render = |messages: &[Message]| {
                    render_turns(messages, system_role)
                        .map(|turns| turns.into_iter().map(into_value).collect::<Vec<_>>())
                };
                let (Ok(before), Ok(after)) = (render(&growth[..end]), render(&growth[..=end]))
                else {
                    panic!("renders at {end}");
                };
                let kept = if matches!(growth[end], Message::Assistant { .. }) {
                    before.len()
                } else {
                    before.len() - 1
                };
                assert_eq!(before[..kept], after[..kept], "{system_role} at {end}");
            }
        }
    }
}
