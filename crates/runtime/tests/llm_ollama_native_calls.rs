//! gemma4's calls written in its own syntax as text, read back into calls by the native Ollama
//! adapter (D-191), and tool rounds sent back so gemma4 keeps its turn open (D-190). The
//! streams are gemma4:latest on Ollama 0.40.5 (`fixtures/ollama_chat/native_*.ndjson`); the
//! unknown, malformed and unclosed ones are cut from `native_call.ndjson`. No network.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{
    AssistantPart, Conversation, Message, ProviderFamily, RawAssistant, UserPart,
};
use boardai_core::ids::ToolCallId;
use boardai_core::input::StopReason;
use boardai_core::tools::{InvalidToolInput, ToolCall, ToolInvocation};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::llm::ollama_chat::{OllamaChatOptions, OllamaChatProvider};
use boardai_runtime::llm::{
    ChatEvent, Effort, LlmProvider, LlmRole, ProviderError, RetryPolicy, TalkerConfig, ToolSpec,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{Scripted, TestServer, fixture};

type Events = Vec<Result<ChatEvent, ProviderError>>;

const SAID: &str = "I see. I will call list_dir again.\n\n";

fn tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "A talker tool.".into(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

fn config(model: &str, tools: Vec<ToolSpec>) -> TalkerConfig {
    TalkerConfig {
        role: LlmRole::Talker,
        model: model.into(),
        effort: Effort::None,
        max_tokens: 300,
        system_prompt: "Answer in one short sentence.".into(),
        tools,
    }
}

fn provider(server: &TestServer, config: &TalkerConfig) -> OllamaChatProvider {
    OllamaChatProvider::new(
        config,
        OllamaChatOptions {
            base_url: format!("{}/v1", server.base_url),
            api_key: None,
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

fn gemma() -> TalkerConfig {
    config(
        "gemma4:latest",
        vec![tool("read_file"), tool("list_dir"), tool("search_code")],
    )
}

fn question() -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "Walk me through this repo".into(),
        }],
    });
    conversation
}

async fn serve_body(body: &[u8], pieces: usize, config: &TalkerConfig) -> Events {
    let mut scripted = Scripted::sse(body, pieces);
    scripted.headers = vec![("content-type".into(), "application/x-ndjson".into())];
    let server = TestServer::start(vec![scripted]).await;
    provider(&server, config)
        .stream(&question(), CancelToken::new())
        .collect()
        .await
}

