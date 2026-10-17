//! The OpenAI-compatible adapter against recorded SSE (real Ollama recordings and OpenRouter
//! shapes) served by a local HTTP server.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{
    AssistantPart, Conversation, Message, ProviderFamily, RawAssistant, UserPart,
};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::ToolCall;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::llm::openai::{Dialect, OpenAiCompatibleProvider, OpenAiOptions};
use boardai_runtime::llm::openrouter::ProviderSlug;
use boardai_runtime::llm::{
    ChatEvent, Effort, LlmProvider, LlmRole, ProviderError, RetryPolicy, TalkerConfig, ToolSpec,
};
use boardai_runtime::settings::Secret;
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{Scripted, TestServer, fixture};

fn config() -> TalkerConfig {
    TalkerConfig {
        role: LlmRole::Talker,
        model: "qwen3.5:2b".into(),
        effort: Effort::Low,
        max_tokens: 300,
        system_prompt: "Answer in one short sentence.".into(),
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the open repository.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    }
}

fn provider(server: &TestServer, api_key: Option<&str>) -> OpenAiCompatibleProvider {
    provider_in(server, api_key, Dialect::Standard)
}

fn provider_in(
    server: &TestServer,
    api_key: Option<&str>,
    dialect: Dialect,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        &config(),
        OpenAiOptions {
            base_url: format!("{}/v1", server.base_url),
            api_key: api_key.map(|key| Secret::new(key.into())),
            clock: Arc::new(FakeClock::new()),
            policy: Some(RetryPolicy {
                first_token_deadline: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(5),
                backoff_base: Duration::from_millis(10),
            }),
            dialect,
        },
    )
    .expect("provider builds")
}

fn question() -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "What is a load balancer?".into(),
        }],
    });
    conversation
}

async fn serve(name: &str) -> (TestServer, Vec<Result<ChatEvent, ProviderError>>) {
    let server = TestServer::start(vec![Scripted::sse(&fixture(name), 5)]).await;
    let events = provider(&server, None)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    (server, events)
}

fn text_of(events: &[Result<ChatEvent, ProviderError>]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::TextDelta(text)) => Some(text.as_str()),
            Ok(ChatEvent::ToolCall(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

fn calls(events: &[Result<ChatEvent, ProviderError>]) -> Vec<ToolCall> {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::ToolCall(invocation)) => invocation.call.clone().ok(),
            Ok(ChatEvent::TextDelta(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

fn stop(events: &[Result<ChatEvent, ProviderError>]) -> StopReason {
    match events.last() {
        Some(Ok(ChatEvent::Done { stop, .. })) => stop.clone(),
        other => panic!("expected Done last, got {other:?}"),
    }
}

#[tokio::test]
async fn ollama_text_streams_with_usage() {
    let (_, events) = serve("openai/ollama_text.sse").await;
    let text = text_of(&events);
    assert!(text.starts_with("A load balancer is a network"), "{text}");
    let Some(Ok(ChatEvent::Done { stop, usage, raw })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::EndTurn);
    assert_eq!((usage.input_tokens, usage.output_tokens), (30, 25));
    let raw = raw.as_ref().expect("raw");
    assert_eq!(raw.provider, ProviderFamily::OpenAiCompatible);
    let message: Value = serde_json::from_str(&raw.content_json).expect("json");
    assert_eq!(message, json!({"role": "assistant", "content": text}));
}

#[tokio::test]
async fn ollama_tool_calls_skip_reasoning_and_parse() {
    let (_, events) = serve("openai/ollama_tool.sse").await;
    assert_eq!(text_of(&events), "");
    assert_eq!(
        calls(&events),
        [ToolCall::ReadFile {
            path: "src/main.rs".into(),
            start_line: None,
            end_line: None
        }]
    );
    assert_eq!(stop(&events), StopReason::ToolUse);
}

#[tokio::test]
async fn fragmented_tool_arguments_are_joined_by_index() {
    let (_, events) = serve("openai/fragmented_tool.sse").await;
    assert_eq!(text_of(&events), "Checking the code.");
    assert_eq!(
        calls(&events),
        [
            ToolCall::SearchCode {
                query: "fn route".into(),
                glob: None
            },
            ToolCall::ListDir { path: "src".into() },
        ]
    );
    let Some(Ok(ChatEvent::Done { usage, raw, .. })) = events.last() else {
        panic!("expected Done");
    };
    assert_eq!((usage.input_tokens, usage.output_tokens), (120, 31));
    let message: Value =
        serde_json::from_str(&raw.as_ref().expect("raw").content_json).expect("json");
    assert_eq!(
        message["tool_calls"][0]["function"]["arguments"],
        r#"{"query": "fn route"}"#
    );
}

#[tokio::test]
async fn length_without_done_marker_is_max_tokens() {
    let (_, events) = serve("openai/length.sse").await;
    assert_eq!(text_of(&events), "The request path");
    assert_eq!(stop(&events), StopReason::MaxTokens);
}

#[tokio::test]
async fn an_upstream_error_before_the_first_token_is_retried_in_its_category() {
    // `{"error":{"code":502}}`: OpenRouter's upstream failed before anything was said, which is
    // retried like an HTTP 502. The test server answers the retries with 500s.
    let (server, events) = serve("openai/error_chunk.sse").await;
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::Server { status: 500, .. }))
    ));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn an_error_after_text_is_never_retried() {
    let server = TestServer::start(vec![Scripted::sse(
        &fixture("openai/openrouter_error_mid_stream.sse"),
        3,
    )])
    .await;
    let events: Vec<_> = provider_in(&server, None, open_router(&["cerebras"]))
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert_eq!(text_of(&events), "A load balancer");
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::Server { status: 500, .. }))
    ));
    assert_eq!(server.requests().len(), 1);
}

