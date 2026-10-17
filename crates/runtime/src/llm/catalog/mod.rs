//! The model picker's lists (D-153): OpenRouter's tool-capable models, one OpenRouter model's
//! providers, and the local Ollama's models. Each fetch is one bounded GET; answers are kept
//! for [`MODEL_LIST_CACHE_MS`] (Ollama's for [`OLLAMA_LIST_CACHE_MS`]), so a picker opened
//! again, or a save that needs a model's reasoning, doesn't ask again. The catalog lives as long
//! as the app's setup state. The lists are public; a provider list is asked with the saved
//! OpenRouter key when there is one ([`ListKey`], D-218), since only then does OpenRouter give
//! its providers' latency and throughput medians. The model list never carries a key.

mod parse;
mod preset_fit;

pub use preset_fit::{PresetMisfit, listing_misfit, route_misfit};

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use boardai_core::setup::{
    Drawing, ModelInfo, ModelListing, ModelProvider, ModelQuery, ModelReasoning,
};
use boardai_core::time::SessionTime;
use reqwest::header::HeaderValue;

use super::http::{bearer, endpoint, get_bounded};
use super::ollama::{OLLAMA_DEFAULT_MODEL, ollama_models};
use super::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use super::openrouter::{PRESETS, drawing, preset};
use super::{Effort, ProviderError};
use crate::clock::{Clock, MonotonicClock};
use crate::limits::{
    MODEL_LIST_CACHE_MS, MODEL_LIST_MAX_BYTES, MODEL_LIST_TIMEOUT_MS, MODEL_PROVIDERS_CACHED_MAX,
    MODEL_PROVIDERS_MAX_BYTES, MODEL_TEXT_MAX_BYTES, OLLAMA_LIST_CACHE_MS,
};
use crate::settings::Secret;

/// An OpenRouter model id from the picker (`qwen/qwen3.8-27b`, `openai/gpt-oss-120b:free`),
/// parsed where it enters: ASCII letters, digits and `- _ . : ~ /`, an author and a name, no
/// empty or dot-only path segment, at most [`MODEL_TEXT_MAX_BYTES`]. It goes into a URL path,
/// so nothing in it can climb out of `/models/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OpenRouterModelId(String);

impl OpenRouterModelId {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid_byte = |byte: u8| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'~' | b'/')
        };
        let segments_ok = text
            .split('/')
            .all(|segment| !segment.is_empty() && segment.bytes().any(|byte| byte != b'.'));
        let valid = text.len() <= MODEL_TEXT_MAX_BYTES
            && text.contains('/')
            && text.bytes().all(valid_byte)
            && segments_ok;
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a picker query was not answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    /// `Providers` named something that isn't an OpenRouter model id.
    NotAModelId,
    /// The list could not be fetched or read.
    Provider(ProviderError),
}

impl From<ProviderError> for CatalogError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

/// A fetched list and when it was fetched.
#[derive(Clone, Debug)]
struct Cached<T> {
    at: SessionTime,
    value: T,
}

/// A list and how many entries were left out of it.
pub type Counted<T> = (Vec<T>, u32);

#[derive(Debug, Default)]
struct Lists {
    open_router: Option<Cached<Counted<ModelInfo>>>,
    ollama: Option<Cached<Vec<ModelInfo>>>,
    /// Oldest first; at most [`MODEL_PROVIDERS_CACHED_MAX`].
    providers: VecDeque<(OpenRouterModelId, Cached<Counted<ModelProvider>>)>,
}

/// Where the catalog gets OpenRouter's key for a provider list (D-218). Asked only when a
/// provider list is fetched, never for the model list; it may block (the Keychain can prompt),
/// so it is asked on the blocking pool.
pub trait ListKey: Send + Sync + std::fmt::Debug {
    /// The saved key, or `None`: the list is then fetched without one.
    fn open_router(&self) -> Option<Secret>;
}

