//! Asking a local Ollama which models it has (`GET /api/tags`), for the first-run screen
//! (LLD §3.8). One short request under one deadline: an Ollama that isn't running must not
//! hold the screen up.

use std::time::Duration;

use serde::Deserialize;

use super::error::{ProviderError, bounded_detail};
use super::http::{build_client, endpoint, error_from_response};
use crate::limits::{
    OLLAMA_MODEL_NAME_MAX_BYTES, OLLAMA_MODELS_MAX, OLLAMA_PROBE_TIMEOUT_MS, OLLAMA_TAGS_MAX_BYTES,
};

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

/// The model names Ollama lists (`gemma4:latest`), in its order, at most
/// [`OLLAMA_MODELS_MAX`]. `base_url` is the talker's OpenAI-compatible URL (`…:11434/v1`) or
/// Ollama's root. Fails with `Network` when nothing answers within
/// [`OLLAMA_PROBE_TIMEOUT_MS`].
pub async fn ollama_models(base_url: &str) -> Result<Vec<String>, ProviderError> {
    let root = base_url.trim_end_matches('/');
    let root = root.strip_suffix("/v1").unwrap_or(root);
    let url = endpoint(root, "api/tags")?;
    let client = build_client()?;
    let deadline = Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS);
    let answered = tokio::time::timeout(deadline, async move {
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|error| ProviderError::Network {
                detail: bounded_detail(&error.to_string()),
            })?;
        if !response.status().is_success() {
            return Err(error_from_response(response).await);
        }
        read_body(response).await
    })
    .await
    .map_err(|_| ProviderError::Network {
        detail: format!("Ollama did not answer within {OLLAMA_PROBE_TIMEOUT_MS} ms"),
    })??;
    parse_tags(&answered)
}

async fn read_body(mut response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ProviderError::Network {
            detail: bounded_detail(&error.to_string()),
        })?
    {
        if body.len() + chunk.len() > OLLAMA_TAGS_MAX_BYTES {
            return Err(ProviderError::BadOutput {
                detail: format!("Ollama's model list exceeds {OLLAMA_TAGS_MAX_BYTES} bytes"),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Names longer than [`OLLAMA_MODEL_NAME_MAX_BYTES`] (or empty) are skipped; the list is cut at
/// [`OLLAMA_MODELS_MAX`].
fn parse_tags(body: &[u8]) -> Result<Vec<String>, ProviderError> {
    let tags: Tags = serde_json::from_slice(body).map_err(|error| ProviderError::BadOutput {
        detail: bounded_detail(&format!("Ollama's model list: {error}")),
    })?;
    Ok(tags
        .models
        .into_iter()
        .map(|tag| tag.name)
        .filter(|name| !name.trim().is_empty() && name.len() <= OLLAMA_MODEL_NAME_MAX_BYTES)
        .take(OLLAMA_MODELS_MAX)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::parse_tags;
    use crate::limits::{OLLAMA_MODEL_NAME_MAX_BYTES, OLLAMA_MODELS_MAX};
    use crate::llm::ProviderError;

    #[test]
    fn names_are_read_in_order_and_bounded() {
        let long = "m".repeat(OLLAMA_MODEL_NAME_MAX_BYTES + 1);
        let body = format!(
            r#"{{"models":[{{"name":"gemma4:latest","size":1}},{{"name":""}},{{"name":"{long}"}},{{"name":"qwen3.5:2b","details":{{}}}}]}}"#
        );
        assert_eq!(
            parse_tags(body.as_bytes()),
            Ok(vec!["gemma4:latest".to_owned(), "qwen3.5:2b".to_owned()])
        );
        assert_eq!(parse_tags(br#"{}"#), Ok(Vec::new()));
        let many: Vec<String> = (0..OLLAMA_MODELS_MAX + 5)
            .map(|index| format!(r#"{{"name":"model-{index}"}}"#))
            .collect();
        let body = format!(r#"{{"models":[{}]}}"#, many.join(","));
        assert_eq!(
            parse_tags(body.as_bytes()).map(|names| names.len()),
            Ok(OLLAMA_MODELS_MAX)
        );
    }

    #[test]
    fn a_malformed_answer_is_bad_output() {
        assert!(matches!(
            parse_tags(b"<html>"),
            Err(ProviderError::BadOutput { .. })
        ));
        assert!(matches!(
            parse_tags(br#"{"models":[{"size":1}]}"#),
            Err(ProviderError::BadOutput { .. })
        ));
    }
}
