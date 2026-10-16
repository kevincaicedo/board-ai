//! HTTP pieces shared by the providers: the client, error responses, and retry timing.

use std::time::Duration;

use reqwest::header::HeaderMap;

use super::error::{ProviderError, bounded_detail};
use crate::limits::{
    PROVIDER_CONNECT_TIMEOUT_MS, PROVIDER_ERROR_BODY_MAX_BYTES, PROVIDER_ERROR_BODY_TIMEOUT_MS,
    PROVIDER_RETRY_AFTER_MAX_MS,
};

/// One request as it goes on the wire; rebuilt for each attempt from these parts.
#[derive(Clone, Debug)]
pub(crate) struct RequestSpec {
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// A client with no overall timeout (responses stream for as long as the model talks; the
/// session bounds them) and a bounded connect.
pub(crate) fn build_client() -> Result<reqwest::Client, ProviderError> {
    install_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(PROVIDER_CONNECT_TIMEOUT_MS))
        .build()
        .map_err(|error| ProviderError::NotConfigured {
            detail: bounded_detail(&error.to_string()),
        })
}

/// reqwest is built without a bundled crypto provider, so one is installed for the process:
/// ring, which needs no OpenSSL and no CMake.
fn install_crypto_provider() {
    match rustls::crypto::ring::default_provider().install_default() {
        Ok(()) => {}
        // Another component (or an earlier client) installed one first; reqwest uses that.
        Err(_already_installed) => {}
    }
}

/// `base_url` joined with `path`, after checking it is an http(s) URL.
pub(crate) fn endpoint(base_url: &str, path: &str) -> Result<String, ProviderError> {
    let parsed = reqwest::Url::parse(base_url).map_err(|_| ProviderError::NotConfigured {
        detail: "the provider base URL is not a valid URL".to_owned(),
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ProviderError::NotConfigured {
            detail: "the provider base URL must be http or https".to_owned(),
        });
    }
    Ok(format!("{}/{path}", base_url.trim_end_matches('/')))
}

/// Turns a non-success response into an error, reading a bounded part of its body for the
/// provider's message.
pub(crate) async fn error_from_response(response: reqwest::Response) -> ProviderError {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let body = tokio::time::timeout(
        Duration::from_millis(PROVIDER_ERROR_BODY_TIMEOUT_MS),
        read_bounded(response),
    )
    .await
    .unwrap_or_default();
    ProviderError::from_status(status, retry_after, error_message(&body))
}

async fn read_bounded(mut response: reqwest::Response) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < PROVIDER_ERROR_BODY_MAX_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = PROVIDER_ERROR_BODY_MAX_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    body
}

/// The provider's `error.message` when the body is the usual JSON error (Ollama's native API
/// sends `error` as a string), else the body text.
pub(crate) fn error_message(body: &[u8]) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let message = parsed.as_ref().and_then(|value| {
        value
            .pointer("/error/message")
            .or_else(|| value.pointer("/error"))
            .and_then(serde_json::Value::as_str)
    });
    match message {
        Some(message) => bounded_detail(message),
        None => bounded_detail(&String::from_utf8_lossy(body)),
    }
}

/// `retry-after` in seconds (integer or decimal). The HTTP-date form is not used by the
/// providers we speak to and reads as absent.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds: f64 = value.trim().parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    // Clamp before converting so a huge value cannot overflow `Duration`.
    let cap_seconds = 3_600.0;
    Some(Duration::from_secs_f64(seconds.min(cap_seconds)))
}

/// The wait before retry `attempt` (0-based): the backoff doubled per attempt plus up to half
/// of it in jitter, or the provider's `retry-after` when that is longer. `None` when the
/// provider asks for longer than [`PROVIDER_RETRY_AFTER_MAX_MS`]: that error is returned.
pub(crate) fn retry_delay(
    attempt: u32,
    base: Duration,
    retry_after: Option<Duration>,
    seed: u64,
) -> Option<Duration> {
    let backoff = base.saturating_mul(1_u32 << attempt.min(16));
    let jitter_range_ms = u64::try_from(backoff.as_millis() / 2).unwrap_or(u64::MAX);
    let jitter_ms = mix(seed ^ u64::from(attempt)) % jitter_range_ms.saturating_add(1);
    let jittered = backoff.saturating_add(Duration::from_millis(jitter_ms));
    match retry_after {
        Some(after) if after > Duration::from_millis(PROVIDER_RETRY_AFTER_MAX_MS) => None,
        Some(after) => Some(after.max(jittered)),
        None => Some(jittered),
    }
}

/// splitmix64: spreads a clock reading into well-mixed bits. Jitter only needs to keep clients
/// from retrying in lockstep, so no RNG crate.
fn mix(seed: u64) -> u64 {
    let mut value = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{error_message, parse_retry_after, retry_delay};

    #[test]
    fn retry_after_reads_seconds_only() {
        assert_eq!(parse_retry_after("3"), Some(Duration::from_secs(3)));
        assert_eq!(
            parse_retry_after(" 1.5 "),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[test]
    fn delays_double_with_bounded_jitter_and_honor_retry_after() {
        let base = Duration::from_millis(250);
        for seed in 0..200 {
            let first = retry_delay(0, base, None, seed).unwrap_or_default();
            let second = retry_delay(1, base, None, seed).unwrap_or_default();
            assert!(first >= base && first <= base * 3 / 2, "{first:?}");
            assert!(second >= base * 2 && second <= base * 3, "{second:?}");
        }
        let after = Some(Duration::from_secs(2));
        assert_eq!(retry_delay(0, base, after, 1), Some(Duration::from_secs(2)));
        assert_eq!(retry_delay(0, base, Some(Duration::from_secs(60)), 1), None);
    }

    #[test]
    fn error_messages_come_from_the_json_body_when_present() {
        let body =
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(error_message(body), "Overloaded");
        assert_eq!(error_message(b"plain\ntext"), "plaintext");
    }
}
