//! What the adapters send to ready a session's first request (D-154), and how a server that
//! may be Ollama is probed once and then spoken to natively or as OpenAI-compatible (D-156).
//! Local fake servers only.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{Conversation, Message, UserPart};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::FakeClock;
use boardai_runtime::limits::{OLLAMA_CONTEXT_TOKENS, OLLAMA_KEEP_ALIVE_SECONDS};
use boardai_runtime::llm::ollama_chat::{MaybeOllama, OllamaChatOptions, OllamaChatProvider};
use boardai_runtime::llm::openai::{Dialect, OpenAiCompatibleProvider, OpenAiOptions};
use boardai_runtime::llm::openrouter::ProviderSlug;
use boardai_runtime::llm::{
    ChatEvent, Effort, LlmProvider, LlmRole, ProviderError, RetryPolicy, TalkerConfig, ToolSpec,
};
use futures_util::StreamExt;
use serde_json::json;
use support::{Scripted, TestServer, fixture};

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

fn policy() -> Option<RetryPolicy> {
    Some(RetryPolicy {
        first_token_deadline: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(5),
        backoff_base: Duration::from_millis(10),
    })
}

fn native(base_url: &str) -> OllamaChatProvider {
    OllamaChatProvider::new(
        &config(Effort::None),
        OllamaChatOptions {
            base_url: base_url.to_owned(),
            clock: Arc::new(FakeClock::new()),
            policy: policy(),
        },
    )
    .expect("provider builds")
}

fn compatible(base_url: &str, dialect: Dialect) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        &config(Effort::Low),
        OpenAiOptions {
            base_url: base_url.to_owned(),
            api_key: None,
            clock: Arc::new(FakeClock::new()),
            policy: policy(),
            dialect,
        },
    )
    .expect("provider builds")
}

fn json_answer(body: &str) -> Scripted {
    Scripted::status(200, body)
}

fn ndjson(name: &str) -> Scripted {
    let mut scripted = Scripted::sse(&fixture(name), 2);
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

async fn text(provider: &dyn LlmProvider) -> String {
    let events: Vec<_> = provider
        .stream(&question(), CancelToken::new())
        .collect()
        .await;
    events
        .iter()
        .filter_map(|event| match event {
            Ok(ChatEvent::TextDelta(text)) => Some(text.as_str()),
            Ok(ChatEvent::ToolCall(_) | ChatEvent::Done { .. }) | Err(_) => None,
        })
        .collect()
}

/// Ollama loads the model at the session's context window and prefills the system prompt and
/// tools; the answer is one token, not streamed.
#[tokio::test]
async fn ollama_warms_up_with_the_frozen_prompt_and_the_same_context() {
    let server = TestServer::start(vec![json_answer(r#"{"done":true}"#)]).await;
    let warm_up = native(&format!("{}/v1", server.base_url))
        .warm_up()
        .expect("a local model is worth warming up");
    assert_eq!(warm_up.await, Ok(()));
    let request = &server.requests()[0];
    assert_eq!(request.path, "/api/chat");
    let body = request.json();
    assert_eq!(body["stream"], false);
    assert_eq!(body["options"]["num_predict"], 1);
    assert_eq!(body["options"]["num_ctx"], OLLAMA_CONTEXT_TOKENS);
    assert_eq!(body["keep_alive"], OLLAMA_KEEP_ALIVE_SECONDS);
    assert_eq!(body["think"], false);
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "Answer in one short sentence."},
            {"role": "user", "content": "Hi."},
        ])
    );
}

#[tokio::test]
async fn a_failed_warm_up_keeps_its_category() {
    let server = TestServer::start(vec![Scripted::status(
        404,
        r#"{"error":"model 'gemma4:latest' not found"}"#,
    )])
    .await;
    let warm_up = native(&server.base_url).warm_up().expect("a warm-up");
    assert!(matches!(
        warm_up.await,
        Err(ProviderError::InvalidRequest {
            status: Some(404),
            ..
        })
    ));
}

