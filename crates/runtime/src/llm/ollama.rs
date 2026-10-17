//! Asking Ollama which models it has (`GET /api/tags`) for the first-run screen and the model
//! picker (LLD §3.8), and whether a server is Ollama at all (`GET /api/version`), so any
//! Ollama host gets the native chat API (D-156). One short request under one deadline each:
//! an Ollama that isn't running must not hold anything up.

use std::time::Duration;

use reqwest::header::HeaderValue;
use serde::Deserialize;

use super::error::{ProviderError, bounded_detail};
use super::http::{endpoint, get_bounded};
use crate::limits::{
    OLLAMA_MODEL_NAME_MAX_BYTES, OLLAMA_MODELS_MAX, OLLAMA_PROBE_TIMEOUT_MS, OLLAMA_TAGS_MAX_BYTES,
};

/// The local model BoardAI picks when Ollama has it and nothing else is chosen, and the one
/// `converse` and `e2e` measure (D-103).
pub const OLLAMA_DEFAULT_MODEL: &str = "gemma4:latest";

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize)]
struct Version {
    version: String,
}

/// Ollama's root for a base URL: its OpenAI-compatible URL (`…:11434/v1`) or the root itself.
#[must_use]
pub fn ollama_root(base_url: &str) -> &str {
    let root = base_url.trim_end_matches('/');
    root.strip_suffix("/v1").unwrap_or(root)
}

/// The model names Ollama lists (`gemma4:latest`), in its order, at most
/// [`OLLAMA_MODELS_MAX`]. `base_url` is the talker's OpenAI-compatible URL (`…:11434/v1`) or
/// Ollama's root. Fails with `Network` when nothing answers within
/// [`OLLAMA_PROBE_TIMEOUT_MS`].
pub async fn ollama_models(base_url: &str) -> Result<Vec<String>, ProviderError> {
    let url = endpoint(ollama_root(base_url), "api/tags")?;
    let deadline = Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS);
    let body = get_bounded(
        &url,
        None,
        OLLAMA_TAGS_MAX_BYTES,
        deadline,
        "Ollama's model list",
    )
    .await?;
    parse_tags(&body)
}

/// What `GET /api/version` said about a server (D-156).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OllamaProbe {
    /// It answered with a version.
    Ollama,
    /// It answered, and not as Ollama: another body, or a status that won't change when asked
    /// again (404 from LM Studio, 401 for a key it doesn't take).
    NotOllama,
    /// No answer to go by: silence past [`OLLAMA_PROBE_TIMEOUT_MS`], a dropped connection, a 5xx,
    /// 429. A busy or restarting Ollama answers this way, so it is asked again later instead of
    /// being taken for another server.
    Unknown,
}

/// Asks the server behind an OpenAI-compatible base URL (or its root) whether it is Ollama:
/// its native API answers `GET /api/version` with a version. `authorization` is the configured
/// key's header, sent where the chat requests go.
pub async fn probe_ollama(base_url: &str, authorization: Option<&HeaderValue>) -> OllamaProbe {
    let Ok(url) = endpoint(ollama_root(base_url), "api/version") else {
        return OllamaProbe::NotOllama;
    };
    let deadline = Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS);
    let answer = get_bounded(
        &url,
        authorization,
        OLLAMA_TAGS_MAX_BYTES,
        deadline,
        "Ollama's version",
    )
    .await;
    match answer {
        Ok(body) => {
            let version = serde_json::from_slice::<Version>(&body);
            if version.is_ok_and(|answer| !answer.version.trim().is_empty()) {
                OllamaProbe::Ollama
            } else {
                OllamaProbe::NotOllama
            }
        }
        // The same errors a chat request retries: asking again may get an answer.
        Err(error) if error.is_retryable() => {
            tracing::info!(%error, "Ollama's version did not come back");
            OllamaProbe::Unknown
        }
        Err(error) => {
            tracing::debug!(%error, "not Ollama's native API");
            OllamaProbe::NotOllama
        }
    }
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
