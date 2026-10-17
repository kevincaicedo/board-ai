//! OpenRouter's presets (D-152, D-189) against the lists OpenRouter gave for their models on
//! 2026-10-16 (recorded with a key, so with latency medians; trimmed): each preset's thinking
//! is one its model takes, and its route names a provider that serves the model with tools.
//! Planted misfits show the checks fail. Then what each preset sends. No network.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{Conversation, Message, UserPart};
use boardai_core::setup::{Drawing, ModelInfo, ModelProvider};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::llm::catalog::{
    ModelCatalog, OpenRouterModelId, PresetMisfit, listing_misfit, route_misfit,
};
use boardai_runtime::llm::openai::{Dialect, OpenAiCompatibleProvider, OpenAiOptions};
use boardai_runtime::llm::openrouter::{
    CLAUDE_HAIKU_5_5, DEEPSEEK_V4_1_FLASH, GEMINI_3_8_FLASH, GPT_6_LUNA, GPT_OSS_120B, PRESETS,
    Preset, QWEN_3_8_27B, RoleSettings, drawing,
};
use boardai_runtime::llm::{Effort, LlmProvider, LlmRole, RetryPolicy, TalkerConfig, ToolSpec};
use futures_util::StreamExt;
use serde_json::json;
use support::{Scripted, TestServer, fixture};

fn json_body(body: &[u8]) -> Scripted {
    Scripted {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![body.to_vec()],
        chunk_delay: Duration::ZERO,
        hang: false,
    }
}

fn catalog(server: &TestServer) -> ModelCatalog {
    ModelCatalog::new(
        format!("{}/api/v1", server.base_url),
        format!("{}/v1", server.base_url),
        Arc::new(FakeClock::new()),
    )
}

async fn listed() -> Vec<ModelInfo> {
    let server =
        TestServer::start(vec![json_body(&fixture("openrouter/models_measured.json"))]).await;
    catalog(&server).open_router().await.expect("a list").0
}

/// The recorded provider list of each preset's model.
fn endpoints(preset: &Preset) -> &'static str {
    match preset.model {
        "openai/gpt-oss-120b" => "openrouter/endpoints_gpt-oss-120b.json",
        "qwen/qwen3.8-27b" => "openrouter/endpoints_qwen3.8-27b.json",
        "deepseek/deepseek-v4.1-flash" => "openrouter/endpoints_deepseek-v4.1-flash.json",
        "anthropic/claude-haiku-5.5" => "openrouter/endpoints_claude-haiku-5.5.json",
        "openai/gpt-6-luna" => "openrouter/endpoints_gpt-6-luna.json",
        "google/gemini-3.8-flash" => "openrouter/endpoints_gemini-3.8-flash.json",
        other => panic!("no recorded provider list for {other}"),
    }
}

async fn providers(preset: &Preset) -> Vec<ModelProvider> {
    let server = TestServer::start(vec![json_body(&fixture(endpoints(preset)))]).await;
    let id = OpenRouterModelId::parse(preset.model).expect("a model id");
    catalog(&server).providers(&id).await.expect("a list").0
}

#[tokio::test]
async fn every_preset_thinks_as_its_model_allows() {
    let models = listed().await;
    let ids: Vec<&str> = models
        .iter()
        .take(PRESETS.len())
        .map(|m| m.id.as_str())
        .collect();
    let presets: Vec<&str> = PRESETS.iter().map(|preset| preset.model).collect();
    assert_eq!(ids, presets, "the presets come first, in their order");
    for preset in PRESETS {
        let model = models.iter().find(|model| model.id == preset.model);
        let model = model.unwrap_or_else(|| panic!("{} is listed", preset.model));
        assert!(model.preset);
        assert_eq!(listing_misfit(&preset, model), None, "{}", preset.model);
        assert_eq!(model.drawing, drawing(preset.model), "{}", preset.model);
    }
    // D-219: the picker's rows say how each preset drew; a model that isn't one, never measured.
    let checked: Vec<&str> = models
        .iter()
        .filter(|model| matches!(model.drawing, Drawing::Checked { .. }))
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        checked,
        [
            GPT_OSS_120B.model,
            DEEPSEEK_V4_1_FLASH.model,
            GEMINI_3_8_FLASH.model
        ]
    );
    let pro = models
        .iter()
        .find(|model| model.id == "openai/gpt-6-luna-pro");
    assert_eq!(pro.map(|model| &model.drawing), Some(&Drawing::NotMeasured));
}

/// Planted misfits: thinking off for a model that always reasons, or less than it takes.
#[tokio::test]
async fn a_preset_that_thinks_less_than_its_model_takes_is_found() {
    let models = listed().await;
    let gemini = models
        .iter()
        .find(|model| model.id == GEMINI_3_8_FLASH.model)
        .expect("listed");
    let off = Preset {
        talker: RoleSettings {
            effort: Effort::None,
            ..GEMINI_3_8_FLASH.talker
        },
        ..GEMINI_3_8_FLASH
    };
    assert_eq!(
        listing_misfit(&off, gemini),
        Some(PresetMisfit::ThinkingOffRefused)
    );
    let mut never = gemini.clone();
    never.reasoning = boardai_core::setup::ModelReasoning::Never;
    assert_eq!(
        listing_misfit(&DEEPSEEK_V4_1_FLASH, &never),
        Some(PresetMisfit::NoReasoning)
    );
    let mut medium = gemini.clone();
    medium.reasoning = boardai_core::setup::ModelReasoning::Always {
        least_effort: "medium".to_owned(),
    };
    assert_eq!(
        listing_misfit(&GEMINI_3_8_FLASH, &medium),
        Some(PresetMisfit::BelowLeastEffort {
            least: Effort::Medium
        })
    );
}