/// OpenRouter: the request as it will be sent (reasoning and route included), answered with
/// one token and not streamed; OpenAI-compatible servers refuse `stream_options` then.
#[tokio::test]
async fn openrouter_warms_up_with_its_own_fields_and_other_servers_do_not() {
    let server = TestServer::start(vec![json_answer(r#"{"choices":[]}"#)]).await;
    let route = vec![ProviderSlug::parse("cerebras").expect("slug")];
    let open_router = compatible(
        &format!("{}/api/v1", server.base_url),
        Dialect::OpenRouter { route },
    );
    let warm_up = open_router.warm_up().expect("OpenRouter warms up");
    assert_eq!(warm_up.await, Ok(()));
    let request = &server.requests()[0];
    assert_eq!(request.path, "/api/v1/chat/completions");
    let body = request.json();
    assert_eq!(body["stream"], false);
    assert_eq!(body.get("stream_options"), None);
    assert_eq!(body["max_tokens"], 1);
    assert_eq!(body["reasoning"], json!({"effort": "low", "exclude": true}));
    assert_eq!(body["provider"]["order"], json!(["cerebras"]));
    assert_eq!(
        body["messages"][1],
        json!({"role": "user", "content": "Hi."})
    );
    let standard = compatible(&server.base_url, Dialect::Standard);
    assert!(standard.warm_up().is_none());
}

/// Ollama on another host answers `/api/version`, so it gets the native API; the probe runs
/// once.
#[tokio::test]
async fn ollama_elsewhere_is_found_by_its_version_and_spoken_to_natively() {
    let server = TestServer::start(vec![
        json_answer(r#"{"version":"0.40.5"}"#),
        ndjson("ollama_chat/text.ndjson"),
        ndjson("ollama_chat/text.ndjson"),
    ])
    .await;
    let base_url = format!("{}/v1", server.base_url);
    let maybe = MaybeOllama::new(
        native(&base_url),
        compatible(&base_url, Dialect::Standard),
        base_url.clone(),
    );
    assert!(!text(&maybe).await.is_empty());
    assert!(!text(&maybe).await.is_empty());
    let paths: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(paths, ["/api/version", "/api/chat", "/api/chat"]);
}

/// Anything that isn't Ollama (LM Studio answers 404 there) keeps the OpenAI-compatible API,
/// and the session's warm-up is where the probe happens.
#[tokio::test]
async fn another_server_keeps_the_compatible_api_after_the_probe() {
    let server = TestServer::start(vec![
        Scripted::status(404, r#"{"error":"Unexpected endpoint"}"#),
        Scripted::sse(&fixture("openai/ollama_text.sse"), 2),
    ])
    .await;
    let base_url = format!("{}/v1", server.base_url);
    let maybe = MaybeOllama::new(
        native(&base_url),
        compatible(&base_url, Dialect::Standard),
        base_url.clone(),
    );
    let warm_up = maybe.warm_up().expect("the probe runs as the warm-up");
    assert_eq!(warm_up.await, Ok(()));
    assert!(!text(&maybe).await.is_empty());
    let paths: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(paths, ["/api/version", "/v1/chat/completions"]);
}

/// A server that answers `/api/version` without a version (a proxy's catch-all page, `{}`) is
/// not Ollama.
#[tokio::test]
async fn an_answer_without_a_version_is_not_ollama() {
    let server = TestServer::start(vec![
        json_answer("{}"),
        Scripted::sse(&fixture("openai/ollama_text.sse"), 2),
    ])
    .await;
    let base_url = format!("{}/v1", server.base_url);
    let maybe = MaybeOllama::new(
        native(&base_url),
        compatible(&base_url, Dialect::Standard),
        base_url.clone(),
    );
    assert!(!text(&maybe).await.is_empty());
    let paths: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(paths, ["/api/version", "/v1/chat/completions"]);
}
