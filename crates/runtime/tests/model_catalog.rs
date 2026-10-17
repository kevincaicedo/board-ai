//! The model picker's lists (D-153) against a local fake server serving lists recorded from
//! OpenRouter (2026-10-16, trimmed to six models): what is kept, presets first, the cache and
//! its bounds, and failures in their categories. No network.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;

use boardai_core::setup::{ModelListing, ModelQuery, ModelReasoning};
use boardai_runtime::clock::FakeClock;
use boardai_runtime::limits::{
    MODEL_LIST_CACHE_MS, MODEL_LIST_MAX_BYTES, MODEL_PROVIDERS_CACHED_MAX, OLLAMA_LIST_CACHE_MS,
};
use boardai_runtime::llm::ProviderError;
use boardai_runtime::llm::catalog::{CatalogError, ModelCatalog, OpenRouterModelId};
use support::{Scripted, TestServer, fixture};

fn json(body: &[u8]) -> Scripted {
    Scripted {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![body.to_vec()],
        chunk_delay: std::time::Duration::ZERO,
        hang: false,
    }
}

fn catalog(server: &TestServer, clock: &FakeClock) -> ModelCatalog {
    ModelCatalog::new(
        format!("{}/api/v1", server.base_url),
        format!("{}/v1", server.base_url),
        Arc::new(clock.clone()),
    )
}

fn id(text: &str) -> OpenRouterModelId {
    OpenRouterModelId::parse(text).expect("a model id")
}

#[tokio::test]
async fn openrouter_lists_tool_models_with_the_presets_first() {
    let server = TestServer::start(vec![json(&fixture("openrouter/models.json"))]).await;
    let clock = FakeClock::new();
    let (models, left_out) = catalog(&server, &clock)
        .open_router()
        .await
        .expect("a list");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "openai/gpt-oss-120b",
            "qwen/qwen3.8-27b",
            "xingchen-agi/xing4.0-29b-a4b:free",
            "unbiased/pareto-26.9",
            "meta/muse-wonder",
        ],
        "moondream takes no tools, so it is not a candidate"
    );
    assert_eq!(left_out, 0);
    let presets: Vec<bool> = models.iter().map(|model| model.preset).collect();
    assert_eq!(presets, [true, true, false, false, false]);
    let routes: Vec<&[String]> = models.iter().map(|model| model.route.as_slice()).collect();
    let cerebras = ["cerebras".to_owned()];
    assert_eq!(
        routes,
        [&cerebras[..], &cerebras[..], &[], &[], &[]],
        "only a preset has a route (D-152)"
    );
    let oss = &models[0];
    assert_eq!(oss.name, "OpenAI: gpt-oss-120b");
    assert_eq!(oss.context_tokens, Some(131_072));
    assert_eq!(
        oss.price.map(|price| (
            price.input_micro_usd_per_million_tokens,
            price.output_micro_usd_per_million_tokens
        )),
        Some((150_000, 600_000))
    );
    let low = ModelReasoning::Always {
        least_effort: "low".to_owned(),
    };
    assert_eq!(oss.reasoning, low);
    assert_eq!(models[1].reasoning, ModelReasoning::Optional);
    assert_eq!(models[3].reasoning, ModelReasoning::Never);
    let minimal = ModelReasoning::Always {
        least_effort: "minimal".to_owned(),
    };
    assert_eq!(models[4].reasoning, minimal);
    let requests = server.requests();
    assert_eq!(
        requests[0].path,
        "/api/v1/models?supported_parameters=tools"
    );
    assert_eq!(
        requests[0].header("authorization"),
        None,
        "lists are public"
    );
}

#[tokio::test]
async fn a_list_is_reused_while_fresh_then_fetched_again() {
    let list = fixture("openrouter/models.json");
    let server = TestServer::start(vec![json(&list), json(&list)]).await;
    let clock = FakeClock::new();
    let catalog = catalog(&server, &clock);
    assert!(catalog.open_router().await.is_ok());
    clock.advance_ms(MODEL_LIST_CACHE_MS - 1);
    assert!(catalog.open_router().await.is_ok());
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        catalog
            .listed(&id("qwen/qwen3.8-27b"))
            .map(|model| model.preset),
        Some(true)
    );
    assert_eq!(catalog.listed(&id("qwen/unlisted")), None);
    clock.advance_ms(1);
    assert!(catalog.open_router().await.is_ok());
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn one_models_providers_come_with_their_limits() {
    let server = TestServer::start(vec![json(&fixture(
        "openrouter/endpoints_qwen3.8-27b.json",
    ))])
    .await;
    let clock = FakeClock::new();
    let query = ModelQuery::Providers {
        model: "qwen/qwen3.8-27b".to_owned(),
    };
    let answer = catalog(&server, &clock).answer(&query).await;
    let Ok(ModelListing::Providers {
        model,
        providers,
        left_out,
    }) = answer
    else {
        panic!("a provider list, got {answer:?}");
    };
    assert_eq!(model, "qwen/qwen3.8-27b");
    assert_eq!((providers.len(), left_out), (20, 0));
    let cerebras = providers
        .iter()
        .find(|provider| provider.name == "Cerebras")
        .expect("Cerebras serves it");
    assert_eq!(cerebras.slug, "cerebras/fp16");
    assert_eq!(cerebras.context_tokens, Some(65_536));
    assert_eq!(cerebras.max_output_tokens, Some(32_768));
    assert_eq!(cerebras.quantization.as_deref(), Some("fp16"));
    assert!(cerebras.tools);
    // Recorded without a key: OpenRouter's public list gives no speeds, for any provider, so
    // the settings page leaves its first-token and speed columns out (D-153).
    assert!(providers.iter().all(|provider| {
        provider.latency_ms_p50.is_none() && provider.throughput_tokens_per_second_p50.is_none()
    }));
    let unknown = providers
        .iter()
        .find(|provider| provider.name == "DekaLLM")
        .expect("listed");
    assert_eq!(
        unknown.quantization, None,
        "`unknown` is not a quantization"
    );
    assert_eq!(
        server.requests()[0].path,
        "/api/v1/models/qwen/qwen3.8-27b/endpoints"
    );
}

