//! A server that may be Ollama on another host or port (D-156). Ollama there needs the native
//! API as much as on this Mac (its OpenAI-compatible endpoint ignores the context window), but
//! its address doesn't say what it is. So the first use asks `GET /api/version`, once, within
//! `OLLAMA_PROBE_TIMEOUT_MS`: Ollama gets [`OllamaChatProvider`], anything else (LM Studio, vLLM,
//! a proxy) the OpenAI-compatible adapter. The session's warm-up is usually that first use, so
//! the first request finds the answer ready.

use std::sync::Arc;

use boardai_core::history::Conversation;
use futures_util::StreamExt;
use futures_util::stream;
use tokio::sync::OnceCell;

use super::OllamaChatProvider;
use crate::cancel::CancelToken;
use crate::llm::ollama::is_ollama;
use crate::llm::openai::OpenAiCompatibleProvider;
use crate::llm::{ChatStream, LlmCapabilities, LlmProvider, WarmUp};

/// Which adapter a probed server gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    OllamaNative,
    OpenAiCompatible,
}

#[derive(Debug)]
pub struct MaybeOllama {
    native: Arc<OllamaChatProvider>,
    compatible: Arc<OpenAiCompatibleProvider>,
    base_url: String,
    api: Arc<OnceCell<Api>>,
}

impl MaybeOllama {
    /// Both adapters are built for `base_url`; the probe picks one on first use.
    #[must_use]
    pub fn new(
        native: OllamaChatProvider,
        compatible: OpenAiCompatibleProvider,
        base_url: String,
    ) -> Self {
        Self {
            native: Arc::new(native),
            compatible: Arc::new(compatible),
            base_url,
            api: Arc::new(OnceCell::new()),
        }
    }

    /// The probe's answer, asked once; later callers wait for the first.
    async fn api(cell: &OnceCell<Api>, base_url: &str) -> Api {
        *cell
            .get_or_init(|| async {
                let api = if is_ollama(base_url).await {
                    Api::OllamaNative
                } else {
                    Api::OpenAiCompatible
                };
                tracing::info!(?api, "probed the talker's server");
                api
            })
            .await
    }
}

impl LlmProvider for MaybeOllama {
    /// Both adapters say the same: system messages anywhere, tools, no server-side fallback.
    fn capabilities(&self) -> LlmCapabilities {
        self.compatible.capabilities()
    }

    fn stream(&self, conversation: &Conversation, cancel: CancelToken) -> ChatStream {
        match self.api.get() {
            Some(Api::OllamaNative) => return self.native.stream(conversation, cancel),
            Some(Api::OpenAiCompatible) => return self.compatible.stream(conversation, cancel),
            None => {}
        }
        let conversation = conversation.clone();
        let native = Arc::clone(&self.native);
        let compatible = Arc::clone(&self.compatible);
        let cell = Arc::clone(&self.api);
        let base_url = self.base_url.clone();
        let opened = async move {
            let api = tokio::select! {
                biased;
                () = cancel.cancelled() => return stream::empty().boxed(),
                api = Self::api(&cell, &base_url) => api,
            };
            match api {
                Api::OllamaNative => native.stream(&conversation, cancel),
                Api::OpenAiCompatible => compatible.stream(&conversation, cancel),
            }
        };
        stream::once(opened).flatten().boxed()
    }

    /// Probes, then warms Ollama up as on this Mac. Another server gets its own warm-up, if
    /// it has one.
    fn warm_up(&self) -> Option<WarmUp> {
        let native = Arc::clone(&self.native);
        let compatible = Arc::clone(&self.compatible);
        let cell = Arc::clone(&self.api);
        let base_url = self.base_url.clone();
        Some(Box::pin(async move {
            let warm_up = match Self::api(&cell, &base_url).await {
                Api::OllamaNative => native.warm_up(),
                Api::OpenAiCompatible => compatible.warm_up(),
            };
            match warm_up {
                Some(warm_up) => warm_up.await,
                None => Ok(()),
            }
        }))
    }
}