fn open_router(route: &[&str]) -> Dialect {
    Dialect::OpenRouter {
        route: route
            .iter()
            .map(|slug| ProviderSlug::parse(slug).expect("slug"))
            .collect(),
    }
}

/// A real gpt-oss-120b answer through OpenRouter (Cerebras, `reasoning.effort = "low"`,
/// recorded 2026-10-16): the reasoning deltas are never spoken.
#[tokio::test]
async fn openrouter_reasoning_is_never_spoken() {
    let server = TestServer::start(vec![Scripted::sse(
        &fixture("openai/openrouter_gpt_oss_text.sse"),
        4,
    )])
    .await;
    let events: Vec<_> = provider_in(&server, Some("sk-or-test"), open_router(&["cerebras"]))
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert_eq!(
        text_of(&events),
        "A load balancer distributes incoming network traffic across multiple servers to \
         improve performance and reliability."
    );
    let Some(Ok(ChatEvent::Done { stop, usage, raw })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::EndTurn);
    assert_eq!((usage.input_tokens, usage.output_tokens), (147, 33));
    assert_eq!(usage.cache_read_input_tokens, 0);
    let raw: Value = serde_json::from_str(&raw.as_ref().expect("raw").content_json).expect("json");
    assert_eq!(raw.get("reasoning"), None);
}

/// A real Qwen3.8 27B tool call through OpenRouter (Cerebras, thinking off, recorded
/// 2026-10-16).
#[tokio::test]
async fn openrouter_tool_calls_parse() {
    let server = TestServer::start(vec![Scripted::sse(
        &fixture("openai/openrouter_qwen_tool.sse"),
        2,
    )])
    .await;
    let events: Vec<_> = provider_in(&server, None, open_router(&[]))
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert_eq!(
        calls(&events),
        [ToolCall::ReadFile {
            path: "src/main.rs".into(),
            start_line: None,
            end_line: None
        }]
    );
    assert_eq!(stop(&events), StopReason::ToolUse);
}

