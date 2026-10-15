//! The Anthropic adapter against recorded SSE fixtures served by a local HTTP server.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use boardai_core::history::{
    AssistantPart, Conversation, Message, ProviderFamily, RawAssistant, UserPart,
};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::{InvalidToolInput, ToolCall};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::llm::anthropic::{AnthropicOptions, AnthropicProvider};
use boardai_runtime::llm::{
    ChatEvent, LlmProvider, ProviderError, RetryPolicy, TalkerConfig, TimeoutKind, ToolSpec,
};
use boardai_runtime::settings::Secret;
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{Scripted, TestServer, fixture};

const TEST_KEY: &str = "sk-ant-test-key-never-real";

fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "read_file".into(),
        description: "Read lines of a file in the open repository.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        }),
    }]
}

fn policy() -> RetryPolicy {
    RetryPolicy {
        first_token_deadline: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(5),
        backoff_base: Duration::from_millis(10),
    }
}

fn provider_with(server: &TestServer, policy: RetryPolicy) -> AnthropicProvider {
    let config = TalkerConfig::talker("You are the BoardAI test talker.".into(), tools());
    AnthropicProvider::new(
        &config,
        AnthropicOptions {
            base_url: Some(server.base_url.clone()),
            api_key: Secret::new(TEST_KEY.into()),
            clock: Arc::new(FakeClock::new()),
            policy: Some(policy),
        },
    )
    .expect("provider builds")
}

fn question() -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "How does the request path work?".into(),
        }],
    });
    conversation
}

async fn run(
    server: &TestServer,
    conversation: &Conversation,
) -> Vec<Result<ChatEvent, ProviderError>> {
    provider_with(server, policy())
        .stream(conversation, CancelToken::new())
        .collect()
        .await
}