async fn serve(name: &str, pieces: usize) -> Events {
    serve_body(
        &fixture(&format!("ollama_chat/{name}.ndjson")),
        pieces,
        &gemma(),
    )
    .await
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

fn calls_of(events: &Events) -> Vec<ToolInvocation> {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::ToolCall(invocation)) => Some(invocation.clone()),
            Ok(ChatEvent::TextDelta(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

fn done_of(events: &Events) -> (StopReason, Value) {
    let Some(Ok(ChatEvent::Done { stop, raw, .. })) = events.last() else {
        panic!("expected Done, got {events:?}");
    };
    let raw = raw.as_ref().expect("a native message");
    assert_eq!(raw.provider, ProviderFamily::Ollama);
    (
        stop.clone(),
        serde_json::from_str(&raw.content_json).expect("json"),
    )
}

fn list_dir(path: &str) -> Result<ToolCall, InvalidToolInput> {
    Ok(ToolCall::ListDir { path: path.into() })
}

/// The call gemma4 wrote as text runs as a call, however the stream is cut: the text it spoke
/// keeps the prose that names `list_dir`, and the stored message holds the call as a call.
#[tokio::test]
async fn a_call_written_in_gemmas_syntax_becomes_a_call() {
    for pieces in [1, 2, 3, 7, 64] {
        let events = serve("native_call", pieces).await;
        assert_eq!(text_of(&events), SAID, "{pieces} pieces");
        let calls = calls_of(&events);
        assert_eq!(calls.len(), 1, "{pieces} pieces");
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].input_json, r#"{"path":"."}"#);
        assert_eq!(calls[0].call, list_dir("."));
        let (stop, message) = done_of(&events);
        assert_eq!(stop, StopReason::ToolUse);
        assert_eq!(
            message,
            json!({"role": "assistant", "content": SAID, "tool_calls": [
                {"id": calls[0].id.as_str(), "function": {"name": "list_dir", "arguments": {"path": "."}}}
            ]})
        );
    }
}

#[tokio::test]
async fn two_calls_keep_their_order_and_distinct_ids() {
    let events = serve("native_calls_two", 3).await;
    let calls = calls_of(&events);
    assert_eq!(
        calls
            .iter()
            .map(|call| call.call.clone())
            .collect::<Vec<_>>(),
        [list_dir("crates"), list_dir("docs")]
    );
    assert_ne!(calls[0].id, calls[1].id);
    assert!(!text_of(&events).contains("list_dir"));
    assert_eq!(done_of(&events).0, StopReason::ToolUse);
}

/// Text that isn't gemma's call syntax for an offered tool is spoken as before (and the core's
/// D-143 and D-179 handling still applies): prose naming a tool, a tag, an unknown tool.
#[tokio::test]
async fn prose_tags_and_unknown_tools_are_left_as_text() {
    for name in [
        "native_call_prose",
        "native_call_tag",
        "native_call_unknown",
    ] {
        let events = serve(name, 2).await;
        assert!(calls_of(&events).is_empty(), "{name}");
        assert_eq!(done_of(&events).0, StopReason::EndTurn, "{name}");
    }
    let events = serve("native_call_unknown", 2).await;
    assert_eq!(
        text_of(&events),
        format!("{SAID}list_files{{path:<|\"|>.<|\"|>}}")
    );
}

/// Malformed arguments, or a call still open when the response ends: never run, never spoken.
#[tokio::test]
async fn a_malformed_or_unclosed_call_is_dropped() {
    for name in ["native_call_malformed", "native_call_unclosed"] {
        let events = serve(name, 2).await;
        assert_eq!(text_of(&events), SAID, "{name}");
        assert!(calls_of(&events).is_empty(), "{name}");
        let (stop, message) = done_of(&events);
        assert_eq!(stop, StopReason::EndTurn, "{name}");
        assert_eq!(message, json!({"role": "assistant", "content": SAID}));
    }
}

/// Arguments read from text get the same checks as a structured call's: a wrong field is an
/// invalid call the core answers with an error, and at the token limit nothing runs.
#[tokio::test]
async fn read_arguments_get_the_checks_a_structured_call_gets() {
    let recorded = String::from_utf8(fixture("ollama_chat/native_call.ndjson")).expect("utf-8");
    let wrong_field = recorded.replace(r#""content":"path""#, r#""content":"folder""#);
    let events = serve_body(wrong_field.as_bytes(), 2, &gemma()).await;
    let calls = calls_of(&events);
    assert_eq!(calls.len(), 1);
    assert!(calls[0].call.is_err(), "{:?}", calls[0].call);
    let cut = recorded.replace(r#""done_reason":"stop""#, r#""done_reason":"length""#);
    let events = serve_body(cut.as_bytes(), 2, &gemma()).await;
    let calls = calls_of(&events);
    assert!(matches!(
        calls[0].call,
        Err(InvalidToolInput::Malformed { .. })
    ));
    assert_eq!(done_of(&events).0, StopReason::MaxTokens);
}

/// Only a request that offered tools reads calls from text.
#[tokio::test]
async fn without_tools_the_text_is_untouched() {
    let body = fixture("ollama_chat/native_call.ndjson");
    let events = serve_body(&body, 2, &config("gemma4:latest", Vec::new())).await;
    assert_eq!(
        text_of(&events),
        format!("{SAID}list_dir{{path:<|\"|>.<|\"|>}}")
    );
    assert!(calls_of(&events).is_empty());
}

/// A conversation whose assistant spoke and then called a tool, with the tool's result.
fn tool_round() -> Conversation {
    let id = ToolCallId::parse("call_0").expect("id");
    let native = json!({"role": "assistant", "content": SAID, "tool_calls": [
        {"id": "call_0", "function": {"name": "list_dir", "arguments": {"path": "."}}}
    ]});
    let mut conversation = question();
    conversation.push(Message::Assistant {
        parts: vec![
            AssistantPart::Text { text: SAID.into() },
            AssistantPart::ToolUse {
                id: id.clone(),
                name: "list_dir".into(),
                input_json: r#"{"path":"."}"#.into(),
            },
        ],
        raw: Some(RawAssistant {
            provider: ProviderFamily::Ollama,
            content_json: native.to_string(),
        }),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::ToolResult {
            id,
            content: "crates/\ndocs/\n".into(),
            is_error: false,
        }],
    });
    conversation
}

async fn sent_messages(model: &str) -> Vec<Value> {
    let mut scripted = Scripted::sse(&fixture("ollama_chat/text.ndjson"), 1);
    scripted.headers = vec![("content-type".into(), "application/x-ndjson".into())];
    let server = TestServer::start(vec![scripted]).await;
    let _: Events = provider(&server, &config(model, vec![tool("list_dir")]))
        .stream(&tool_round(), CancelToken::new())
        .collect()
        .await;
    let body = server.requests()[0].json();
    body["messages"].as_array().cloned().unwrap_or_default()
}

/// gemma4's renderer writes a message's text after its tool results and closes the turn, so
/// its text goes first in a message of its own and the calls follow with none; other models'
/// renderers take the message as stored.
#[tokio::test]
async fn gemma4_gets_the_text_before_the_calls() {
    let call =
        json!({"id": "call_0", "function": {"name": "list_dir", "arguments": {"path": "."}}});
    let result = json!({"role": "tool", "content": "crates/\ndocs/\n", "tool_name": "list_dir", "tool_call_id": "call_0"});
    for model in ["gemma4:latest", "gemma4:e4b", "library/gemma4:31b"] {
        let messages = sent_messages(model).await;
        assert_eq!(
            messages[2..],
            [
                json!({"role": "assistant", "content": SAID}),
                json!({"role": "assistant", "content": "", "tool_calls": [call.clone()]}),
                result.clone(),
            ],
            "{model}"
        );
    }
    let messages = sent_messages("qwen3.5:2b").await;
    assert_eq!(
        messages[2..],
        [
            json!({"role": "assistant", "content": SAID, "tool_calls": [call]}),
            result,
        ]
    );
}
