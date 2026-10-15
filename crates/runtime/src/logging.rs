//! Logs to stderr with redaction on the sink (LLD §8, §9; law 10).
//!
//! A small `tracing` subscriber instead of `tracing-subscriber`: the runtime logs events, not
//! spans, and the part that matters, redaction and bounded fields, would be ours either way.
//! Every line is cut to [`LOG_LINE_MAX_BYTES`], every field to [`LOG_FIELD_MAX_BYTES`], and
//! anything shaped like a key is replaced before the line is written. Redaction is a backstop:
//! secrets are already kept out by type (`Secret` prints redacted).

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::io::Write as _;
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::SetGlobalDefaultError;
use tracing::{Event, Level, Metadata, Subscriber};

use crate::limits::{LOG_FIELD_MAX_BYTES, LOG_LINE_MAX_BYTES};
use crate::store::redact_text;

/// `text` with every secret-shaped run replaced. The rules are the store's (D-042): one table,
/// with a planted-secret test per rule, for every place BoardAI writes text.
#[must_use]
pub fn redact(text: &str) -> Cow<'_, str> {
    let redacted = redact_text(text);
    if redacted.count() == 0 {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(redacted.into_string())
    }
}

/// Cuts `text` to at most `max_bytes` on a char boundary, ending with `…` when cut.
pub fn bound(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
}

/// Installs the stderr logger for the whole process. Events above `level` are skipped.
pub fn init_stderr(level: Level) -> Result<(), SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(StderrLogger::new(level))
}

/// Writes each event as one bounded, redacted line: seconds since start, level, target,
/// message, fields.
#[derive(Debug)]
pub struct StderrLogger {
    level: Level,
    start: Instant,
}

impl StderrLogger {
    #[must_use]
    pub fn new(level: Level) -> Self {
        Self {
            level,
            start: Instant::now(),
        }
    }

    /// The line for one event, before redaction.
    fn line(&self, event: &Event<'_>) -> String {
        let metadata = event.metadata();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut line = String::with_capacity(128);
        let _ = write!(
            line,
            "{:>9.3} {:<5} {}: {}{}",
            self.start.elapsed().as_secs_f64(),
            metadata.level(),
            metadata.target(),
            fields.message,
            fields.rest
        );
        line
    }
}

impl Subscriber for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.level
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::from_level(self.level))
    }

    // Spans are not recorded: the runtime marks stages with events (LLD §8). Every span gets
    // the same id, which `tracing` allows for a subscriber that tracks none.
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = redact(&self.line(event)).into_owned();
        bound(&mut line, LOG_LINE_MAX_BYTES);
        line.push('\n');
        // One write per line, so lines from different threads never interleave. A failed
        // write to stderr has nowhere to be reported.
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// An event's fields: the message, then ` name=value` for the rest, each value bounded.
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_debug(field, &format_args!("{value}"));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let mut text = String::new();
        let _ = write!(text, "{value:?}");
        bound(&mut text, LOG_FIELD_MAX_BYTES);
        if field.name() == "message" {
            self.message = text;
        } else {
            let _ = write!(self.rest, " {}={text}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{bound, redact};

    /// Each planted secret is replaced, whatever surrounds it.
    #[test]
    fn planted_secrets_never_survive() {
        let planted = [
            "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv",
            "sk-or-v1-0123456789abcdef0123",
            "sk_3f9a8b7c6d5e4f3a2b1c0d9e",
            "ghp_0123456789abcdefABCDEF0123456789",
            "github_pat_11ABCDEFG0123456789_abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-1234567890-abcdefghij",
            "-----BEGIN RSA PRIVATE KEY-----MIIEow-----END RSA PRIVATE KEY-----",
        ];
        for secret in planted {
            let line = format!("request failed: key {secret} rejected");
            let redacted = redact(&line);
            assert!(!redacted.contains(secret), "{secret} leaked: {redacted}");
            assert!(redacted.contains("[redacted"), "{redacted}");
            assert!(redacted.starts_with("request failed: key "), "{redacted}");
        }
    }

    #[test]
    fn named_values_keep_their_name_and_lose_their_value() {
        assert_eq!(
            redact("password=hunter2 user=kai"),
            "password=[redacted:assigned-secret] user=kai"
        );
        assert_eq!(
            redact("api_key: abc123def"),
            "api_key: [redacted:assigned-secret]"
        );
        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOi.payload.sig"),
            "Authorization: Bearer [redacted:authorization]"
        );
    }

    #[test]
    fn ordinary_text_is_untouched_and_not_copied() {
        let line = "first token latency_ms=412 provider=anthropic task=sk";
        assert!(matches!(redact(line), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn bounding_cuts_on_a_char_boundary() {
        let mut text = "aé".repeat(10);
        bound(&mut text, 6);
        assert!(text.len() <= 6, "{text}");
        assert!(text.ends_with('…'));
        let mut short = "ok".to_owned();
        bound(&mut short, 6);
        assert_eq!(short, "ok");
    }
}