async fn serve_fixture(name: &str) -> (TestServer, Vec<Result<ChatEvent, ProviderError>>) {
    let server = TestServer::start(vec![Scripted::sse(&fixture(name), 7)]).await;
    let events = run(&server, &question()).await;
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

fn done(events: &[Result<ChatEvent, ProviderError>]) -> (StopReason, Value) {
    match events.last() {
        Some(Ok(ChatEvent::Done { stop, raw, .. })) => {
            let raw = raw.as_ref().expect("raw content");
            assert_eq!(raw.provider, ProviderFamily::Anthropic);
            (
                stop.clone(),
                serde_json::from_str(&raw.content_json).expect("raw is JSON"),
            )
        }
        other => panic!("expected Done last, got {other:?}"),
    }
}

fn tool_calls(
    events: &[Result<ChatEvent, ProviderError>],
) -> Vec<(String, Result<ToolCall, InvalidToolInput>)> {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::ToolCall(invocation)) => {
                Some((invocation.id.as_str().to_owned(), invocation.call.clone()))
            }
            Ok(ChatEvent::TextDelta(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

#[tokio::test]
async fn plain_text_streams_and_completes_with_usage_and_raw_content() {
    let (server, events) = serve_fixture("anthropic/plain_text.sse").await;
    assert_eq!(
        text_of(&events),
        "A load balancer spreads requests across servers — é ✓."
    );
    let Some(Ok(ChatEvent::Done { stop, usage, .. })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::EndTurn);
    assert_eq!((usage.input_tokens, usage.output_tokens), (18, 14));
    assert_eq!(usage.cache_read_input_tokens, 2_311);
    let (_, raw) = done(&events);
    assert_eq!(
        raw,
        json!([{"type": "text", "text": "A load balancer spreads requests across servers — é ✓."}])
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn the_request_carries_the_session_setup() {
    let (server, _) = serve_fixture("anthropic/plain_text.sse").await;
    let request = &server.requests()[0];
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header("x-api-key"), Some(TEST_KEY));
    assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        request.header("anthropic-beta"),
        Some("server-side-fallback-2026-07-01")
    );
    let body = request.json();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 16_000);
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(body["tools"][0]["eager_input_streaming"], true);
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
    assert_eq!(body["thinking"], json!({"type": "adaptive"}));
    assert_eq!(body["output_config"], json!({"effort": "low"}));
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(
        body["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "How does the request path work?"}]}])
    );
}

#[tokio::test]
async fn tool_input_streams_in_fragments_and_is_validated_before_use() {
    let (_, events) = serve_fixture("anthropic/tool_use.sse").await;
    assert_eq!(text_of(&events), "Let me open the balancer.");
    let calls = tool_calls(&events);
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        (
            "toolu_01ReadLb".into(),
            Ok(ToolCall::ReadFile {
                path: "src/lb.rs".into(),
                start_line: Some(1),
                end_line: Some(40)
            })
        )
    );
    assert!(matches!(
        calls[1].1,
        Err(InvalidToolInput::Malformed { .. })
    ));
    let (stop, raw) = done(&events);
    assert_eq!(stop, StopReason::ToolUse);
    assert_eq!(
        raw[1]["input"],
        json!({"path": "src/lb.rs", "start_line": 1, "end_line": 40})
    );
    assert_eq!(raw[2]["input"], json!({}));
}

#[tokio::test]
async fn thinking_blocks_are_kept_for_the_echo_but_never_spoken() {
    let (_, events) = serve_fixture("anthropic/thinking.sse").await;
    assert_eq!(text_of(&events), "Requests reach the balancer first.");
    let (stop, raw) = done(&events);
    assert_eq!(stop, StopReason::EndTurn);
    assert_eq!(
        raw,
        json!([
            {"type": "thinking", "thinking": "", "signature": "EqQBCgIYAhIMz8xKq7Rk1vXw2dTqbGgwxYmRkZjQ5"},
            {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpP"},
            {"type": "text", "text": "Requests reach the balancer first."},
        ])
    );
}

#[tokio::test]
async fn a_refusal_is_a_refusal_and_its_tool_calls_never_run() {
    let (_, events) = serve_fixture("anthropic/refusal.sse").await;
    assert_eq!(text_of(&events), "Sure, the first step");
    let calls = tool_calls(&events);
    assert_eq!(calls.len(), 1);
    assert!(matches!(
        calls[0].1,
        Err(InvalidToolInput::Malformed { .. })
    ));
    assert_eq!(done(&events).0, StopReason::Refusal);
}

#[tokio::test]
async fn max_tokens_cuts_a_tool_call_that_is_reported_but_not_run() {
    let (_, events) = serve_fixture("anthropic/max_tokens.sse").await;
    let calls = tool_calls(&events);
    assert_eq!(calls.len(), 1);
    assert!(matches!(
        calls[0].1,
        Err(InvalidToolInput::Malformed { .. })
    ));
    let (stop, raw) = done(&events);
    assert_eq!(stop, StopReason::MaxTokens);
    assert_eq!(
        raw[1],
        json!({"type": "tool_use", "id": "toolu_01CutOff", "name": "read_file", "input": {}})
    );
}

#[tokio::test]
async fn a_server_side_fallback_keeps_only_text_before_the_switch() {
    let (_, events) = serve_fixture("anthropic/fallback.sse").await;
    assert_eq!(text_of(&events), "Here is how the cache works.");
    let (_, raw) = done(&events);
    let kinds: Vec<&str> = raw
        .as_array()
        .expect("array")
        .iter()
        .map(|block| block["type"].as_str().expect("type"))
        .collect();
    assert_eq!(kinds, ["text", "fallback", "thinking", "text"]);
    assert_eq!(raw[2]["signature"], "EqQBFallbackModel");
}

#[tokio::test]
async fn an_error_after_the_first_token_is_returned_and_never_retried() {
    let script = vec![
        Scripted::sse(&fixture("anthropic/error_mid_stream.sse"), 3),
        Scripted::sse(&fixture("anthropic/plain_text.sse"), 1),
    ];
    let server = TestServer::start(script).await;
    let events = run(&server, &question()).await;
    assert_eq!(text_of(&events), "Hello");
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::Overloaded { .. }))
    ));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn failures_before_the_first_token_are_retried_twice() {
    let script = vec![
        Scripted::sse(&fixture("anthropic/error_before_token.sse"), 2),
        Scripted::status(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ),
        Scripted::sse(&fixture("anthropic/plain_text.sse"), 3),
    ];
    let server = TestServer::start(script).await;
    let events = run(&server, &question()).await;
    assert_eq!(done(&events).0, StopReason::EndTurn);
    assert_eq!(server.requests().len(), 3);
    assert_eq!(server.requests()[0].body, server.requests()[2].body);
}