#[derive(Debug)]
pub struct ModelCatalog {
    open_router_url: String,
    ollama_url: String,
    clock: Arc<dyn Clock>,
    /// The OpenRouter key for provider lists; `None` sends none.
    key: Option<Arc<dyn ListKey>>,
    /// Held only to read or replace an entry, never across a fetch.
    lists: Mutex<Lists>,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self::new(
            OPENROUTER_BASE_URL.to_owned(),
            OLLAMA_BASE_URL.to_owned(),
            Arc::new(MonotonicClock::start()),
        )
    }
}

impl ModelCatalog {
    #[must_use]
    pub fn new(open_router_url: String, ollama_url: String, clock: Arc<dyn Clock>) -> Self {
        Self {
            open_router_url,
            ollama_url,
            clock,
            key: None,
            lists: Mutex::new(Lists::default()),
        }
    }

    /// The catalog asking provider lists with `key` (D-218).
    #[must_use]
    pub fn with_list_key(mut self, key: Arc<dyn ListKey>) -> Self {
        self.key = Some(key);
        self
    }

    /// Answers the picker, from the cache while it is fresh.
    pub async fn answer(&self, query: &ModelQuery) -> Result<ModelListing, CatalogError> {
        match query {
            ModelQuery::OpenRouter => {
                let (models, left_out) = self.open_router().await?;
                Ok(ModelListing::OpenRouter { models, left_out })
            }
            ModelQuery::Ollama => Ok(ModelListing::Ollama {
                models: self.ollama().await?,
            }),
            ModelQuery::Providers { model } => {
                let id = OpenRouterModelId::parse(model).ok_or(CatalogError::NotAModelId)?;
                let (providers, left_out) = self.providers(&id).await?;
                Ok(ModelListing::Providers {
                    model: id.0,
                    providers,
                    left_out,
                })
            }
        }
    }

    /// OpenRouter's tool-capable models, BoardAI's presets first.
    pub async fn open_router(&self) -> Result<Counted<ModelInfo>, ProviderError> {
        let now = self.clock.now();
        let cached = self
            .lock()
            .open_router
            .as_ref()
            .filter(|cached| fresh(cached.at, now, MODEL_LIST_CACHE_MS))
            .map(|cached| cached.value.clone());
        if let Some(value) = cached {
            return Ok(value);
        }
        let url = format!("{}?supported_parameters=tools", self.url("models")?);
        let body = fetch(&url, None, MODEL_LIST_MAX_BYTES, "OpenRouter's model list").await?;
        let (mut models, left_out) = parse::models(&body)?;
        mark_presets(&mut models, |model| preset(&model.id).is_some());
        for model in &models {
            if let Some(preset) = preset(&model.id)
                && let Some(misfit) = listing_misfit(&preset, model)
            {
                tracing::warn!(
                    model = preset.model,
                    ?misfit,
                    "a preset no longer fits its listing"
                );
            }
        }
        let value = (models, left_out);
        self.lock().open_router = Some(Cached {
            at: self.clock.now(),
            value: value.clone(),
        });
        Ok(value)
    }

    /// A model from the last OpenRouter list, without fetching: what a save knows about a
    /// model the picker showed.
    #[must_use]
    pub fn listed(&self, model: &OpenRouterModelId) -> Option<ModelInfo> {
        let lists = self.lock();
        let cached = lists.open_router.as_ref()?;
        cached
            .value
            .0
            .iter()
            .find(|listed| listed.id == model.0)
            .cloned()
    }

    /// Who serves one OpenRouter model.
    pub async fn providers(
        &self,
        model: &OpenRouterModelId,
    ) -> Result<Counted<ModelProvider>, ProviderError> {
        let now = self.clock.now();
        let cached = self
            .lock()
            .providers
            .iter()
            .find(|(id, cached)| id == model && fresh(cached.at, now, MODEL_LIST_CACHE_MS))
            .map(|(_, cached)| cached.value.clone());
        if let Some(value) = cached {
            return Ok(value);
        }
        let url = self.url(&format!("models/{}/endpoints", model.0))?;
        let body = self.provider_list(&url).await?;
        let value = parse::providers(&body)?;
        if let Some(preset) = preset(model.as_str())
            && let Some(misfit) = route_misfit(&preset, &value.0)
        {
            tracing::warn!(
                model = preset.model,
                ?misfit,
                "a preset's route no longer fits"
            );
        }
        let mut lists = self.lock();
        lists.providers.retain(|(id, _)| id != model);
        if lists.providers.len() >= MODEL_PROVIDERS_CACHED_MAX {
            lists.providers.pop_front();
        }
        let at = self.clock.now();
        lists.providers.push_back((
            model.clone(),
            Cached {
                at,
                value: value.clone(),
            },
        ));
        Ok(value)
    }

