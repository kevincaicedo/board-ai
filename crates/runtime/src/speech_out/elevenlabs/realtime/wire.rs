//! The multi-context WebSocket's messages (ElevenLabs API reference,
//! `v1/text-to-speech/{voice_id}/multi-stream-input`). Client messages are snake_case; server
//! messages are camelCase (`contextId`, `isFinal`, `charStartTimesMs`), though ElevenLabs' own
//! example reads `is_final`, so both spellings are accepted. Everything the server sends is
//! parsed here, once, into [`ServerMessage`].

use serde::Deserialize;

use crate::llm::bounded_detail;
use crate::speech_out::TtsError;

/// A clause's context on the connection. Ids are never reused within a connection's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContextId(pub(crate) u64);

impl ContextId {
    fn wire(self) -> String {
        format!("c{}", self.0)
    }

    fn parse(text: &str) -> Option<Self> {
        text.strip_prefix('c')?.parse().ok().map(Self)
    }
}

/// Starts a context with the clause's text. The API asks for text ending in one space.
pub(super) fn context_text(context: ContextId, text: &str) -> String {
    let mut spoken = text.trim_end().to_owned();
    spoken.push(' ');
    serde_json::json!({ "text": spoken, "context_id": context.wire() }).to_string()
}

/// Generates whatever the context holds now, so the clause is never kept waiting for more text.
pub(super) fn flush(context: ContextId) -> String {
    serde_json::json!({ "context_id": context.wire(), "flush": true }).to_string()
}

/// Ends the context after its flush; the server finishes it and answers with a final message.
pub(super) fn close_context(context: ContextId) -> String {
    serde_json::json!({ "context_id": context.wire(), "close_context": true }).to_string()
}

/// Closes every context and the connection.
pub(super) fn close_socket() -> String {
    serde_json::json!({ "close_socket": true }).to_string()
}