#[tokio::test]
async fn every_preset_route_serves_its_model_with_tools() {
    for preset in PRESETS {
        let providers = providers(&preset).await;
        assert_eq!(route_misfit(&preset, &providers), None, "{}", preset.model);
    }
}

/// Planted misfits: a provider the model's list doesn't name, and one that serves it without
/// tools (DigitalOcean's gpt-oss-120b, recorded).
#[tokio::test]
async fn a_route_the_list_refuses_is_found() {
    let haiku = providers(&CLAUDE_HAIKU_5_5).await;
    let elsewhere = Preset {
        route: &["cerebras"],
        ..CLAUDE_HAIKU_5_5
    };
    assert_eq!(
        route_misfit(&elsewhere, &haiku),
        Some(PresetMisfit::RouteNotListed {
            slug: "cerebras".to_owned()
        })
    );
    let oss = providers(&GPT_OSS_120B).await;
    let no_tools = Preset {
        route: &["cerebras", "digitalocean"],
        ..GPT_OSS_120B
    };
    assert_eq!(
        route_misfit(&no_tools, &oss),
        Some(PresetMisfit::RouteWithoutTools {
            slug: "digitalocean".to_owned()
        })
    );
}

/// The evidence a route was picked by (HLD §11): OpenRouter's 30-minute medians, recorded.
#[tokio::test]
async fn the_recorded_lists_carry_each_routes_speed() {
    for preset in [
        DEEPSEEK_V4_1_FLASH,
        CLAUDE_HAIKU_5_5,
        GPT_6_LUNA,
        GEMINI_3_8_FLASH,
    ] {
        let providers = providers(&preset).await;
        let first = preset.route[0];
        let routed = providers
            .iter()
            .find(|provider| provider.slug == first)
            .unwrap_or_else(|| panic!("{first} serves {}", preset.model));
        assert!(routed.tools, "{}", preset.model);
        assert!(routed.latency_ms_p50.is_some(), "{}", preset.model);
        assert!(routed.throughput_tokens_per_second_p50.is_some());
    }
}

fn talker(preset: &Preset) -> TalkerConfig {
    TalkerConfig {
        role: LlmRole::Talker,
        model: preset.model.to_owned(),
        effort: preset.talker.effort,
        max_tokens: preset.talker.max_tokens,
        system_prompt: "Answer in one short sentence.".into(),
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the open repository.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    }
}

fn provider(server: &TestServer, preset: &Preset) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        &talker(preset),
        OpenAiOptions {
            base_url: format!("{}/v1", server.base_url),
            api_key: None,
            clock: Arc::new(FakeClock::new()),
            policy: Some(RetryPolicy {
                first_token_deadline: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(5),
                backoff_base: Duration::from_millis(10),
            }),
            dialect: Dialect::OpenRouter {
                route: preset.route(),
            },
        },
    )
    .expect("provider builds")
}

fn question() -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::System {
        text: "Board now:\nmain: empty".into(),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "What is a load balancer?".into(),
        }],
    });
    conversation
}

/// Each preset's request: its thinking, its strict route, the tools, context folded into the
/// user turn (D-160), and a cache request only where the provider caches on request (D-215).
/// The warm-up carries the same fields, so for Claude it writes the cache the answers read.
#[tokio::test]
async fn each_preset_sends_its_thinking_route_and_cache() {
    for preset in PRESETS {
        let text = fixture("openai/openrouter_gpt_oss_text.sse");
        let server = TestServer::start(vec![Scripted::sse(&text, 1), json_body(b"{}")]).await;
        let talker = provider(&server, &preset);
        let _: Vec<_> = talker
            .stream(&question(), CancelToken::new())
            .collect()
            .await;
        let warm_up = talker.warm_up().expect("OpenRouter warms up");
        let _ = warm_up.await;
        let requests = server.requests();
        let (body, warm) = (requests[0].json(), requests[1].json());
        let effort = match preset.talker.effort {
            Effort::None => "none",
            Effort::Low => "low",
            other @ (Effort::Medium | Effort::High | Effort::ExtraHigh | Effort::Max) => {
                panic!("{}: a talker preset at {other:?}", preset.model)
            }
        };
        assert_eq!(
            body["reasoning"],
            json!({"effort": effort, "exclude": true})
        );
        assert_eq!(body.get("reasoning_effort"), None);
        assert_eq!(
            body["provider"],
            json!({"order": preset.route, "allow_fallbacks": false, "require_parameters": true}),
            "{}",
            preset.model
        );
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["max_tokens"], 4096);
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter_map(|message| message["role"].as_str())
            .collect();
        assert_eq!(roles, ["system", "user"], "{}", preset.model);
        let cached = preset.model == CLAUDE_HAIKU_5_5.model;
        let cache = json!({"type": "ephemeral"});
        assert_eq!(body.get("cache_control"), cached.then_some(&cache));
        assert_eq!(warm.get("cache_control"), cached.then_some(&cache));
        assert_eq!(warm["max_tokens"], 1);
        // The system prompt is the warm-up's breakpoint, so the first answer reads what the
        // warm-up wrote.
        let system = if cached {
            json!([{"type": "text", "text": "Answer in one short sentence.", "cache_control": cache}])
        } else {
            json!("Answer in one short sentence.")
        };
        assert_eq!(body["messages"][0]["content"], system, "{}", preset.model);
        assert_eq!(warm["messages"][0]["content"], system, "{}", preset.model);
    }
    assert_eq!(QWEN_3_8_27B.talker.effort, Effort::None);
}
