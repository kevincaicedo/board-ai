//! The measured OpenRouter talkers (D-189) through the OpenAI-compatible adapter, from streams
//! recorded on 2026-10-16 with BoardAI's request shape on each preset's route: DeepSeek V4.1
//! Flash (Together), Claude Haiku 5.5 (Claude Platform on AWS, prompt cached on request),
//! GPT-6 Luna (Amazon Bedrock) and Gemini 3.8 Flash (Google AI Studio, reasoning mandatory at
//! `low`). Each gives a text answer and a tool call; every split of a stream decodes the same.
//! The recordings were made with `read_file` offered and "Answer in one short sentence." (or,
//! for Haiku's cached ones, BoardAI's real system prompt). No network.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{AssistantPart, Conversation, Message, UserPart};
use boardai_core::input::StopReason;
use boardai_core::tools::ToolCall;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::llm::openai::{Dialect, OpenAiCompatibleProvider, OpenAiOptions};
use boardai_runtime::llm::openrouter::{
    CLAUDE_HAIKU_5_5, DEEPSEEK_V4_1_FLASH, GEMINI_3_8_FLASH, GPT_6_LUNA, Preset,
};
use boardai_runtime::llm::{
    ChatEvent, LlmProvider, LlmRole, ProviderError, RetryPolicy, TalkerConfig, ToolSpec, Usage,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{Scripted, TestServer, fixture};

type Events = Vec<Result<ChatEvent, ProviderError>>;

fn provider(server: &TestServer, preset: &Preset) -> OpenAiCompatibleProvider {
    let config = TalkerConfig {
        role: LlmRole::Talker,
        model: preset.model.to_owned(),
        effort: preset.talker.effort,
        max_tokens: 300,
        system_prompt: "Answer in one short sentence.".into(),
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the open repository.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    };
    OpenAiCompatibleProvider::new(
        &config,
        OpenAiOptions {
            base_url: format!("{}/v1", server.base_url),
            api_key: Some(boardai_runtime::settings::Secret::new("sk-or-test".into())),
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

fn ask(text: &str) -> Conversation {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text { text: text.into() }],
    });
    conversation
}

/// The stream decoded whole, then in 3 and 11 pieces: every split gives the same events.
async fn decode(preset: &Preset, name: &str) -> Events {
    let body = fixture(name);
    let mut decoded: Vec<Events> = Vec::new();
    for pieces in [1, 3, 11] {
        let server = TestServer::start(vec![Scripted::sse(&body, pieces)]).await;
        let events = provider(&server, preset)
            .stream(&ask("What is a load balancer?"), CancelToken::new())
            .collect()
            .await;
        decoded.push(events);
    }
    assert_eq!(decoded[0], decoded[1], "{name}: 3 pieces");
    assert_eq!(decoded[0], decoded[2], "{name}: 11 pieces");
    decoded.swap_remove(0)
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

/// The last event, which must be `Done`: its stop reason, usage and the turn as stored.
fn done(events: &Events) -> (StopReason, Usage, Value) {
    let Some(Ok(ChatEvent::Done { stop, usage, raw })) = events.last() else {
        panic!("expected Done last, got {events:?}");
    };
    let raw = raw.as_ref().expect("a stored turn");
    let raw = serde_json::from_str(&raw.content_json).expect("the stored turn is JSON");
    (stop.clone(), *usage, raw)
}

fn usage(input_tokens: u64, output_tokens: u64, read: u64, written: u64) -> Usage {
    Usage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: written,
        cache_read_input_tokens: read,
    }
}

/// The stored turn is only what the model said: reasoning (Gemini's encrypted
/// `reasoning_details` included) is never spoken and never echoed (D-152).
fn assert_stored_without_reasoning(raw: &Value) {
    let keys: Vec<&str> = raw
        .as_object()
        .expect("a message")
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        keys.iter()
            .all(|key| matches!(*key, "role" | "content" | "tool_calls")),
        "{keys:?}"
    );
}

async fn assert_text(preset: Preset, name: &str, expected: &str, tokens: Usage) {
    let events = decode(&preset, name).await;
    assert_eq!(text_of(&events), expected, "{name}");
    let (stop, usage, raw) = done(&events);
    assert_eq!(stop, StopReason::EndTurn, "{name}");
    assert_eq!(usage, tokens, "{name}");
    assert_eq!(
        raw,
        json!({"role": "assistant", "content": expected}),
        "{name}"
    );
    assert_stored_without_reasoning(&raw);
}

#[tokio::test]
async fn each_measured_model_streams_a_text_answer_with_usage() {
    assert_text(
        DEEPSEEK_V4_1_FLASH,
        "openai/openrouter_deepseek_v4_1_flash_text.sse",
        "A load balancer distributes incoming network traffic across multiple servers to ensure \
         no single server is overwhelmed, improving reliability and performance.",
        usage(271, 26, 0, 0),
    )
    .await;
    assert_text(
        CLAUDE_HAIKU_5_5,
        "openai/openrouter_claude_haiku_5_5_text.sse",
        "A load balancer is a system that distributes incoming network traffic across multiple \
         servers to improve reliability, availability, and performance.",
        usage(375, 41, 0, 0),
    )
    .await;
    assert_text(
        GPT_6_LUNA,
        "openai/openrouter_gpt_6_luna_text.sse",
        "A load balancer distributes incoming network traffic across multiple servers to \
         improve performance, reliability, and availability.",
        usage(61, 24, 0, 0),
    )
    .await;
    assert_text(
        GEMINI_3_8_FLASH,
        "openai/openrouter_gemini_3_8_flash_text.sse",
        "A load balancer is a device or software that distributes incoming network traffic \
         across multiple servers to optimize performance, prevent overload, and ensure high \
         availability.",
        usage(53, 28, 0, 0),
    )
    .await;
}

