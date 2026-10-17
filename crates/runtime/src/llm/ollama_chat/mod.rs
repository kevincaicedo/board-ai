//! Ollama's native chat API (`POST /api/chat`, newline-delimited JSON) for the talker and scribe
//! on a local Ollama (LLD §3.3).
//!
//! Ollama's OpenAI-compatible endpoint cannot ask for a context window (it ignores
//! `options.num_ctx`), and Ollama loads a model with 4,096 tokens: too few for the system prompt
//! and tools (~3,600) plus a conversation, so Ollama cut the front of the prompt or stopped the
//! answer at the window. Every request here carries `options.num_ctx`
//! ([`OLLAMA_CONTEXT_TOKENS`]), `keep_alive` ([`OLLAMA_KEEP_ALIVE_SECONDS`]) and, at effort
//! `None`, `think: false`. The request is otherwise frozen per session like the other adapters,
//! so Ollama reuses the cached prompt prefix.
//!
//! [`OLLAMA_CONTEXT_TOKENS`]: crate::limits::OLLAMA_CONTEXT_TOKENS
//! [`OLLAMA_KEEP_ALIVE_SECONDS`]: crate::limits::OLLAMA_KEEP_ALIVE_SECONDS

mod decode;
mod maybe;
mod native_calls;
mod request;

use std::sync::Arc;

use boardai_core::history::Conversation;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use self::decode::OllamaDecoder;
use self::request::FrozenRequest;
use super::error::ProviderError;
use super::http::{RequestSpec, bearer, build_client, endpoint, post_and_drain};
use super::ollama::{OllamaProbe, ollama_root, probe_ollama};
use super::session::{self, Framing, SessionParts};
use super::{ChatStream, LlmCapabilities, LlmProvider, RetryPolicy, TalkerConfig, WarmUp};
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::limits::TALKER_WARM_UP_MAX_BYTES;
use crate::settings::Secret;

pub use maybe::MaybeOllama;

/// Ollama's port when it runs as installed.
const OLLAMA_PORT: u16 = 11434;

/// How to reach Ollama: its OpenAI-compatible base URL (`http://localhost:11434/v1`) or its
/// root. Local Ollama needs no key; one configured for the base URL (Ollama behind a proxy that
/// checks it) goes as `Authorization: Bearer` (D-193), on every request and on the probe, as the
/// OpenAI-compatible adapter would send it to the same host.
#[derive(Clone, Debug)]
pub struct OllamaChatOptions {
    pub base_url: String,
    pub api_key: Option<Secret>,
    pub clock: Arc<dyn Clock>,
    pub policy: Option<RetryPolicy>,
}

#[derive(Debug)]
pub struct OllamaChatProvider {
    client: reqwest::Client,
    root: String,
    url: String,
    headers: HeaderMap,
    frozen: FrozenRequest,
    /// The request's tool names, which calls written as text may name (D-191).
    tools: Arc<[String]>,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
}

impl OllamaChatProvider {
    pub fn new(config: &TalkerConfig, options: OllamaChatOptions) -> Result<Self, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/x-ndjson"));
        if let Some(key) = &options.api_key {
            headers.insert(AUTHORIZATION, bearer(key)?);
        }
        let root = ollama_root(&options.base_url).to_owned();
        Ok(Self {
            client: build_client()?,
            url: endpoint(&root, "api/chat")?,
            root,
            headers,
            frozen: FrozenRequest::new(config),
            tools: config.tools.iter().map(|tool| tool.name.clone()).collect(),
            policy: options
                .policy
                .unwrap_or_else(|| RetryPolicy::for_role(config.role)),
            clock: options.clock,
        })
    }

    /// Asks the server whether it is Ollama, where and as this adapter's requests would go.
    pub(super) async fn probe(&self) -> OllamaProbe {
        probe_ollama(&self.root, self.headers.get(AUTHORIZATION)).await
    }
}

impl LlmProvider for OllamaChatProvider {
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
            provider: "ollama",
            framing: Framing::NdJson,
        };
        session::open(parts, OllamaDecoder::new(&self.tools), cancel)
    }

    /// Loads the model with the session's context window (~4 s cold) and prefills the system
    /// prompt and tools (~3 s), which the first answer would otherwise wait for (D-154). The
    /// prefill is reused only by a model whose cache can be cut back to a shared prefix
    /// (qwen3.5:2b: 2.4 s → 0.6 s); gemma4's sliding-window cache can't, so for it only the
    /// load is saved. Dropped when the first real request starts.
    fn warm_up(&self) -> Option<WarmUp> {
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

/// Whether an OpenAI-compatible base URL is Ollama as installed: port 11434 on this machine,
/// which gets the native API without asking. Any other server is probed ([`MaybeOllama`]).
#[must_use]
pub fn is_local_ollama(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    let path = url.path().trim_end_matches('/');
    local && url.port() == Some(OLLAMA_PORT) && (path.is_empty() || path == "/v1")
}

#[cfg(test)]
mod tests {
    use super::{is_local_ollama, ollama_root};

    #[test]
    fn local_ollama_is_recognized_by_its_address() {
        assert!(is_local_ollama("http://localhost:11434/v1"));
        assert!(is_local_ollama("http://127.0.0.1:11434"));
        assert!(is_local_ollama("http://[::1]:11434/v1/"));
        assert!(!is_local_ollama("http://localhost:1234/v1"));
        assert!(!is_local_ollama("https://openrouter.ai/api/v1"));
        assert!(!is_local_ollama("http://gpu-box:11434/v1"));
        assert!(!is_local_ollama("not a url"));
        assert_eq!(
            ollama_root("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_root("http://localhost:11434"),
            "http://localhost:11434"
        );
    }
}