/// OpenRouter's documented request fields (D-152): `reasoning.effort` instead of
/// `reasoning_effort`, and a route as `provider.order` with no fallbacks, whose providers
/// must take every parameter.
#[tokio::test]
async fn openrouter_requests_carry_reasoning_and_the_route() {
    let server = TestServer::start(vec![
        Scripted::sse(&fixture("openai/openrouter_gpt_oss_text.sse"), 1),
        Scripted::sse(&fixture("openai/openrouter_gpt_oss_text.sse"), 1),
    ])
    .await;
    let routed = provider_in(&server, None, open_router(&["cerebras", "groq"]));
    let _: Vec<_> = routed
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    let unrouted = provider_in(&server, None, open_router(&[]));
    let _: Vec<_> = unrouted
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    let requests = server.requests();
    let body = requests[0].json();
    assert_eq!(body["reasoning"], json!({"effort": "low", "exclude": true}));
    assert_eq!(body.get("reasoning_effort"), None);
    assert_eq!(
        body["provider"],
        json!({"order": ["cerebras", "groq"], "allow_fallbacks": false, "require_parameters": true})
    );
    let body = requests[1].json();
    assert_eq!(body["reasoning"], json!({"effort": "low", "exclude": true}));
    assert_eq!(body.get("provider"), None);
}

#[tokio::test]
async fn the_request_uses_chat_completions_shapes() {
    let server =
        TestServer::start(vec![Scripted::sse(&fixture("openai/ollama_text.sse"), 1)]).await;
    let id = ToolCallId::parse("call_1").expect("id");
    let mut conversation = question();
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::ToolUse {
            id: id.clone(),
            name: "read_file".into(),
            input_json: r#"{"path":"src/lb.rs"}"#.into(),
        }],
        raw: Some(RawAssistant {
            provider: ProviderFamily::Anthropic,
            content_json: "[]".into(),
        }),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::ToolResult {
            id,
            content: "not found".into(),
            is_error: true,
        }],
    });
    conversation.push(Message::System {
        text: "Board: lb.".into(),
    });
    let events: Vec<_> = provider(&server, Some("sk-or-test"))
        .stream(&conversation, CancelToken::new())
        .collect()
        .await;
    assert_eq!(stop(&events), StopReason::EndTurn);
    let request = &server.requests()[0];
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.header("authorization"), Some("Bearer sk-or-test"));
    let body = request.json();
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(body.get("reasoning"), None);
    assert_eq!(body.get("provider"), None);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "Answer in one short sentence."},
            {"role": "user", "content": "What is a load balancer?"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"src/lb.rs\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "Error: not found"},
            {"role": "user", "content": "<system-reminder>\nBoard: lb.\n</system-reminder>"},
        ])
    );
}

/// D-160: context is never a system message after the first. Qwen's chat template on Cerebras
/// refuses one ("System message must be at the beginning", every BoardAI request in the
/// 2026-10-16 run): the session's opening context folds into the question, and a run of context
/// before an assistant turn becomes one user turn of its own.
#[tokio::test]
async fn context_folds_into_the_next_user_turn() {
    let server =
        TestServer::start(vec![Scripted::sse(&fixture("openai/ollama_text.sse"), 1)]).await;
    let mut conversation = Conversation::new();
    conversation.push(Message::System {
        text: "Board now:\nmain: empty".into(),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "Explain the TCP handshake.".into(),
        }],
    });
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::Text {
            text: "Three steps.".into(),
        }],
        raw: None,
    });
    conversation.push(Message::System {
        text: "Thread: handshake".into(),
    });
    conversation.push(Message::System {
        text: String::new(),
    });
    conversation.push(Message::System {
        text: "Board now:\nmain: 3 boxes".into(),
    });
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::Text {
            text: "Go on.".into(),
        }],
        raw: None,
    });
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "Why three?".into(),
        }],
    });
    let _: Vec<_> = provider(&server, None)
        .stream(&conversation, CancelToken::new())
        .collect()
        .await;
    let body = server.requests()[0].json();
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["role"].as_str())
        .collect();
    assert_eq!(
        roles,
        ["system", "user", "assistant", "user", "assistant", "user"]
    );
    assert_eq!(
        body["messages"][1]["content"],
        "<system-reminder>\nBoard now:\nmain: empty\n</system-reminder>\n\nExplain the TCP handshake."
    );
    assert_eq!(
        body["messages"][3]["content"],
        "<system-reminder>\nThread: handshake\n</system-reminder>\n\n<system-reminder>\nBoard now:\nmain: 3 boxes\n</system-reminder>"
    );
    assert_eq!(body["messages"][5]["content"], "Why three?");
}

#[tokio::test]
async fn no_key_means_no_authorization_header() {
    let (server, _) = serve("openai/ollama_text.sse").await;
    assert_eq!(server.requests()[0].header("authorization"), None);
}
