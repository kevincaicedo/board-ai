//! OpenAI-compatible chat completions (`/v1/chat/completions`, streaming): OpenRouter, Ollama
//! (`http://localhost:11434/v1`), LM Studio and similar servers (LLD §3.3).

mod decode;
mod request;

use std::sync::Arc;

use boardai_core::history::Conversation;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use self::decode::OpenAiDecoder;
use self::request::FrozenRequest;
use super::error::ProviderError;
use super::http::{RequestSpec, build_client, endpoint};
use super::session::{self, Framing, SessionParts};
use super::{ChatStream, LlmCapabilities, LlmProvider, RetryPolicy, TalkerConfig};
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::settings::Secret;

/// Ollama's OpenAI-compatible endpoint on its default port.
pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

/// OpenRouter's OpenAI-compatible endpoint.
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// How to reach the server. Local servers need no key.
#[derive(Clone, Debug)]
pub struct OpenAiOptions {
    pub base_url: String,
    pub api_key: Option<Secret>,
    pub clock: Arc<dyn Clock>,
    pub policy: Option<RetryPolicy>,
}

#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    frozen: FrozenRequest,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: &TalkerConfig, options: OpenAiOptions) -> Result<Self, ProviderError> {
        Ok(Self {
            client: build_client()?,
            url: endpoint(&options.base_url, "chat/completions")?,
            headers: headers(options.api_key.as_ref())?,
            frozen: FrozenRequest::new(config),
            policy: options
                .policy
                .unwrap_or_else(|| RetryPolicy::for_role(config.role)),
            clock: options.clock,
        })
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: true,
            tools: true,
            server_side_fallback: false,
        }
    }

    fn stream(&self, conversation: &Conversation, cancel: CancelToken) -> ChatStream {
        let request = self
            .frozen
            .body(conversation.messages())
            .map(|body| RequestSpec {
                url: self.url.clone(),
                headers: self.headers.clone(),
                body,
            });
        let parts = SessionParts {
            client: self.client.clone(),
            request,
            policy: self.policy,
            clock: Arc::clone(&self.clock),
            provider: "openai_compatible",
            framing: Framing::Sse,
        };
        session::open(parts, OpenAiDecoder::default(), cancel)
    }
}

fn headers(api_key: Option<&Secret>) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    if let Some(key) = api_key {
        let mut bearer =
            HeaderValue::from_str(&format!("Bearer {}", key.expose())).map_err(|_| {
                ProviderError::NotConfigured {
                    detail: "the API key has characters that cannot be sent".to_owned(),
                }
            })?;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);
    }
    Ok(headers)
}