#[tokio::test]
async fn retries_stop_after_two() {
    let overloaded =
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let script = vec![Scripted::status(529, overloaded); 4];
    let server = TestServer::start(script).await;
    let events = run(&server, &question()).await;
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], Err(ProviderError::Overloaded { detail, .. }) if detail == "Overloaded")
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn unauthorized_is_an_auth_failure_that_is_not_retried() {
    let body =
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let server = TestServer::start(vec![Scripted::status(401, body)]).await;
    let events = run(&server, &question()).await;
    let [Err(error)] = events.as_slice() else {
        panic!("expected one error, got {events:?}");
    };
    assert!(matches!(error, ProviderError::Auth { .. }));
    let failure = error.to_talker_failure();
    assert!(!failure.detail.contains(TEST_KEY));
    assert!(!format!("{error:?}").contains(TEST_KEY));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rate_limits_wait_out_retry_after() {
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let script = vec![
        Scripted::status(429, body).with_header("retry-after", "1"),
        Scripted::sse(&fixture("anthropic/plain_text.sse"), 2),
    ];
    let server = TestServer::start(script).await;
    let started = Instant::now();
    let events = run(&server, &question()).await;
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(done(&events).0, StopReason::EndTurn);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_retry_after_beyond_the_limit_is_returned_with_its_wait() {
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let script = vec![Scripted::status(429, body).with_header("retry-after", "120")];
    let server = TestServer::start(script).await;
    let events = run(&server, &question()).await;
    let [Err(ProviderError::RateLimited { retry_after, .. })] = events.as_slice() else {
        panic!("expected a rate limit, got {events:?}");
    };
    assert_eq!(*retry_after, Some(Duration::from_secs(120)));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn malformed_output_is_bad_output() {
    let (_, events) = serve_fixture("anthropic/malformed.sse").await;
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::BadOutput { .. }))
    ));
}

#[tokio::test]
async fn a_stream_that_ends_early_is_a_network_failure() {
    let body = fixture("anthropic/plain_text.sse");
    let cut = body.len() - 60;
    let script = vec![Scripted::sse(&body[..cut], 2)];
    let server = TestServer::start(script).await;
    let events = run(&server, &question()).await;
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::Network { .. }))
    ));
}

/// The plain-text fixture up to and including its first text delta.
fn first_token_only() -> Vec<u8> {
    let body = fixture("anthropic/plain_text.sse");
    let marker = b"A load balancer \"}}\n\n";
    let end = body
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("marker")
        + marker.len();
    body[..end].to_vec()
}

#[tokio::test]
async fn dropping_the_stream_closes_the_connection() {
    let server = TestServer::start(vec![Scripted::sse(&first_token_only(), 1).hanging()]).await;
    let provider = provider_with(&server, policy());
    let mut stream = provider.stream(&question(), CancelToken::new());
    let first = stream.next().await;
    assert!(matches!(first, Some(Ok(ChatEvent::TextDelta(_)))));
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), server.client_disconnected())
        .await
        .expect("the server saw the client go away");
}

#[tokio::test]
async fn the_cancel_token_ends_the_stream_quietly() {
    let server = TestServer::start(vec![Scripted::sse(&first_token_only(), 1).hanging()]).await;
    let provider = provider_with(&server, policy());
    let cancel = CancelToken::new();
    let mut stream = provider.stream(&question(), cancel.clone());
    assert!(matches!(
        stream.next().await,
        Some(Ok(ChatEvent::TextDelta(_)))
    ));
    cancel.cancel();
    let rest = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
    assert_eq!(rest.expect("ends promptly"), None);
}

