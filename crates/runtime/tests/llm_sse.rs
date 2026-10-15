//! The SSE parser against every recorded fixture: golden event counts, and the split-invariance
//! property (the same events and the same error wherever the byte stream is cut).
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use boardai_runtime::llm::sse::{SseError, SseEvent, SseParser};

/// Every SSE fixture and the number of events it holds.
const FIXTURES: &[(&str, usize)] = &[
    ("anthropic/plain_text.sse", 9),
    ("anthropic/tool_use.sse", 14),
    ("anthropic/thinking.sse", 13),
    ("anthropic/refusal.sse", 9),
    ("anthropic/max_tokens.sse", 8),
    ("anthropic/error_mid_stream.sse", 4),
    ("anthropic/error_before_token.sse", 3),
    ("anthropic/malformed.sse", 3),
    ("anthropic/fallback.sse", 17),
    ("openai/ollama_text.sse", 27),
    ("openai/ollama_tool.sse", 41),
    ("openai/fragmented_tool.sse", 9),
    ("openai/length.sse", 2),
    ("openai/error_chunk.sse", 2),
];

fn parse_chunks<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> (Vec<SseEvent>, Option<SseError>) {
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    for chunk in chunks {
        if let Err(error) = parser.push(chunk, &mut events) {
            return (events, Some(error));
        }
    }
    (events, None)
}

fn with_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut converted = Vec::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        if byte == b'\n' {
            converted.push(b'\r');
        }
        converted.push(byte);
    }
    converted
}

#[test]
fn golden_event_counts() {
    for (name, expected) in FIXTURES {
        let bytes = support::fixture(name);
        let (events, error) = parse_chunks([bytes.as_slice()]);
        assert_eq!(error, None, "{name}");
        assert_eq!(events.len(), *expected, "{name}");
        assert!(events.iter().all(|event| !event.data.is_empty()), "{name}");
    }
}

#[test]
fn every_split_point_gives_the_same_events() {
    for (name, _) in FIXTURES {
        for bytes in [support::fixture(name), with_crlf(&support::fixture(name))] {
            let whole = parse_chunks([bytes.as_slice()]);
            for split in 0..=bytes.len() {
                let (head, tail) = bytes.split_at(split);
                assert_eq!(parse_chunks([head, tail]), whole, "{name} split at {split}");
            }
            let bytewise = parse_chunks(bytes.chunks(1));
            assert_eq!(bytewise, whole, "{name} byte by byte");
        }
    }
}

#[test]
fn errors_land_at_the_same_byte_wherever_the_stream_is_cut() {
    let mut bytes = support::fixture("anthropic/plain_text.sse");
    bytes.extend_from_slice(b"data: \xFF\xFE\n\n");
    let whole = parse_chunks([bytes.as_slice()]);
    assert_eq!(whole.1, Some(SseError::InvalidUtf8));
    assert_eq!(whole.0.len(), 9);
    for split in 0..=bytes.len() {
        let (head, tail) = bytes.split_at(split);
        assert_eq!(parse_chunks([head, tail]), whole, "split at {split}");
    }
}