async fn assert_tool(preset: Preset, name: &str, id: &str, said: &str, tokens: Usage) -> Value {
    let events = decode(&preset, name).await;
    assert_eq!(text_of(&events), said, "{name}");
    let calls: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::ToolCall(invocation)) => Some(invocation),
            Ok(ChatEvent::TextDelta(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect();
    assert_eq!(calls.len(), 1, "{name}");
    assert_eq!(calls[0].id.as_str(), id, "{name}");
    let read = ToolCall::ReadFile {
        path: "src/main.rs".into(),
        start_line: None,
        end_line: None,
    };
    assert_eq!(calls[0].call, Ok(read), "{name}");
    let (stop, usage, raw) = done(&events);
    assert_eq!(stop, StopReason::ToolUse, "{name}");
    assert_eq!(usage, tokens, "{name}");
    assert_eq!(raw["tool_calls"][0]["id"], id, "{name}");
    assert_eq!(raw["tool_calls"][0]["function"]["name"], "read_file");
    assert_stored_without_reasoning(&raw);
    raw
}

#[tokio::test]
async fn each_measured_model_calls_a_tool() {
    assert_tool(
        DEEPSEEK_V4_1_FLASH,
        "openai/openrouter_deepseek_v4_1_flash_tool.sse",
        "call_01a14806809570a4a4c36edb",
        "I'll read that file for you.",
        usage(288, 47, 0, 0),
    )
    .await;
    // Recorded with BoardAI's system prompt cached on request (D-215): the prompt and tools
    // came from the cache, and only the new turn was written.
    assert_tool(
        CLAUDE_HAIKU_5_5,
        "openai/openrouter_claude_haiku_5_5_tool.sse",
        "toolu_01EvEauhXDXzibqcWmccrvW5",
        "I'll read that file now.",
        usage(4, 61, 3_703, 88),
    )
    .await;
    assert_tool(
        GPT_6_LUNA,
        "openai/openrouter_gpt_6_luna_tool.sse",
        "call_cf558d1669e75345bc7aa3ab8c1bb254",
        "",
        usage(78, 20, 0, 0),
    )
    .await;
    assert_tool(
        GEMINI_3_8_FLASH,
        "openai/openrouter_gemini_3_8_flash_tool.sse",
        "call_1926696",
        "",
        usage(74, 20, 0, 0),
    )
    .await;
}

/// Claude's first answer of a session with the prompt cached on request, cut at the
/// recording's 300-token budget: the cache write is counted apart from the input, and a turn
/// cut off at its budget stops on `max_tokens`.
#[tokio::test]
async fn a_cached_claude_answer_counts_its_cache_write() {
    let events = decode(
        &CLAUDE_HAIKU_5_5,
        "openai/openrouter_claude_haiku_5_5_cached_length.sse",
    )
    .await;
    assert!(
        text_of(&events).starts_with("A load balancer sits in front of several copies"),
        "{events:?}"
    );
    let (stop, usage, _) = done(&events);
    assert_eq!(stop, StopReason::MaxTokens);
    assert_eq!(usage, self::usage(4, 300, 0, 3_783));
}

/// Gemini's call echoed as stored, with no `reasoning_details`, followed by its result: what
/// the next request sends. OpenRouter accepted this shape from Gemini 3.8 Flash on AI Studio
/// (live, 2026-10-16), so the encrypted reasoning needs no keeping.
#[tokio::test]
async fn a_geminis_call_is_echoed_as_stored_with_its_result() {
    let raw = assert_tool(
        GEMINI_3_8_FLASH,
        "openai/openrouter_gemini_3_8_flash_tool.sse",
        "call_1926696",
        "",
        usage(74, 20, 0, 0),
    )
    .await;
    let text = fixture("openai/openrouter_gemini_3_8_flash_text.sse");
    let server = TestServer::start(vec![Scripted::sse(&text, 2)]).await;
    let id = boardai_core::ids::ToolCallId::parse("call_1926696").expect("id");
    let mut conversation = ask("Read src/main.rs and tell me what it does.");
    conversation.push(Message::Assistant {
        parts: vec![AssistantPart::ToolUse {
            id: id.clone(),
            name: "read_file".into(),
            input_json: r#"{"path":"src/main.rs"}"#.into(),
        }],
        raw: Some(boardai_core::history::RawAssistant {
            provider: boardai_core::history::ProviderFamily::OpenAiCompatible,
            content_json: raw.to_string(),
        }),
    });
    conversation.push(Message::User {
        parts: vec![UserPart::ToolResult {
            id,
            content: "fn main() {}".into(),
            is_error: false,
        }],
    });
    conversation.push(Message::System {
        text: "Board now:\nmain: 1 box".into(),
    });
    let _: Events = provider(&server, &GEMINI_3_8_FLASH)
        .stream(&conversation, CancelToken::new())
        .collect()
        .await;
    let body = server.requests()[0].json();
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages[2], raw);
    assert_eq!(
        messages[3],
        json!({"role": "tool", "tool_call_id": "call_1926696", "content": "fn main() {}"})
    );
    assert_eq!(
        messages[4],
        json!({"role": "user", "content": "<system-reminder>\nBoard now:\nmain: 1 box\n</system-reminder>"})
    );
}