#[tokio::test]
async fn no_first_token_before_the_deadline_is_a_timeout() {
    let start = fixture("anthropic/error_before_token.sse");
    let first_event_end = start
        .windows(2)
        .position(|window| window == b"\n\n")
        .expect("an event")
        + 2;
    let only_start = &start[..first_event_end];
    let server = TestServer::start(vec![Scripted::sse(only_start, 1).hanging()]).await;
    let tight = RetryPolicy {
        first_token_deadline: Duration::from_millis(300),
        ..policy()
    };
    let started = Instant::now();
    let events: Vec<_> = provider_with(&server, tight)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        events.last(),
        Some(&Err(ProviderError::Timeout {
            kind: TimeoutKind::FirstToken
        }))
    );
}

#[tokio::test]
async fn a_stream_that_stalls_after_starting_times_out_as_idle() {
    let server = TestServer::start(vec![Scripted::sse(&first_token_only(), 1).hanging()]).await;
    let tight = RetryPolicy {
        idle_timeout: Duration::from_millis(300),
        ..policy()
    };
    let events: Vec<_> = provider_with(&server, tight)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert_eq!(text_of(&events), "A load balancer ");
    assert_eq!(
        events.last(),
        Some(&Err(ProviderError::Timeout {
            kind: TimeoutKind::Idle
        }))
    );
}

#[tokio::test]
async fn an_unreachable_server_is_a_network_failure_after_retries() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let config = TalkerConfig::talker(String::new(), Vec::new());
    let provider = AnthropicProvider::new(
        &config,
        AnthropicOptions {
            base_url: Some(format!("http://{address}")),
            api_key: Secret::new(TEST_KEY.into()),
            clock: Arc::new(FakeClock::new()),
            policy: Some(policy()),
        },
    )
    .expect("provider builds");
    let events: Vec<_> = provider
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert!(matches!(
        events.as_slice(),
        [Err(ProviderError::Network { .. })]
    ));
}

#[tokio::test]
async fn history_is_echoed_and_context_follows_the_placement_rules() {
    let server =
        TestServer::start(vec![Scripted::sse(&fixture("anthropic/plain_text.sse"), 1)]).await;
    let raw = r#"[{"type":"thinking","thinking":"","signature":"sig"},{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"src/lb.rs"}}]"#;
    let mut conversation = question();
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::ToolUse {
            id: ToolCallId::parse("toolu_1").expect("id"),
            name: "read_file".into(),
            input_json: r#"{"path":"src/lb.rs"}"#.into(),
        }],
        raw: Some(RawAssistant {
            provider: ProviderFamily::Anthropic,
            content_json: raw.into(),
        }),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::ToolResult {
            id: ToolCallId::parse("toolu_1").expect("id"),
            content: "fn route() {}".into(),
            is_error: false,
        }],
    });
    conversation.push(Message::System {
        text: "Board: lb, api.".into(),
    });
    let events = run(&server, &conversation).await;
    assert_eq!(done(&events).0, StopReason::EndTurn);
    let body = server.requests()[0].json();
    assert_eq!(
        body["messages"][1]["content"],
        serde_json::from_str::<Value>(raw).expect("raw")
    );
    assert_eq!(
        body["messages"][2],
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "fn route() {}"}]})
    );
    assert_eq!(
        body["messages"][3],
        json!({"role": "system", "content": [{"type": "text", "text": "Board: lb, api."}]})
    );
}

#[tokio::test]
async fn an_assistant_prefill_is_refused_before_anything_is_sent() {
    let server = TestServer::start(Vec::new()).await;
    let mut conversation = question();
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::Text {
            text: "So the".into(),
        }],
        raw: None,
    });
    let events = run(&server, &conversation).await;
    assert!(matches!(
        events.as_slice(),
        [Err(ProviderError::InvalidRequest { status: None, .. })]
    ));
    assert!(server.requests().is_empty());
}
