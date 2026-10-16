//! Ollama's native chat adapter against recorded `/api/chat` streams (gemma4 and qwen3.5 on
//! Ollama 0.32, with two failure shapes cut from them) served by a local HTTP server. No network.
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
use boardai_runtime::limits::{OLLAMA_CONTEXT_TOKENS, OLLAMA_KEEP_ALIVE_SECONDS};
use boardai_runtime::llm::ollama_chat::{OllamaChatOptions, OllamaChatProvider};
use boardai_runtime::llm::{
    ChatEvent, Effort, LlmProvider, LlmRole, ProviderError, RetryPolicy, TalkerConfig, ToolSpec,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{Scripted, TestServer, fixture};

type Events = Vec<Result<ChatEvent, ProviderError>>;

fn config(effort: Effort) -> TalkerConfig {
    TalkerConfig {
        role: LlmRole::Talker,
        model: "gemma4:latest".into(),
        effort,
        max_tokens: 300,
        system_prompt: "Answer in one short sentence.".into(),
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the open repository.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    }
}

fn provider(server: &TestServer, effort: Effort) -> OllamaChatProvider {
    OllamaChatProvider::new(
        &config(effort),
        OllamaChatOptions {
            base_url: format!("{}/v1", server.base_url),
            clock: Arc::new(FakeClock::new()),
            policy: Some(RetryPolicy {
                first_token_deadline: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(5),
                backoff_base: Duration::from_millis(10),
            }),
        },
    )
    .expect("provider builds")
}

/// `body` in `pieces` roughly equal chunks, as Ollama sends it.
fn ndjson(body: &[u8], pieces: usize) -> Scripted {
    let mut scripted = Scripted::sse(body, pieces);
    scripted.headers = vec![("content-type".into(), "application/x-ndjson".into())];
    scripted
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

async fn serve(name: &str, pieces: usize) -> (TestServer, Events) {
    let server = TestServer::start(vec![ndjson(&fixture(name), pieces)]).await;
    let events = provider(&server, Effort::None)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    (server, events)
}

fn text_of(events: &Events) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::TextDelta(text)) => Some(text.as_str()),
            Ok(ChatEvent::ToolCall(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

const ANSWER: &str = "A load balancer distributes incoming network traffic across a group of \
                      backend servers to ensure no single server becomes a bottleneck.";

#[tokio::test]
async fn a_text_answer_streams_with_usage_and_its_native_message() {
    let (_, events) = serve("ollama_chat/text.ndjson", 7).await;
    assert_eq!(text_of(&events), ANSWER);
    let Some(Ok(ChatEvent::Done { stop, usage, raw })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::EndTurn);
    assert_eq!((usage.input_tokens, usage.output_tokens), (26, 23));
    let raw = raw.as_ref().expect("raw");
    assert_eq!(raw.provider, ProviderFamily::Ollama);
    let message: Value = serde_json::from_str(&raw.content_json).expect("json");
    assert_eq!(message, json!({"role": "assistant", "content": ANSWER}));
}

/// Where the chunks split never changes what the consumer sees.
#[tokio::test]
async fn every_chunking_gives_the_same_events() {
    let (_, whole) = serve("ollama_chat/text.ndjson", 1).await;
    for pieces in [2, 3, 13, 64, 500] {
        let (_, split) = serve("ollama_chat/text.ndjson", pieces).await;
        assert_eq!(split, whole, "{pieces} pieces");
    }
}

/// The request asks Ollama for the context window, keeps the model loaded, turns thinking off
/// at effort `None`, and goes to the native endpoint.
#[tokio::test]
async fn the_request_asks_for_the_context_window_and_no_thinking() {
    let (server, _) = serve("ollama_chat/text.ndjson", 1).await;
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/chat");
    assert_eq!(requests[0].header("accept"), Some("application/x-ndjson"));
    let body = requests[0].json();
    assert_eq!(body["options"]["num_ctx"], json!(OLLAMA_CONTEXT_TOKENS));
    assert_eq!(body["options"]["num_predict"], json!(300));
    assert_eq!(body["keep_alive"], json!(OLLAMA_KEEP_ALIVE_SECONDS));
    assert_eq!(body["think"], json!(false));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "Answer in one short sentence."})
    );
    assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
    // Other efforts leave the model's own default: a model that cannot think rejects `true`.
    let server = TestServer::start(vec![ndjson(&fixture("ollama_chat/text.ndjson"), 1)]).await;
    let _: Events = provider(&server, Effort::Low)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert_eq!(server.requests()[0].json().get("think"), None);
}

