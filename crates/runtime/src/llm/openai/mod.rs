//! OpenAI-compatible chat completions (`/v1/chat/completions`, streaming): OpenRouter, Ollama
//! (`http://localhost:11434/v1`), LM Studio and similar servers (LLD §3.3). OpenRouter's
//! requests differ in two fields ([`Dialect::OpenRouter`]): a provider route and its
//! `reasoning` object.

mod decode;
mod request;

use std::sync::Arc;

use boardai_core::history::Conversation;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use self::decode::OpenAiDecoder;
use self::request::FrozenRequest;
use super::error::ProviderError;
use super::http::{RequestSpec, bearer, build_client, endpoint, post_and_drain};
use super::openrouter::ProviderSlug;
use super::session::{self, Framing, SessionParts};
use super::{ChatStream, LlmCapabilities, LlmProvider, RetryPolicy, TalkerConfig, WarmUp};
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::limits::TALKER_WARM_UP_MAX_BYTES;
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
    pub dialect: Dialect,
}

/// What the server makes of the request's extra fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// Any OpenAI-compatible server: effort goes out as `reasoning_effort`.
    #[default]
    Standard,
    /// OpenRouter: effort goes out as `reasoning.effort`, and a route as `provider.order`
    /// with no fallbacks: only the route's providers, in order (D-152). An empty route leaves
    /// the choice to OpenRouter.
    OpenRouter { route: Vec<ProviderSlug> },
}

#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    frozen: FrozenRequest,
    /// The provider a route asks for first, to notice when OpenRouter served another.
    expected: Option<ProviderSlug>,
    /// Whether a session's first request is worth readying (OpenRouter, D-154).
    warms_up: bool,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: &TalkerConfig, options: OpenAiOptions) -> Result<Self, ProviderError> {
        Ok(Self {
            client: build_client()?,
            url: endpoint(&options.base_url, "chat/completions")?,
            headers: headers(options.api_key.as_ref())?,
            frozen: FrozenRequest::new(config, &options.dialect),
            expected: match &options.dialect {
                Dialect::Standard => None,
                Dialect::OpenRouter { route } => route.first().cloned(),
            },
            warms_up: matches!(options.dialect, Dialect::OpenRouter { .. }),
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
            // Context is folded into user turns (D-160).
            mid_conversation_system: false,
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
        session::open(parts, OpenAiDecoder::new(self.expected.clone()), cancel)
    }

    fn warm_up(&self) -> Option<WarmUp> {
        if !self.warms_up {
            return None;
        }
        let request = self.frozen.warm_up_body().map(|body| RequestSpec {
            url: self.url.clone(),
            headers: self.headers.clone(),
            body,
        });
        let client = self.client.clone();
        Some(Box::pin(async move {
            post_and_drain(client, request?, TALKER_WARM_UP_MAX_BYTES).await
        }))
    }
}

fn headers(api_key: Option<&Secret>) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    if let Some(key) = api_key {
        headers.insert(AUTHORIZATION, bearer(key)?);
    }
    Ok(headers)
}
