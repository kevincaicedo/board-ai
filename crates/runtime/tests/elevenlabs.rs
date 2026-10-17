//! The ElevenLabs HTTP engine (the realtime engine's fallback) against the local test server,
//! with a fixture in the documented `stream/with-timestamps` shape. No network.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

mod support;

use std::time::Duration;

use boardai_core::ids::ClauseId;
use boardai_core::input::WordTiming;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::settings::Secret;
use boardai_runtime::speech_out::{
    ClauseText, ELEVENLABS_DEFAULT_VOICE_ID, ElevenLabsOptions, ElevenLabsVoice, TextToSpeech,
    TtsChunk, TtsError, VoiceId,
};
use futures_util::StreamExt;
use support::{Scripted, TestServer, fixture};

const TEXT: &str = "Hi there, café.";

fn voice(server: &TestServer, model: Option<&str>) -> ElevenLabsVoice {
    ElevenLabsVoice::new(ElevenLabsOptions {
        base_url: Some(server.base_url.clone()),
        api_key: Secret::new("xi-test-key".to_owned()),
        model: model.map(str::to_owned),
    })
    .expect("adapter builds")
}

fn clause() -> ClauseText {
    ClauseText {
        id: ClauseId(7),
        text: TEXT.to_owned(),
    }
}

async fn collect(voice: &ElevenLabsVoice, id: &VoiceId) -> Vec<Result<TtsChunk, TtsError>> {
    voice
        .synthesize(&clause(), id, CancelToken::new())
        .collect()
        .await
}

fn json_lines(body: &[u8], pieces: usize) -> Scripted {
    let mut scripted = Scripted::sse(body, pieces);
    scripted.headers = vec![("content-type".into(), "application/json".into())];
    scripted
}

#[tokio::test]
async fn audio_streams_and_the_alignment_becomes_exact_word_timings() {
    let server = TestServer::start(vec![json_lines(
        &fixture("elevenlabs/with_timestamps.jsonl"),
        7,
    )])
    .await;
    let chunks = collect(
        &voice(&server, Some("eleven_flash_v2_5")),
        &VoiceId::default(),
    )
    .await;
    let chunks: Vec<TtsChunk> = chunks
        .into_iter()
        .map(|chunk| chunk.expect("chunk"))
        .collect();
    let frames: usize = chunks
        .iter()
        .map(|chunk| match chunk {
            TtsChunk::Audio(samples) => samples.len(),
            TtsChunk::Words(_) => 0,
        })
        .sum();
    // 100 + 200 + 300 ms at 24 kHz.
    assert_eq!(frames, 14_400);
    let Some(TtsChunk::Words(words)) = chunks.last() else {
        panic!("word timings come last")
    };
    let expected = [(0, 0), (3, 150), (10, 500)].map(|(byte_offset, start_ms)| WordTiming {
        byte_offset,
        start_ms,
    });
    assert_eq!(words.as_slice(), expected.as_slice());
    let request = &server.requests()[0];
    assert_eq!(
        request.path,
        format!(
            "/v1/text-to-speech/{ELEVENLABS_DEFAULT_VOICE_ID}/stream/with-timestamps?output_format=pcm_24000"
        )
    );
    assert_eq!(request.header("xi-api-key"), Some("xi-test-key"));
    let body = request.json();
    assert_eq!(body["text"], TEXT);
    assert_eq!(body["model_id"], "eleven_flash_v2_5");
}

#[tokio::test]
async fn samples_are_converted_from_signed_16_bit() {
    let line = "{\"audio_base64\":\"AEAAwP9/AIA=\"}\n";
    let server = TestServer::start(vec![json_lines(line.as_bytes(), 1)]).await;
    let chunks = collect(&voice(&server, None), &VoiceId::new("abcDEF123")).await;
    assert_eq!(
        chunks.first(),
        Some(&Ok(TtsChunk::Audio(vec![
            0.5,
            -0.5,
            32_767.0 / 32_768.0,
            -1.0
        ])))
    );
    // No alignment: no `Words`, so speech out estimates.
    assert_eq!(chunks.len(), 1);
    let request = &server.requests()[0];
    assert!(request.path.starts_with("/v1/text-to-speech/abcDEF123/"));
    assert_eq!(
        request.json()["model_id"],
        "eleven_flash_v2_5",
        "Flash by default"
    );
}

#[tokio::test]
async fn errors_keep_their_category() {
    let script = vec![
        Scripted::status(
            401,
            r#"{"detail":{"status":"invalid_api_key","message":"bad key"}}"#,
        ),
        Scripted::status(429, "{}").with_header("retry-after", "2"),
        Scripted::status(
            422,
            r#"{"detail":[{"loc":["body","text"],"msg":"too long"}]}"#,
        ),
    ];
    let server = TestServer::start(script).await;
    let voice = voice(&server, None);
    let first = collect(&voice, &VoiceId::default()).await;
    assert!(
        matches!(first.as_slice(), [Err(TtsError::Auth { .. })]),
        "{first:?}"
    );
    let second = collect(&voice, &VoiceId::default()).await;
    assert!(
        matches!(
            second.as_slice(),
            [Err(TtsError::RateLimited { retry_after: Some(after), .. })] if *after == Duration::from_secs(2)
        ),
        "{second:?}"
    );
    let third = collect(&voice, &VoiceId::default()).await;
    assert!(
        matches!(
            third.as_slice(),
            [Err(TtsError::Provider {
                status: Some(422),
                ..
            })]
        ),
        "{third:?}"
    );
}

#[tokio::test]
async fn malformed_output_and_bad_voice_ids_fail_out_loud() {
    let server = TestServer::start(vec![json_lines(b"{\"audio_base64\": 12}\n", 1)]).await;
    let voice = voice(&server, None);
    let malformed = collect(&voice, &VoiceId::default()).await;
    assert!(
        matches!(malformed.as_slice(), [Err(TtsError::BadOutput { .. })]),
        "{malformed:?}"
    );
    let refused = collect(&voice, &VoiceId::new("../admin")).await;
    assert!(matches!(
        refused.as_slice(),
        [Err(TtsError::Provider { status: None, .. })]
    ));
    assert_eq!(
        server.requests().len(),
        1,
        "an invalid voice id is never sent"
    );
}

#[tokio::test]
async fn cancelling_ends_the_stream_and_the_connection() {
    let line = "{\"audio_base64\":\"AEAAwA==\"}\n";
    let server = TestServer::start(vec![json_lines(line.as_bytes(), 1).hanging()]).await;
    let voice = voice(&server, None);
    let cancel = CancelToken::new();
    let mut stream = voice.synthesize(&clause(), &VoiceId::default(), cancel.clone());
    assert!(matches!(stream.next().await, Some(Ok(TtsChunk::Audio(_)))));
    cancel.cancel();
    assert!(stream.next().await.is_none());
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), server.client_disconnected())
        .await
        .expect("the connection closes");
}
