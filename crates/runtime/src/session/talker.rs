//! The talker actor (LLD §3.1): one stream at a time from the configured [`LlmProvider`],
//! turned into talker inputs for the core.
//!
//! Each stream gets a generation number and its inputs carry it. `CancelTalk` drops the stream
//! (dropping is the cancellation) and forgets its generation, so anything it already put in the
//! inbox is dropped there and never reaches the core (D-034).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::effect::TalkRequest;
use boardai_core::history::Conversation;
use boardai_core::input::{Input, StopReason, TalkerFailure, TalkerFailureKind};
use futures_util::StreamExt;
use futures_util::stream;
use tokio::task::JoinHandle;

use super::inbox::Inbox;
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::limits::TALKER_WARM_UP_TIMEOUT_MS;
use crate::llm::anthropic::{AnthropicOptions, AnthropicProvider};
use crate::llm::ollama_chat::{
    MaybeOllama, OllamaChatOptions, OllamaChatProvider, is_local_ollama,
};
use crate::llm::openai::{Dialect, OLLAMA_BASE_URL, OpenAiCompatibleProvider, OpenAiOptions};
use crate::llm::openrouter::{ProviderSlug, is_openrouter};
use crate::llm::{
    ChatEvent, ChatStream, LlmCapabilities, LlmProvider, ProviderError, TalkerConfig, ToolSpec,
};
use crate::settings::{LlmKind, LlmSettings, Secret, SecretError, SecretStore};

/// The current stream, if any.
#[derive(Debug)]
struct Run {
    generation: u64,
    cancel: CancelToken,
    task: JoinHandle<()>,
}

pub(crate) struct Talker {
    provider: Arc<dyn LlmProvider>,
    next_generation: u64,
    current: Option<Run>,
    /// The session's warm-up (D-154), dropped when a real request starts or the talker stops.
    warming: Option<JoinHandle<()>>,
}

impl Talker {
    pub(crate) fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            provider,
            next_generation: 0,
            current: None,
            warming: None,
        }
    }

    /// Readies the provider for the session's first request, in the background and within
    /// `TALKER_WARM_UP_TIMEOUT_MS`. Nothing it does reaches the core or the conversation; a
    /// real request drops it at once, so it never holds one up.
    pub(crate) fn warm_up(&mut self) {
        self.stop_warm_up();
        let Some(warm_up) = self.provider.warm_up() else {
            return;
        };
        let bound = Duration::from_millis(TALKER_WARM_UP_TIMEOUT_MS);
        self.warming = Some(tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let outcome = tokio::time::timeout(bound, warm_up).await;
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            match outcome {
                Ok(Ok(())) => tracing::info!(elapsed_ms, "talker warmed up"),
                // The first request will meet the same problem and say it out loud.
                Ok(Err(error)) => tracing::info!(elapsed_ms, %error, "talker warm-up failed"),
                Err(_elapsed) => tracing::info!(elapsed_ms, "talker warm-up timed out"),
            }
        }));
    }

    fn stop_warm_up(&mut self) {
        if let Some(task) = self.warming.take() {
            task.abort();
        }
    }

    /// Starts a stream for the whole history. The core sends `CancelTalk` before a new `Talk`
    /// (I5); a stream still here is cancelled anyway, so two never run at once.
    pub(crate) fn talk(&mut self, request: TalkRequest, inbox: &Inbox) {
        debug_assert!(
            self.current.is_none(),
            "I5: Talk while a stream is in flight"
        );
        self.cancel();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        let cancel = CancelToken::new();
        let mut conversation = Conversation::new();
        for message in request.messages {
            conversation.push(message);
        }
        let stream = self.provider.stream(&conversation, cancel.clone());
        let task = tokio::spawn(forward(stream, generation, inbox.clone()));
        self.current = Some(Run {
            generation,
            cancel,
            task,
        });
    }

    /// Drops the stream in flight and any warm-up; the stream's inputs still in the inbox are
    /// dropped there.
    pub(crate) fn cancel(&mut self) {
        self.stop_warm_up();
        if let Some(run) = self.current.take() {
            run.cancel.cancel();
            run.task.abort();
        }
    }

    /// Whether an input from stream `generation` may reach the core.
    pub(crate) fn accepts(&self, generation: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|run| run.generation == generation)
    }

    /// The stream delivered its last input.
    pub(crate) fn finished(&mut self, generation: u64) {
        if self.accepts(generation) {
            self.current = None;
        }
    }
}