#[tokio::test]
async fn a_tool_call_keeps_its_id_and_object_arguments() {
    let (_, events) = serve("ollama_chat/tool_call.ndjson", 3).await;
    let calls: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::ToolCall(invocation)) => Some(invocation.clone()),
            Ok(ChatEvent::TextDelta(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id.as_str(), "call_scfq50k4");
    assert_eq!(calls[0].input_json, r#"{"path":"src/main.rs"}"#);
    assert_eq!(
        calls[0].call,
        Ok(ToolCall::ReadFile {
            path: "src/main.rs".into(),
            start_line: None,
            end_line: None
        })
    );
    let Some(Ok(ChatEvent::Done { stop, raw, .. })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    // Ollama says `stop` after a tool call; it is a tool round.
    assert_eq!(*stop, StopReason::ToolUse);
    let message: Value =
        serde_json::from_str(&raw.as_ref().expect("raw").content_json).expect("json");
    assert_eq!(
        message["tool_calls"][0],
        json!({"id": "call_scfq50k4", "function": {"name": "read_file", "arguments": {"path": "src/main.rs"}}})
    );
}

/// The history goes back as Ollama expects it: its own message echoed, a heard prefix rebuilt
/// from its parts, and each tool result naming its tool.
#[tokio::test]
async fn the_history_renders_tool_rounds_and_heard_prefixes() {
    let server = TestServer::start(vec![ndjson(&fixture("ollama_chat/text.ndjson"), 1)]).await;
    let id = ToolCallId::parse("call_scfq50k4").expect("id");
    let native = json!({"role": "assistant", "content": "", "tool_calls": [
        {"id": "call_scfq50k4", "function": {"name": "read_file", "arguments": {"path": "src/main.rs"}}}
    ]});
    let mut conversation = question();
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::ToolUse {
            id: id.clone(),
            name: "read_file".into(),
            input_json: r#"{"path":"src/main.rs"}"#.into(),
        }],
        raw: Some(RawAssistant {
            provider: ProviderFamily::Ollama,
            content_json: native.to_string(),
        }),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::ToolResult {
            id: id.clone(),
            content: "fn main() {}".into(),
            is_error: false,
        }],
    });
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::Text {
            text: "It starts the".into(),
        }],
        raw: None,
    });
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "wait, which file?".into(),
        }],
    });
    let _: Events = provider(&server, Effort::None)
        .stream(&conversation, CancelToken::new())
        .collect()
        .await;
    let messages = server.requests()[0].json()["messages"].clone();
    assert_eq!(messages[2], native);
    assert_eq!(
        messages[3],
        json!({"role": "tool", "content": "fn main() {}", "tool_name": "read_file", "tool_call_id": "call_scfq50k4"})
    );
    assert_eq!(
        messages[4],
        json!({"role": "assistant", "content": "It starts the"})
    );
    assert_eq!(
        messages[5],
        json!({"role": "user", "content": "wait, which file?"})
    );
}

#[tokio::test]
async fn an_answer_stopped_at_its_budget_is_max_tokens() {
    let (_, events) = serve("ollama_chat/length.ndjson", 2).await;
    assert!(!text_of(&events).is_empty());
    let Some(Ok(ChatEvent::Done { stop, .. })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::MaxTokens);
}

/// Thinking is never spoken: a model that spent its whole budget thinking said nothing.
#[tokio::test]
async fn thinking_is_never_spoken() {
    let (_, events) = serve("ollama_chat/thinking.ndjson", 9).await;
    assert_eq!(text_of(&events), "");
    let Some(Ok(ChatEvent::Done { stop, raw, .. })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    assert_eq!(*stop, StopReason::MaxTokens);
    assert_eq!(*raw, None);
}

#[tokio::test]
async fn an_error_line_fails_the_stream_after_the_text_it_followed() {
    let (_, events) = serve("ollama_chat/error_mid_stream.ndjson", 2).await;
    assert_eq!(text_of(&events), "A load balancer");
    assert!(
        matches!(events.last(), Some(Err(ProviderError::Server { status: 500, detail })) if detail.contains("unexpected EOF")),
        "{events:?}"
    );
}

#[tokio::test]
async fn a_stream_cut_before_done_is_a_network_failure() {
    let (_, events) = serve("ollama_chat/truncated.ndjson", 2).await;
    assert!(
        matches!(events.last(), Some(Err(ProviderError::Network { .. }))),
        "{events:?}"
    );
}

#[tokio::test]
async fn an_error_status_keeps_ollamas_message() {
    let server = TestServer::start(vec![Scripted::status(
        404,
        r#"{"error":"model 'gemma9' not found"}"#,
    )])
    .await;
    let events: Events = provider(&server, Effort::None)
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    assert!(
        matches!(events.last(), Some(Err(ProviderError::InvalidRequest { status: Some(404), detail })) if detail == "model 'gemma9' not found"),
        "{events:?}"
    );
}
