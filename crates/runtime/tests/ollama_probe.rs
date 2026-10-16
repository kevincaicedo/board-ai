//! The first-run screen's Ollama probe against a local fake server: the model list, error
//! statuses, and a server that never answers (bounded by `OLLAMA_PROBE_TIMEOUT_MS`).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::time::{Duration, Instant};

use boardai_runtime::limits::OLLAMA_PROBE_TIMEOUT_MS;
use boardai_runtime::llm::ProviderError;
use boardai_runtime::llm::ollama::ollama_models;
use support::{Scripted, TestServer};

const TAGS: &str = r#"{"models":[{"name":"gemma4:latest","model":"gemma4:latest","size":9600000000},{"name":"qwen3.5:2b"}]}"#;

#[tokio::test]
async fn the_model_list_is_read_from_the_root_of_the_openai_url() {
    let server = TestServer::start(vec![Scripted::status(200, TAGS)]).await;
    let models = ollama_models(&format!("{}/v1", server.base_url)).await;
    assert_eq!(
        models,
        Ok(vec!["gemma4:latest".to_owned(), "qwen3.5:2b".to_owned()])
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/tags");
}

#[tokio::test]
async fn an_error_status_or_a_malformed_list_is_an_error() {
    let server = TestServer::start(vec![
        Scripted::status(500, r#"{"error":"boom"}"#),
        Scripted::status(200, "not json"),
    ])
    .await;
    assert!(matches!(
        ollama_models(&server.base_url).await,
        Err(ProviderError::Server { status: 500, .. })
    ));
    assert!(matches!(
        ollama_models(&server.base_url).await,
        Err(ProviderError::BadOutput { .. })
    ));
}

#[tokio::test]
async fn a_server_that_never_answers_is_given_up_on_in_time() {
    let server = TestServer::start(vec![Scripted::status(200, r#"{"models":["#).hanging()]).await;
    let started = Instant::now();
    let probed = ollama_models(&server.base_url).await;
    assert!(
        matches!(probed, Err(ProviderError::Network { .. })),
        "{probed:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS * 3));
}

#[tokio::test]
async fn nothing_listening_is_a_network_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let probed = ollama_models(&format!("http://{address}")).await;
    assert!(
        matches!(probed, Err(ProviderError::Network { .. })),
        "{probed:?}"
    );
}