    /// The local Ollama's models, the default first.
    pub async fn ollama(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let now = self.clock.now();
        let cached = self
            .lock()
            .ollama
            .as_ref()
            .filter(|cached| fresh(cached.at, now, OLLAMA_LIST_CACHE_MS))
            .map(|cached| cached.value.clone());
        if let Some(value) = cached {
            return Ok(value);
        }
        let mut models: Vec<ModelInfo> = ollama_models(&self.ollama_url)
            .await?
            .into_iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                name,
                context_tokens: None,
                price: None,
                reasoning: ModelReasoning::Unknown,
                preset: false,
                route: Vec::new(),
                drawing: Drawing::NotMeasured,
            })
            .collect();
        mark_presets(&mut models, |model| model.id == OLLAMA_DEFAULT_MODEL);
        self.lock().ollama = Some(Cached {
            at: self.clock.now(),
            value: models.clone(),
        });
        Ok(models)
    }

    /// A provider list's body: with the key when one is saved, else, or when OpenRouter refuses
    /// the key, without one (the list is public; only its speeds need the key).
    async fn provider_list(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        const WHAT: &str = "OpenRouter's provider list";
        let Some(authorization) = self.authorization().await else {
            return fetch(url, None, MODEL_PROVIDERS_MAX_BYTES, WHAT).await;
        };
        match fetch(url, Some(&authorization), MODEL_PROVIDERS_MAX_BYTES, WHAT).await {
            Err(ProviderError::Auth { .. }) => {
                tracing::info!("OpenRouter refused the key for a provider list; asking without");
                fetch(url, None, MODEL_PROVIDERS_MAX_BYTES, WHAT).await
            }
            answer @ (Ok(_)
            | Err(
                ProviderError::RateLimited { .. }
                | ProviderError::Overloaded { .. }
                | ProviderError::Network { .. }
                | ProviderError::Timeout { .. }
                | ProviderError::BadOutput { .. }
                | ProviderError::InvalidRequest { .. }
                | ProviderError::Server { .. }
                | ProviderError::Provider { .. }
                | ProviderError::NotConfigured { .. },
            )) => answer,
        }
    }

    /// The saved key's header, read on the blocking pool; `None` without one.
    async fn authorization(&self) -> Option<HeaderValue> {
        let source = Arc::clone(self.key.as_ref()?);
        let key = tokio::task::spawn_blocking(move || source.open_router())
            .await
            .ok()
            .flatten()?;
        bearer(&key).ok()
    }

    fn url(&self, path: &str) -> Result<String, ProviderError> {
        endpoint(&self.open_router_url, path)
    }

    fn lock(&self) -> MutexGuard<'_, Lists> {
        // A panic while holding it can only leave a stale cache entry, which is still a list.
        self.lists
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The effort a talker on this model gets (D-152): thinking off where the model allows it,
/// else the least it takes; `low` when the list doesn't say, which every thinking model takes.
#[must_use]
pub fn talker_effort(reasoning: &ModelReasoning) -> Effort {
    match reasoning {
        ModelReasoning::Never | ModelReasoning::Optional => Effort::None,
        ModelReasoning::Unknown => Effort::Low,
        ModelReasoning::Always { least_effort } => match least_effort.as_str() {
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::ExtraHigh,
            "max" => Effort::Max,
            // `minimal` has no setting of its own; `low` is the nearest a model takes.
            _ => Effort::Low,
        },
    }
}