/// A talker dropped without `cancel` (an aborted session task) still stops its tasks.
impl Drop for Talker {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Sends a stream's events to the inbox as talker inputs. Ends after the stream's last event;
/// a stream that ends without one still ends the turn, as a failure the user hears.
async fn forward(mut stream: ChatStream, generation: u64, inbox: Inbox) {
    while let Some(item) = stream.next().await {
        let (input, last) = match item {
            Ok(ChatEvent::TextDelta(text)) => (Input::TalkerDelta { text }, false),
            Ok(ChatEvent::ToolCall(invocation)) => (Input::TalkerToolCall { invocation }, false),
            Ok(ChatEvent::Done { stop, usage, raw }) => {
                tracing::debug!(
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    cache_read_tokens = usage.cache_read_input_tokens,
                    cache_write_tokens = usage.cache_creation_input_tokens,
                    "talker usage"
                );
                (Input::TalkerDone { stop, raw }, true)
            }
            Err(error) => {
                tracing::warn!(%error, "talker stream failed");
                let failure = error.to_talker_failure();
                let stop = StopReason::Failed { failure };
                (Input::TalkerDone { stop, raw: None }, true)
            }
        };
        if inbox.send_talker(generation, input).await.is_err() || last {
            return;
        }
    }
    let failure = TalkerFailure {
        kind: TalkerFailureKind::Network,
        detail: "the stream ended without a result".to_owned(),
    };
    let stop = StopReason::Failed { failure };
    let _ = inbox
        .send_talker(generation, Input::TalkerDone { stop, raw: None })
        .await;
}

/// A talker that can't be built (no key, a bad URL). Every request fails at once with
/// `NotConfigured`, so the user hears why instead of silence (LLD §6).
#[derive(Clone, Debug)]
pub struct UnconfiguredTalker {
    detail: String,
}

impl UnconfiguredTalker {
    #[must_use]
    pub fn new(detail: String) -> Self {
        Self { detail }
    }
}

impl LlmProvider for UnconfiguredTalker {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: true,
            tools: false,
            server_side_fallback: false,
        }
    }

    fn stream(&self, _conversation: &Conversation, _cancel: CancelToken) -> ChatStream {
        let error = ProviderError::NotConfigured {
            detail: self.detail.clone(),
        };
        Box::pin(stream::once(std::future::ready(Err(error))))
    }
}

/// Why the configured talker could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TalkerSetupError {
    Secret(SecretError),
    Provider(ProviderError),
}

