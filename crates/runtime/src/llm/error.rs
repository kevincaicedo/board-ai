//! Provider errors keep their category all the way to the spoken failure line (LLD §6).

use std::fmt;
use std::time::Duration;

use boardai_core::input::{TalkerFailure, TalkerFailureKind};

use crate::limits::PROVIDER_ERROR_DETAIL_MAX_CHARS;

/// Why a provider stream failed. `detail` is for the log and the caption: it is bounded and
/// never holds a secret (request headers are never quoted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// 401 or 403: a missing, wrong or unauthorized key.
    Auth {
        detail: String,
    },
    /// 429.
    RateLimited {
        retry_after: Option<Duration>,
        detail: String,
    },
    /// 529 or an `overloaded_error` event.
    Overloaded {
        retry_after: Option<Duration>,
        detail: String,
    },
    /// Connect, TLS or read failure, or a stream that ended early.
    Network {
        detail: String,
    },
    Timeout {
        kind: TimeoutKind,
    },
    /// The provider sent something we could not parse.
    BadOutput {
        detail: String,
    },
    /// The provider rejected the request (400, 404, 413…), or the conversation cannot be sent
    /// as is (an assistant prefill). `status` is absent when we refused before sending.
    InvalidRequest {
        status: Option<u16>,
        detail: String,
    },
    /// A 5xx other than 529.
    Server {
        status: u16,
        detail: String,
    },
    /// An error event of a kind we don't classify.
    Provider {
        kind: String,
        detail: String,
    },
    /// The provider cannot be built: no key, a bad base URL.
    NotConfigured {
        detail: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutKind {
    /// No first token within the deadline, retries included.
    FirstToken,
    /// The stream went silent after it started.
    Idle,
}

impl ProviderError {
    /// Whether another attempt may succeed. Only used before the first token.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. }
            | Self::Overloaded { .. }
            | Self::Network { .. }
            | Self::Server { .. } => true,
            Self::Auth { .. }
            | Self::Timeout { .. }
            | Self::BadOutput { .. }
            | Self::InvalidRequest { .. }
            | Self::Provider { .. }
            | Self::NotConfigured { .. } => false,
        }
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } | Self::Overloaded { retry_after, .. } => {
                *retry_after
            }
            Self::Auth { .. }
            | Self::Network { .. }
            | Self::Timeout { .. }
            | Self::BadOutput { .. }
            | Self::InvalidRequest { .. }
            | Self::Server { .. }
            | Self::Provider { .. }
            | Self::NotConfigured { .. } => None,
        }
    }

    /// The core's view of the failure: same category, bounded detail.
    #[must_use]
    pub fn to_talker_failure(&self) -> TalkerFailure {
        let kind = match self {
            Self::Auth { .. } => TalkerFailureKind::Auth,
            Self::RateLimited { retry_after, .. } => TalkerFailureKind::RateLimited {
                retry_after_ms: retry_after
                    .map(|after| u32::try_from(after.as_millis()).unwrap_or(u32::MAX)),
            },
            Self::Network { .. } => TalkerFailureKind::Network,
            Self::Timeout { .. } => TalkerFailureKind::Timeout,
            Self::BadOutput { .. } => TalkerFailureKind::BadOutput,
            Self::Overloaded { .. }
            | Self::InvalidRequest { .. }
            | Self::Server { .. }
            | Self::Provider { .. } => TalkerFailureKind::Provider,
            Self::NotConfigured { .. } => TalkerFailureKind::NotConfigured,
        };
        TalkerFailure {
            kind,
            detail: self.to_string(),
        }
    }

    /// Maps an HTTP error status. `detail` is the provider's message, already bounded.
    pub(crate) fn from_status(status: u16, retry_after: Option<Duration>, detail: String) -> Self {
        match status {
            401 | 403 => Self::Auth { detail },
            429 => Self::RateLimited {
                retry_after,
                detail,
            },
            529 => Self::Overloaded {
                retry_after,
                detail,
            },
            500..=599 => Self::Server { status, detail },
            _ => Self::InvalidRequest {
                status: Some(status),
                detail,
            },
        }
    }

    /// Maps an error event's `type` (Anthropic's names, which OpenAI-compatible servers mostly
    /// share) when it arrives inside a 200 stream.
    pub(crate) fn from_error_type(kind: &str, detail: String) -> Self {
        match kind {
            "authentication_error" | "permission_error" => Self::Auth { detail },
            "rate_limit_error" => Self::RateLimited {
                retry_after: None,
                detail,
            },
            "overloaded_error" => Self::Overloaded {
                retry_after: None,
                detail,
            },
            "api_error" | "server_error" => Self::Server {
                status: 500,
                detail,
            },
            "invalid_request_error" | "not_found_error" | "request_too_large" => {
                Self::InvalidRequest {
                    status: None,
                    detail,
                }
            }
            _ => Self::Provider {
                kind: bounded_detail(kind),
                detail,
            },
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth { detail } => write!(formatter, "authentication failed: {detail}"),
            Self::RateLimited { detail, .. } => write!(formatter, "rate limited: {detail}"),
            Self::Overloaded { detail, .. } => write!(formatter, "provider overloaded: {detail}"),
            Self::Network { detail } => write!(formatter, "network error: {detail}"),
            Self::Timeout {
                kind: TimeoutKind::FirstToken,
            } => formatter.write_str("no response before the deadline"),
            Self::Timeout {
                kind: TimeoutKind::Idle,
            } => formatter.write_str("the response stalled"),
            Self::BadOutput { detail } => write!(formatter, "unreadable provider output: {detail}"),
            Self::InvalidRequest {
                status: Some(status),
                detail,
            } => write!(formatter, "request rejected ({status}): {detail}"),
            Self::InvalidRequest {
                status: None,
                detail,
            } => write!(formatter, "request rejected: {detail}"),
            Self::Server { status, detail } => {
                write!(formatter, "server error ({status}): {detail}")
            }
            Self::Provider { kind, detail } => write!(formatter, "provider error {kind}: {detail}"),
            Self::NotConfigured { detail } => {
                write!(formatter, "provider not configured: {detail}")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

/// Cuts provider text to [`PROVIDER_ERROR_DETAIL_MAX_CHARS`] and drops control characters, so a
/// detail is always one bounded line.
pub(crate) fn bounded_detail(text: &str) -> String {
    let mut detail: String = text
        .chars()
        .filter(|character| !character.is_control())
        .take(PROVIDER_ERROR_DETAIL_MAX_CHARS)
        .collect();
    if text.chars().count() > PROVIDER_ERROR_DETAIL_MAX_CHARS {
        detail.push('…');
    }
    detail
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use boardai_core::input::TalkerFailureKind;

    use super::{ProviderError, bounded_detail};

    #[test]
    fn statuses_keep_their_category() {
        let after = Some(Duration::from_secs(3));
        let cases = [
            (401, TalkerFailureKind::Auth),
            (403, TalkerFailureKind::Auth),
            (
                429,
                TalkerFailureKind::RateLimited {
                    retry_after_ms: Some(3_000),
                },
            ),
            (529, TalkerFailureKind::Provider),
            (503, TalkerFailureKind::Provider),
            (400, TalkerFailureKind::Provider),
        ];
        for (status, kind) in cases {
            let error = ProviderError::from_status(status, after, "x".into());
            assert_eq!(error.to_talker_failure().kind, kind, "{status}");
        }
        assert!(ProviderError::from_status(529, None, String::new()).is_retryable());
        assert!(!ProviderError::from_status(401, None, String::new()).is_retryable());
        assert!(!ProviderError::from_status(400, None, String::new()).is_retryable());
    }

    #[test]
    fn details_are_bounded_single_lines() {
        let long = format!("line\none{}", "x".repeat(1_000));
        let detail = bounded_detail(&long);
        assert!(!detail.contains('\n'));
        assert!(detail.chars().count() <= crate::limits::PROVIDER_ERROR_DETAIL_MAX_CHARS + 1);
        assert!(detail.ends_with('…'));
    }
}