/// One message from the server, checked.
#[derive(Debug, PartialEq)]
pub(super) enum ServerMessage {
    /// Audio and its alignment for a context, possibly its last (`is_final`).
    Chunk {
        context: Option<ContextId>,
        /// Base64 16-bit PCM; absent or empty on a final message.
        audio: Option<String>,
        alignment: Vec<(String, f64)>,
        is_final: bool,
    },
    /// The server refused something: one context when it names one, else the connection.
    Error {
        context: Option<ContextId>,
        error: TtsError,
    },
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    audio: Option<String>,
    #[serde(default)]
    alignment: Option<RawAlignment>,
    #[serde(default, rename = "contextId", alias = "context_id")]
    context: Option<String>,
    #[serde(default, rename = "isFinal", alias = "is_final")]
    is_final: Option<bool>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct RawAlignment {
    #[serde(default)]
    chars: Vec<String>,
    #[serde(default, rename = "charStartTimesMs", alias = "char_start_times_ms")]
    starts_ms: Vec<f64>,
}

/// Parses one text message. Not JSON, or JSON of the wrong shape, is `BadOutput`.
pub(super) fn parse(text: &str) -> Result<ServerMessage, TtsError> {
    let raw: RawMessage = serde_json::from_str(text).map_err(|error| TtsError::BadOutput {
        detail: bounded_detail(&format!("a realtime message is not valid: {error}")),
    })?;
    let context = raw.context.as_deref().and_then(ContextId::parse);
    let has_audio = raw.audio.as_deref().is_some_and(|audio| !audio.is_empty());
    let is_final = raw.is_final == Some(true);
    if !has_audio && !is_final && (raw.error.is_some() || raw.message.is_some()) {
        let kind = match &raw.error {
            Some(serde_json::Value::String(kind)) => kind.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let message = raw.message.unwrap_or_default();
        return Ok(ServerMessage::Error {
            context,
            error: classify(&kind, &message),
        });
    }
    let alignment = raw
        .alignment
        .map(|alignment| {
            alignment
                .chars
                .into_iter()
                .zip(alignment.starts_ms)
                .collect()
        })
        .unwrap_or_default();
    Ok(ServerMessage::Chunk {
        context,
        audio: raw.audio.filter(|audio| !audio.is_empty()),
        alignment,
        is_final,
    })
}

/// A refusal in the server's words, with its category: a bad key is `Auth`, a used-up quota or
/// too many requests is `RateLimited`, anything else `Provider`.
pub(super) fn classify(kind: &str, message: &str) -> TtsError {
    let words = format!("{kind} {message}").to_ascii_lowercase();
    let detail = bounded_detail(if message.is_empty() { kind } else { message });
    let any = |needles: &[&str]| needles.iter().any(|needle| words.contains(needle));
    if any(&[
        "api_key",
        "api key",
        "unauthorized",
        "authenticat",
        "invalid_key",
    ]) {
        TtsError::Auth { detail }
    } else if any(&["quota", "rate_limit", "rate limit", "too_many", "concurren"]) {
        TtsError::RateLimited {
            retry_after: None,
            detail,
        }
    } else {
        TtsError::Provider {
            status: None,
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextId, ServerMessage, classify, close_context, context_text, flush, parse};
    use crate::speech_out::TtsError;

    #[test]
    fn client_messages_have_the_documented_shape() {
        let context = ContextId(7);
        let text: serde_json::Value =
            serde_json::from_str(&context_text(context, "Hi there.  ")).expect("json");
        assert_eq!(
            text,
            serde_json::json!({"text": "Hi there. ", "context_id": "c7"})
        );
        let flushed: serde_json::Value = serde_json::from_str(&flush(context)).expect("json");
        assert_eq!(
            flushed,
            serde_json::json!({"context_id": "c7", "flush": true})
        );
        let closed: serde_json::Value =
            serde_json::from_str(&close_context(context)).expect("json");
        assert_eq!(
            closed,
            serde_json::json!({"context_id": "c7", "close_context": true})
        );
    }

    #[test]
    fn audio_alignment_and_final_messages_parse_in_either_spelling() {
        let chunk = r#"{"audio":"AAA=","contextId":"c3","normalizedAlignment":null,
            "alignment":{"chars":["H","i"],"charStartTimesMs":[0,58],"charDurationsMs":[58,70]}}"#;
        assert_eq!(
            parse(chunk),
            Ok(ServerMessage::Chunk {
                context: Some(ContextId(3)),
                audio: Some("AAA=".into()),
                alignment: vec![("H".into(), 0.0), ("i".into(), 58.0)],
                is_final: false,
            })
        );
        for last in [
            r#"{"isFinal":true,"contextId":"c3"}"#,
            r#"{"is_final":true,"context_id":"c3","audio":null}"#,
            r#"{"isFinal":true,"contextId":"c3","audio":""}"#,
        ] {
            assert_eq!(
                parse(last),
                Ok(ServerMessage::Chunk {
                    context: Some(ContextId(3)),
                    audio: None,
                    alignment: Vec::new(),
                    is_final: true,
                }),
                "{last}"
            );
        }
        let foreign = r#"{"audio":"AAA=","contextId":"default"}"#;
        assert!(matches!(
            parse(foreign),
            Ok(ServerMessage::Chunk { context: None, .. })
        ));
    }

    #[test]
    fn errors_keep_their_category_and_bad_json_is_bad_output() {
        let quota = r#"{"message":"This request exceeds your quota.","error":"quota_exceeded","code":1008}"#;
        assert!(matches!(
            parse(quota),
            Ok(ServerMessage::Error {
                context: None,
                error: TtsError::RateLimited { .. }
            })
        ));
        assert!(matches!(
            classify("invalid_api_key", "Invalid API key"),
            TtsError::Auth { .. }
        ));
        assert!(matches!(
            classify("voice_not_found", "A voice with that id does not exist"),
            TtsError::Provider { status: None, .. }
        ));
        let in_context =
            r#"{"error":{"type":"input_error"},"message":"bad text","contextId":"c9"}"#;
        assert!(matches!(
            parse(in_context),
            Ok(ServerMessage::Error {
                context: Some(ContextId(9)),
                error: TtsError::Provider { .. }
            })
        ));
        assert!(matches!(parse("not json"), Err(TtsError::BadOutput { .. })));
        assert!(matches!(
            parse(r#"{"audio": 12}"#),
            Err(TtsError::BadOutput { .. })
        ));
    }
}