#[tokio::test]
async fn provider_lists_cached_are_bounded_oldest_out() {
    let body = fixture("openrouter/endpoints_qwen3.8-27b.json");
    let script = vec![json(&body); MODEL_PROVIDERS_CACHED_MAX + 2];
    let server = TestServer::start(script).await;
    let clock = FakeClock::new();
    let catalog = catalog(&server, &clock);
    for index in 0..=MODEL_PROVIDERS_CACHED_MAX {
        let model = id(&format!("author/model-{index}"));
        assert!(catalog.providers(&model).await.is_ok());
    }
    let newest = id(&format!("author/model-{MODEL_PROVIDERS_CACHED_MAX}"));
    assert!(catalog.providers(&newest).await.is_ok());
    assert_eq!(server.requests().len(), MODEL_PROVIDERS_CACHED_MAX + 1);
    assert!(catalog.providers(&id("author/model-0")).await.is_ok());
    assert_eq!(
        server.requests().len(),
        MODEL_PROVIDERS_CACHED_MAX + 2,
        "the oldest was forgotten"
    );
}

#[tokio::test]
async fn a_bad_model_id_never_reaches_the_network() {
    let server = TestServer::start(Vec::new()).await;
    let clock = FakeClock::new();
    for model in ["../key", "qwen/../../x", "gemma4:latest"] {
        let query = ModelQuery::Providers {
            model: model.to_owned(),
        };
        assert_eq!(
            catalog(&server, &clock).answer(&query).await,
            Err(CatalogError::NotAModelId)
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn failures_keep_their_category_and_nothing_is_cached() {
    let oversized = vec![b' '; MODEL_LIST_MAX_BYTES + 1];
    let server = TestServer::start(vec![
        Scripted::status(503, r#"{"error":{"message":"down"}}"#),
        json(&oversized),
        json(b"<html>"),
        json(&fixture("openrouter/models.json")),
    ])
    .await;
    let clock = FakeClock::new();
    let catalog = catalog(&server, &clock);
    assert!(matches!(
        catalog.open_router().await,
        Err(ProviderError::Server { status: 503, .. })
    ));
    assert!(matches!(
        catalog.open_router().await,
        Err(ProviderError::BadOutput { .. })
    ));
    assert!(matches!(
        catalog.open_router().await,
        Err(ProviderError::BadOutput { .. })
    ));
    assert!(catalog.open_router().await.is_ok());
}

#[tokio::test]
async fn ollama_lists_its_models_with_the_default_first() {
    let tags = br#"{"models":[{"name":"qwen3.5:2b"},{"name":"gemma4:latest"}]}"#;
    let server = TestServer::start(vec![json(tags), json(tags)]).await;
    let clock = FakeClock::new();
    let catalog = catalog(&server, &clock);
    let answer = catalog.answer(&ModelQuery::Ollama).await;
    let Ok(ModelListing::Ollama { models }) = answer else {
        panic!("an Ollama list, got {answer:?}");
    };
    let listed: Vec<(&str, bool)> = models
        .iter()
        .map(|model| (model.id.as_str(), model.preset))
        .collect();
    assert_eq!(listed, [("gemma4:latest", true), ("qwen3.5:2b", false)]);
    assert_eq!(models[0].reasoning, ModelReasoning::Unknown);
    assert_eq!(server.requests()[0].path, "/api/tags");
    clock.advance_ms(OLLAMA_LIST_CACHE_MS - 1);
    assert!(catalog.ollama().await.is_ok());
    assert_eq!(server.requests().len(), 1);
    clock.advance_ms(1);
    assert!(catalog.ollama().await.is_ok());
    assert_eq!(server.requests().len(), 2);
}