/// The effort a scribe on this model gets: digests are not latency-bound, so it thinks at
/// `medium` (D-017's scribe effort) or the least the model takes above that, and not at all
/// where the model takes no reasoning setting.
#[must_use]
pub fn scribe_effort(reasoning: &ModelReasoning) -> Effort {
    match reasoning {
        ModelReasoning::Never => Effort::None,
        ModelReasoning::Optional | ModelReasoning::Unknown => Effort::Medium,
        ModelReasoning::Always { .. } => match talker_effort(reasoning) {
            Effort::None | Effort::Low | Effort::Medium => Effort::Medium,
            higher @ (Effort::High | Effort::ExtraHigh | Effort::Max) => higher,
        },
    }
}

fn fresh(at: SessionTime, now: SessionTime, ttl_ms: u64) -> bool {
    now.ms_since(at) < ttl_ms
}

async fn fetch(
    url: &str,
    authorization: Option<&HeaderValue>,
    max_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, ProviderError> {
    get_bounded(
        url,
        authorization,
        max_bytes,
        Duration::from_millis(MODEL_LIST_TIMEOUT_MS),
        what,
    )
    .await
}

/// Marks the presets (with an OpenRouter preset's route and how it draws, D-219) and moves them
/// to the front, in [`PRESETS`] order for OpenRouter.
fn mark_presets(models: &mut [ModelInfo], is_preset: impl Fn(&ModelInfo) -> bool) {
    for model in models.iter_mut() {
        model.preset = is_preset(model);
        model.route = preset(&model.id)
            .filter(|_| model.preset)
            .map(|preset| preset.route.iter().map(|slug| (*slug).to_owned()).collect())
            .unwrap_or_default();
        model.drawing = if model.preset {
            drawing(&model.id)
        } else {
            Drawing::NotMeasured
        };
    }
    let rank = |model: &ModelInfo| {
        PRESETS
            .iter()
            .position(|preset| preset.model == model.id)
            .unwrap_or(if model.preset { 0 } else { PRESETS.len() })
    };
    models.sort_by_key(rank);
}

#[cfg(test)]
mod tests {
    use boardai_core::setup::ModelReasoning;

    use super::{OpenRouterModelId, scribe_effort, talker_effort};
    use crate::limits::MODEL_TEXT_MAX_BYTES;
    use crate::llm::Effort;

    #[test]
    fn model_ids_are_author_and_name_and_never_climb_out_of_the_path() {
        for good in [
            "qwen/qwen3.8-27b",
            "openai/gpt-oss-120b:free",
            "~anthropic/claude-opus-latest",
            "meta-llama/llama-3.3-70b-instruct",
        ] {
            assert!(OpenRouterModelId::parse(good).is_some(), "{good}");
        }
        let long = format!("a/{}", "m".repeat(MODEL_TEXT_MAX_BYTES));
        for bad in [
            "gemma4:latest",
            "../models",
            "qwen/../../key",
            "qwen//x",
            "/qwen/x",
            "qwen/x?y=1",
            "qwen/x#y",
            "qwen/a b",
            "",
            long.as_str(),
        ] {
            assert!(OpenRouterModelId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_talker_thinks_as_little_as_its_model_allows() {
        assert_eq!(talker_effort(&ModelReasoning::Optional), Effort::None);
        assert_eq!(talker_effort(&ModelReasoning::Never), Effort::None);
        assert_eq!(talker_effort(&ModelReasoning::Unknown), Effort::Low);
        let least = |effort: &str| ModelReasoning::Always {
            least_effort: effort.to_owned(),
        };
        assert_eq!(talker_effort(&least("minimal")), Effort::Low);
        assert_eq!(talker_effort(&least("low")), Effort::Low);
        assert_eq!(talker_effort(&least("medium")), Effort::Medium);
    }

    #[test]
    fn a_scribe_thinks_at_medium_where_the_model_thinks_at_all() {
        let least = |effort: &str| ModelReasoning::Always {
            least_effort: effort.to_owned(),
        };
        assert_eq!(scribe_effort(&ModelReasoning::Never), Effort::None);
        assert_eq!(scribe_effort(&ModelReasoning::Optional), Effort::Medium);
        assert_eq!(scribe_effort(&ModelReasoning::Unknown), Effort::Medium);
        assert_eq!(scribe_effort(&least("low")), Effort::Medium);
        assert_eq!(scribe_effort(&least("high")), Effort::High);
    }
}
