//! The LLM conversation, provider-neutral and append-only (LLD §2.7, invariant I3).
//!
//! `Conversation` has `push` and no way to edit or remove a message. Claude's preserved-thinking
//! check rejects a history whose earlier turns changed, and prompt caching needs a stable prefix.
//! An interruption therefore never rewrites a turn; it appends what was heard.

use serde::{Deserialize, Serialize};

use crate::ids::ToolCallId;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        parts: Vec<UserPart>,
    },
    /// `raw` is the provider's own content, kept byte for byte when the turn completed so it can
    /// be echoed back unchanged (thinking blocks included). A turn cut by an interruption has no
    /// raw content: it holds only the heard prefix.
    Assistant {
        parts: Vec<AssistantPart>,
        raw: Option<RawAssistant>,
    },
    /// Context appended mid-conversation: recaps, board and thread updates, research results.
    /// Providers without mid-conversation system messages get it as a block in the next user
    /// message (LLD §3.3).
    System {
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum UserPart {
    Text {
        text: String,
    },
    ToolResult {
        id: ToolCallId,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum AssistantPart {
    Text {
        text: String,
    },
    ToolUse {
        id: ToolCallId,
        name: String,
        input_json: String,
    },
}

/// A provider's native assistant content as a JSON string, opaque to the core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawAssistant {
    pub provider: ProviderFamily,
    pub content_json: String,
}

/// Raw content is only echoed to the provider family that produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFamily {
    Anthropic,
    OpenAiCompatible,
    /// Ollama's native chat API (`/api/chat`).
    Ollama,
}
