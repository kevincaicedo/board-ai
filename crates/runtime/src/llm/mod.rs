//! LLM providers behind one seam (LLD §3.2, §3.3): the talker and the scribe stream a
//! [`Conversation`] and get back text deltas, validated tool calls and a final [`ChatEvent::Done`].
//!
//! Adapters own the wire format and nothing else: they make no conversation decisions. The
//! system prompt and tool set are fixed per session and injected through [`TalkerConfig`], so the
//! cached prefix never changes mid-session.

pub mod anthropic;
mod error;
mod http;
pub mod openai;
mod session;
pub mod sse;

use std::time::Duration;

use boardai_core::history::{Conversation, RawAssistant};
use boardai_core::input::StopReason;
use boardai_core::tools::ToolInvocation;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::limits::{
    PROVIDER_STREAM_IDLE_TIMEOUT_MS, SCRIBE_FIRST_TOKEN_DEADLINE_MS, SCRIBE_MAX_TOKENS_DEFAULT,
    TALKER_FIRST_TOKEN_DEADLINE_MS, TALKER_MAX_TOKENS_DEFAULT,
};

pub use error::{ProviderError, TimeoutKind};

/// The default talker (D-017). Spike S3 may change it; that is the owner's call.
pub const DEFAULT_TALKER_MODEL: &str = "claude-opus-5-5";
/// The default scribe (D-017).
pub const DEFAULT_SCRIBE_MODEL: &str = "claude-haiku-5-5";

/// A provider stream. Dropping it cancels the request.
pub type ChatStream = BoxStream<'static, Result<ChatEvent, ProviderError>>;

pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> LlmCapabilities;

    /// Streams one response to the whole append-only conversation. The stream ends after
    /// `Done`, after an error, or (with no further item) when `cancel` fires.
    fn stream(&self, conversation: &Conversation, cancel: CancelToken) -> ChatStream;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LlmCapabilities {
    /// `Message::System` goes out as a mid-conversation system message. Without it the text is
    /// folded into the next user message as a block (LLD §3.3).
    pub mid_conversation_system: bool,
    pub tools: bool,
    /// A classifier refusal is retried server-side on a fallback model.
    pub server_side_fallback: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    TextDelta(String),
    /// A complete tool call. A call that failed validation is still delivered, with `call` set
    /// to the error, so the core can answer it with an error result.
    ToolCall(ToolInvocation),
    /// The response is complete. `raw` is the provider's own assistant content, to be echoed
    /// back unchanged.
    Done {
        stop: StopReason,
        usage: Usage,
        raw: Option<RawAssistant>,
    },
}

/// Token counts of one response. `input_tokens` is the uncached remainder only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// How hard the model thinks. On Claude it is `output_config.effort`; on OpenAI-compatible
/// servers it is `reasoning_effort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// No thinking where the model allows it. Local thinking models (Ollama) need it for a
    /// responsive voice: at `low`, `qwen3.5:2b` spent its whole budget reasoning. Claude has no
    /// "off" on current models (Opus 5.5 rejects disabled thinking), so there it means `low`.
    None,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    ExtraHigh,
    Max,
}

impl Effort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::ExtraHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Which job the model does; it sets the latency bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmRole {
    /// The voice: bounded by the first-token deadline the user can feel.
    Talker,
    /// Digests and memory: not latency-bound, but still bounded.
    Scribe,
}

/// A tool as declared to the provider. The set is fixed for the session (LLD §3.3).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Everything about a request that stays the same for the whole session.
#[derive(Clone, Debug, PartialEq)]
pub struct TalkerConfig {
    pub role: LlmRole,
    pub model: String,
    pub effort: Effort,
    pub max_tokens: u32,
    /// Frozen for the session and identical across sessions, so it caches.
    pub system_prompt: String,
    pub tools: Vec<ToolSpec>,
}

impl TalkerConfig {
    /// The default talker: `claude-opus-5-5` at effort `low` (D-017).
    #[must_use]
    pub fn talker(system_prompt: String, tools: Vec<ToolSpec>) -> Self {
        Self {
            role: LlmRole::Talker,
            model: DEFAULT_TALKER_MODEL.to_owned(),
            effort: Effort::Low,
            max_tokens: TALKER_MAX_TOKENS_DEFAULT,
            system_prompt,
            tools,
        }
    }

    /// The default scribe: `claude-haiku-5-5`. Effort is set explicitly because the model's
    /// default is `medium` and an unset value would change silently with the model.
    #[must_use]
    pub fn scribe(system_prompt: String) -> Self {
        Self {
            role: LlmRole::Scribe,
            model: DEFAULT_SCRIBE_MODEL.to_owned(),
            effort: Effort::Medium,
            max_tokens: SCRIBE_MAX_TOKENS_DEFAULT,
            system_prompt,
            tools: Vec::new(),
        }
    }
}

/// Retry and deadline bounds of one stream. The counts are fixed limits; only the times vary
/// by role (and in tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Until the first token, retries included.
    pub first_token_deadline: Duration,
    /// Between chunks after the first token.
    pub idle_timeout: Duration,
    /// First retry delay, doubled per retry, plus jitter.
    pub backoff_base: Duration,
}

impl RetryPolicy {
    #[must_use]
    pub const fn for_role(role: LlmRole) -> Self {
        let deadline_ms = match role {
            LlmRole::Talker => TALKER_FIRST_TOKEN_DEADLINE_MS,
            LlmRole::Scribe => SCRIBE_FIRST_TOKEN_DEADLINE_MS,
        };
        Self {
            first_token_deadline: Duration::from_millis(deadline_ms),
            idle_timeout: Duration::from_millis(PROVIDER_STREAM_IDLE_TIMEOUT_MS),
            backoff_base: Duration::from_millis(crate::limits::PROVIDER_RETRY_BACKOFF_BASE_MS),
        }
    }
}