impl fmt::Display for TalkerSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Secret(error) => write!(formatter, "{error}"),
            Self::Provider(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TalkerSetupError {}

/// The session config for the talker: the settings' model, plus BoardAI's frozen system prompt
/// and its full tool set (LLD §3.2).
#[must_use]
pub fn talker_config(settings: &LlmSettings) -> TalkerConfig {
    let tools = boardai_core::tools::schemas()
        .into_iter()
        .map(|schema| ToolSpec {
            name: schema.name.to_owned(),
            description: schema.description.to_owned(),
            input_schema: schema.input_schema,
        })
        .collect();
    let mut config = TalkerConfig::talker(boardai_core::prompt::system_prompt().to_owned(), tools);
    config.model.clone_from(&settings.model);
    config.effort = settings.effort;
    config.max_tokens = settings.max_tokens;
    config
}

/// Builds the talker the settings describe. Reading the secret may block (the Keychain can
/// prompt), so async callers run this on the blocking pool.
pub fn talker_from_settings(
    settings: &LlmSettings,
    secrets: &dyn SecretStore,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn LlmProvider>, TalkerSetupError> {
    provider_from_settings(settings, &talker_config(settings), secrets, clock)
}

/// The scribe the settings describe (`[scribe]`): the store's fixed digest prompt with the
/// settings' model, effort and output budget (D-017). Blocks like [`talker_from_settings`].
pub fn scribe_from_settings(
    settings: &LlmSettings,
    secrets: &dyn SecretStore,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn LlmProvider>, TalkerSetupError> {
    let mut config = crate::store::scribe_config();
    config.model.clone_from(&settings.model);
    config.effort = settings.effort;
    config.max_tokens = settings.max_tokens;
    provider_from_settings(settings, &config, secrets, clock)
}

fn provider_from_settings(
    settings: &LlmSettings,
    config: &TalkerConfig,
    secrets: &dyn SecretStore,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn LlmProvider>, TalkerSetupError> {
    let secret = settings
        .secret
        .as_ref()
        .map(|name| secrets.get(name))
        .transpose()
        .map_err(TalkerSetupError::Secret)?;
    let provider: Arc<dyn LlmProvider> = match settings.kind {
        LlmKind::Anthropic => {
            let Some(api_key) = secret else {
                let detail = "the Anthropic provider has no secret set".to_owned();
                let error = ProviderError::NotConfigured { detail };
                return Err(TalkerSetupError::Provider(error));
            };
            let options = AnthropicOptions {
                base_url: settings.base_url.clone(),
                api_key,
                clock,
                policy: None,
            };
            Arc::new(AnthropicProvider::new(config, options).map_err(TalkerSetupError::Provider)?)
        }
        LlmKind::OpenAiCompatible => open_ai_compatible(settings, config, secret, clock)
            .map_err(TalkerSetupError::Provider)?,
    };
    Ok(provider)
}

/// Which adapter an OpenAI-compatible base URL gets.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CompatibleApi {
    /// Ollama on this machine: native without asking, so it can be given a context window
    /// the system prompt and a conversation fit in (D-133).
    LocalOllama,
    /// OpenRouter: its own request fields, and the settings' route (D-152).
    OpenRouter { route: Vec<ProviderSlug> },
    /// Anything else: asked on first use whether it is Ollama (D-156).
    Probed,
}

fn compatible_api(settings: &LlmSettings, base_url: &str) -> CompatibleApi {
    if is_local_ollama(base_url) {
        CompatibleApi::LocalOllama
    } else if is_openrouter(base_url) {
        CompatibleApi::OpenRouter {
            route: settings.route.clone(),
        }
    } else {
        CompatibleApi::Probed
    }
}

fn open_ai_compatible(
    settings: &LlmSettings,
    config: &TalkerConfig,
    secret: Option<Secret>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn LlmProvider>, ProviderError> {
    let base_url = settings
        .base_url
        .clone()
        .unwrap_or_else(|| OLLAMA_BASE_URL.to_owned());
    let native = |clock| {
        let options = OllamaChatOptions {
            base_url: base_url.clone(),
            api_key: secret.clone(),
            clock,
            policy: None,
        };
        OllamaChatProvider::new(config, options)
    };
    let compatible = |dialect, clock| {
        let options = OpenAiOptions {
            base_url: base_url.clone(),
            api_key: secret.clone(),
            clock,
            policy: None,
            dialect,
        };
        OpenAiCompatibleProvider::new(config, options)
    };
    let provider: Arc<dyn LlmProvider> = match compatible_api(settings, &base_url) {
        CompatibleApi::LocalOllama => Arc::new(native(clock)?),
        CompatibleApi::OpenRouter { route } => {
            Arc::new(compatible(Dialect::OpenRouter { route }, clock)?)
        }
        CompatibleApi::Probed => Arc::new(MaybeOllama::new(
            native(Arc::clone(&clock))?,
            compatible(Dialect::Standard, clock)?,
        )),
    };
    Ok(provider)
}

/// How the spoken failure lines name the talker ("I lost the connection to Claude.").
#[must_use]
pub fn talker_name(settings: &LlmSettings) -> String {
    let local = settings
        .base_url
        .as_deref()
        .is_none_or(|url| url.contains("localhost") || url.contains("127.0.0.1"));
    let open_router = settings.base_url.as_deref().is_some_and(is_openrouter);
    match settings.kind {
        LlmKind::Anthropic => "Claude".to_owned(),
        LlmKind::OpenAiCompatible if local => "the local model".to_owned(),
        LlmKind::OpenAiCompatible if open_router => "OpenRouter".to_owned(),
        LlmKind::OpenAiCompatible => "the model".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CompatibleApi, compatible_api, talker_name};
    use crate::llm::Effort;
    use crate::llm::openrouter::ProviderSlug;
    use crate::settings::{LlmKind, LlmSettings};

    fn settings(kind: LlmKind, base_url: Option<&str>) -> LlmSettings {
        LlmSettings {
            kind,
            model: "m".to_owned(),
            effort: Effort::Low,
            max_tokens: 100,
            base_url: base_url.map(str::to_owned),
            secret: None,
            route: Vec::new(),
        }
    }

    #[test]
    fn failure_lines_name_the_talker_by_where_it_runs() {
        let name = |kind, url| talker_name(&settings(kind, url));
        assert_eq!(name(LlmKind::Anthropic, None), "Claude");
        let local = Some("http://localhost:11434/v1");
        assert_eq!(name(LlmKind::OpenAiCompatible, local), "the local model");
        let open_router = Some("https://openrouter.ai/api/v1");
        assert_eq!(name(LlmKind::OpenAiCompatible, open_router), "OpenRouter");
        let other = Some("http://gpu-box:1234/v1");
        assert_eq!(name(LlmKind::OpenAiCompatible, other), "the model");
    }

    #[test]
    fn each_server_gets_its_adapter() {
        let mut open_router = settings(LlmKind::OpenAiCompatible, None);
        open_router.route = ProviderSlug::parse("cerebras").into_iter().collect();
        let api = |url: &str| compatible_api(&open_router, url);
        assert_eq!(api("http://localhost:11434/v1"), CompatibleApi::LocalOllama);
        assert_eq!(
            api("https://openrouter.ai/api/v1"),
            CompatibleApi::OpenRouter {
                route: open_router.route.clone()
            }
        );
        assert_eq!(api("http://gpu-box:11434/v1"), CompatibleApi::Probed);
        assert_eq!(api("http://localhost:1234/v1"), CompatibleApi::Probed);
    }
}
