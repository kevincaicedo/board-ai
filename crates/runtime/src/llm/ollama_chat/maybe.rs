//! A server that may be Ollama on another host or port (D-156). Ollama there needs the native
//! API as much as on this Mac (its OpenAI-compatible endpoint ignores the context window), but
//! its address doesn't say what it is. So a use asks `GET /api/version` within
//! `OLLAMA_PROBE_TIMEOUT_MS`, where and with the key the chat requests use: Ollama gets
//! [`OllamaChatProvider`], anything else (LM Studio, vLLM, a proxy) the OpenAI-compatible
//! adapter. The session's warm-up is usually the first use, so the first request finds the
//! answer ready.
//!
//! Only a definite answer is kept (D-192). A probe that got none (a busy or restarting Ollama: a 503, a
//! blip, an answer past the deadline) sends that one use to the compatible API, and the next use
//! asks again, at most `OLLAMA_PROBES_MAX` times. Keeping "not Ollama" after one failed probe
//! sent every later request of the app's run to `/v1`, where Ollama loads the model with 4,096
//! tokens and cuts the prompt (D-133).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use boardai_core::history::Conversation;
use futures_util::StreamExt;
use futures_util::stream;
use tokio::sync::OnceCell;

use super::OllamaChatProvider;
use crate::cancel::CancelToken;
use crate::limits::OLLAMA_PROBES_MAX;
use crate::llm::ollama::OllamaProbe;
use crate::llm::openai::OpenAiCompatibleProvider;
use crate::llm::{ChatStream, LlmCapabilities, LlmProvider, WarmUp};

/// Which adapter a probed server gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    OllamaNative,
    OpenAiCompatible,
}

/// A probe that got no answer: this use goes to the compatible API, and the next one asks again.
#[derive(Debug)]
struct Unanswered;

#[derive(Debug)]
pub struct MaybeOllama {
    native: Arc<OllamaChatProvider>,
    compatible: Arc<OpenAiCompatibleProvider>,
    /// The definite answer, once there is one.
    api: Arc<OnceCell<Api>>,
    /// Probes sent so far, at most `OLLAMA_PROBES_MAX` without a definite answer.
    probes: Arc<AtomicU32>,
}

impl MaybeOllama {
    /// Both adapters are built for the same base URL; the probe goes where the native one's
    /// requests would, with its key.
    #[must_use]
    pub fn new(native: OllamaChatProvider, compatible: OpenAiCompatibleProvider) -> Self {
        Self {
            native: Arc::new(native),
            compatible: Arc::new(compatible),
            api: Arc::new(OnceCell::new()),
            probes: Arc::new(AtomicU32::new(0)),
        }
    }

    /// The API for one use: the kept answer, or a probe's. Concurrent uses wait for one probe;
    /// when it gets no answer, each of them goes to the compatible API this time.
    async fn api(cell: &OnceCell<Api>, probes: &AtomicU32, native: &OllamaChatProvider) -> Api {
        let decided = cell
            .get_or_try_init(|| async {
                let sent = probes.fetch_add(1, Ordering::Relaxed);
                if sent >= OLLAMA_PROBES_MAX {
                    tracing::warn!(
                        probes = sent,
                        "the talker's server never answered whether it is Ollama; keeping the \
                         OpenAI-compatible API"
                    );
                    return Ok(Api::OpenAiCompatible);
                }
                let api = match native.probe().await {
                    OllamaProbe::Ollama => Api::OllamaNative,
                    OllamaProbe::NotOllama => Api::OpenAiCompatible,
                    OllamaProbe::Unknown => return Err(Unanswered),
                };
                tracing::info!(?api, "probed the talker's server");
                Ok(api)
            })
            .await;
        match decided {
            Ok(api) => *api,
            Err(Unanswered) => Api::OpenAiCompatible,
        }
    }
}

impl LlmProvider for MaybeOllama {
    /// The chosen adapter's. Until the probe answers, the compatible adapter's, the narrower of
    /// the two: it takes system messages only first (D-160), the native one anywhere.
    fn capabilities(&self) -> LlmCapabilities {
        match self.api.get() {
            Some(Api::OllamaNative) => self.native.capabilities(),
            Some(Api::OpenAiCompatible) | None => self.compatible.capabilities(),
        }
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
        let probes = Arc::clone(&self.probes);
        let opened = async move {
            let api = tokio::select! {
                biased;
                () = cancel.cancelled() => return stream::empty().boxed(),
                api = Self::api(&cell, &probes, &native) => api,
            };
            match api {
                Api::OllamaNative => native.stream(&conversation, cancel),
                Api::OpenAiCompatible => compatible.stream(&conversation, cancel),
            }
        };
        stream::once(opened).flatten().boxed()
    }

    /// Probes, then warms Ollama up as on this Mac. Another server gets its own warm-up, if
    /// it has one; so does a server that didn't answer this time.
    fn warm_up(&self) -> Option<WarmUp> {
        let native = Arc::clone(&self.native);
        let compatible = Arc::clone(&self.compatible);
        let cell = Arc::clone(&self.api);
        let probes = Arc::clone(&self.probes);
        Some(Box::pin(async move {
            let warm_up = match Self::api(&cell, &probes, &native).await {
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
