//! The history's shape (I3/I12 support): every tool use answered in the next message, no
//! stray or orphan results, never an assistant turn first.

use crate::history::{AssistantPart, Message, UserPart};

pub fn history_shape(messages: &[Message]) -> Vec<String> {
    let mut problems = Vec::new();
    let first = messages
        .iter()
        .find(|message| !matches!(message, Message::System { .. }));
    if matches!(first, Some(Message::Assistant { .. })) {
        problems.push("history: the first turn is the assistant's".to_owned());
    }
    for (index, message) in messages.iter().enumerate() {
        let calls = tool_uses(message);
        let answered: Vec<&str> = messages
            .get(index + 1)
            .map(tool_results)
            .unwrap_or_default();
        for id in &calls {
            if !answered.contains(id) {
                problems.push(format!(
                    "history: tool_use {id} has no tool_result after it"
                ));
            }
        }
        let asked: Vec<&str> = index
            .checked_sub(1)
            .and_then(|before| messages.get(before))
            .map(tool_uses)
            .unwrap_or_default();
        for id in tool_results(message) {
            if !asked.contains(&id) {
                problems.push(format!(
                    "history: tool_result {id} answers no call before it"
                ));
            }
        }
    }
    problems
}

fn tool_uses(message: &Message) -> Vec<&str> {
    let Message::Assistant { parts, .. } = message else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolUse { id, .. } => Some(id.as_str()),
            AssistantPart::Text { .. } => None,
        })
        .collect()
}

fn tool_results(message: &Message) -> Vec<&str> {
    let Message::User { parts } = message else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            UserPart::ToolResult { id, .. } => Some(id.as_str()),
            UserPart::Text { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::history_shape;
    use crate::history::{AssistantPart, Message, UserPart};
    use crate::ids::ToolCallId;

    fn id(text: &str) -> ToolCallId {
        ToolCallId::parse(text).unwrap_or_else(|| unreachable!("a valid id"))
    }

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![UserPart::Text { text: text.into() }],
        }
    }

    fn calls(ids: &[&str]) -> Message {
        Message::Assistant {
            parts: ids
                .iter()
                .map(|call| AssistantPart::ToolUse {
                    id: id(call),
                    name: "read_file".into(),
                    input_json: "{}".into(),
                })
                .collect(),
            raw: None,
        }
    }

    fn results(ids: &[&str]) -> Message {
        Message::User {
            parts: ids
                .iter()
                .map(|call| UserPart::ToolResult {
                    id: id(call),
                    content: "ok".into(),
                    is_error: false,
                })
                .collect(),
        }
    }

    #[test]
    fn a_well_formed_history_passes() {
        let history = [
            Message::System {
                text: "Board now: empty".into(),
            },
            user("show me the service"),
            calls(&["a", "b"]),
            results(&["a", "b"]),
            Message::Assistant {
                parts: vec![AssistantPart::Text {
                    text: "Here.".into(),
                }],
                raw: None,
            },
        ];
        assert!(history_shape(&history).is_empty());
    }

    /// The planted violations: each shape the providers reject turns the check red.
    #[test]
    fn orphans_strays_and_an_assistant_first_are_caught() {
        let orphan = [user("hi"), calls(&["a"]), user("what does it do")];
        assert_eq!(
            history_shape(&orphan).len(),
            1,
            "{:?}",
            history_shape(&orphan)
        );
        let late = [
            user("hi"),
            calls(&["a"]),
            Message::System {
                text: "note".into(),
            },
            results(&["a"]),
        ];
        assert_eq!(history_shape(&late).len(), 2, "{:?}", history_shape(&late));
        let partial = [user("hi"), calls(&["a", "b"]), results(&["a"])];
        assert_eq!(history_shape(&partial).len(), 1);
        let assistant_first = [
            Message::System {
                text: "context".into(),
            },
            Message::Assistant {
                parts: vec![AssistantPart::Text {
                    text: "Sure".into(),
                }],
                raw: None,
            },
            user("and the database"),
        ];
        assert_eq!(history_shape(&assistant_first).len(), 1);
    }
}
