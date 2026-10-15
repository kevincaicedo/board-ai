//! The Anthropic Messages API over raw HTTPS and SSE (LLD §3.3). Rust has no official SDK; the
//! wire format follows the claude-api reference's raw HTTP documentation.
//!
//! Per session: a frozen system prompt with a cache breakpoint, the full tool set declared once
//! with `eager_input_streaming`, adaptive thinking at the configured effort, `tool_choice` auto,
//! automatic caching of the conversation tail, and server-side refusal fallbacks where the model
//! has them. Per request: the whole append-only conversation, never ending in an assistant
//! prefill.

mod decode;
mod models;
mod request;

use std::sync::Arc;

use boardai_core::history::Conversation;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};

use self::decode::AnthropicDecoder;
use self::models::ModelTraits;
use self::request::FrozenRequest;
use super::error::ProviderError;
use super::http::{RequestSpec, build_client, endpoint};
use super::session::{self, SessionParts};
use super::{ChatStream, LlmCapabilities, LlmProvider, RetryPolicy, TalkerConfig};
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::settings::Secret;

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// How to reach the API. `policy` defaults to the role's bounds.
#[derive(Clone, Debug)]
pub struct AnthropicOptions {
    pub base_url: Option<String>,
    pub api_key: Secret,
    pub clock: Arc<dyn Clock>,
    pub policy: Option<RetryPolicy>,
}

#[derive(Debug)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    frozen: FrozenRequest,
    traits: ModelTraits,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
}

impl AnthropicProvider {
    pub fn new(config: &TalkerConfig, options: AnthropicOptions) -> Result<Self, ProviderError> {
        let traits = models::traits(&config.model);
        let base_url = options.base_url.as_deref().unwrap_or(ANTHROPIC_BASE_URL);
        Ok(Self {
            client: build_client()?,
            url: endpoint(base_url, "v1/messages")?,
            headers: headers(&options.api_key, traits)?,
            frozen: FrozenRequest::new(config, traits),
            traits,
            policy: options
                .policy
                .unwrap_or_else(|| RetryPolicy::for_role(config.role)),
            clock: options.clock,
        })
    }
}

impl LlmProvider for AnthropicProvider {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: self.traits.mid_conversation_system,
            tools: true,
            server_side_fallback: self.traits.server_side_fallback,
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
            provider: "anthropic",
        };
        session::open(parts, AnthropicDecoder::default(), cancel)
    }
}

fn headers(api_key: &Secret, traits: ModelTraits) -> Result<HeaderMap, ProviderError> {
    let mut key =
        HeaderValue::from_str(api_key.expose()).map_err(|_| ProviderError::NotConfigured {
            detail: "the Anthropic API key has characters that cannot be sent".to_owned(),
        })?;
    // Keeps the key out of any `Debug` print of the request.
    key.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(HeaderName::from_static("x-api-key"), key);
    headers.insert(
        HeaderName::from_static("anthropic-version"),
        HeaderValue::from_static(API_VERSION),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    if traits.server_side_fallback {
        headers.insert(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static(SERVER_SIDE_FALLBACK_BETA),
        );
    }
    Ok(headers)
}
