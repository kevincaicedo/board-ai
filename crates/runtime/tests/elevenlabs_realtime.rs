//! The ElevenLabs realtime engine against a local WebSocket server that speaks the message shapes
//! recorded live (`fixtures/elevenlabs/realtime_clause.jsonl`: camelCase keys, `isFinal: null`
//! on audio, alignment times that go on across a clause's messages, a final message with
//! `audio: null`), and serves the HTTP fallback from the same address. No network.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::ids::ClauseId;
use boardai_core::input::WordTiming;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::settings::Secret;
use boardai_runtime::speech_out::{
    ClauseText, ELEVENLABS_DEFAULT_VOICE_ID, ElevenLabsOptions, ElevenLabsRealtime, TextToSpeech,
    TtsChunk, TtsError, VoiceId,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::fixture;
use support::websocket::{Pending, ServerSocket, WsServer};

const TEXT: &str = "Hi there, café.";

type Log = Arc<Mutex<Vec<Value>>>;

fn engine(server: &WsServer) -> ElevenLabsRealtime {
    ElevenLabsRealtime::new(ElevenLabsOptions {
        base_url: Some(server.base_url.clone()),
        api_key: Secret::new("xi-test-key".to_owned()),
        model: None,
    })
    .expect("engine builds")
}

fn clause(id: u32) -> ClauseText {
    ClauseText {
        id: ClauseId(id),
        text: TEXT.to_owned(),
    }
}

async fn speak(engine: &ElevenLabsRealtime, id: u32) -> Vec<Result<TtsChunk, TtsError>> {
    let stream = engine.synthesize(&clause(id), &VoiceId::default(), CancelToken::new());
    tokio::time::timeout(Duration::from_secs(20), stream.collect())
        .await
        .expect("the clause ends")
}

/// The fixture's messages for one context, in order.
fn clause_messages(context: &str) -> Vec<String> {
    String::from_utf8(fixture("elevenlabs/realtime_clause.jsonl"))
        .expect("UTF-8")
        .lines()
        .map(|line| line.replace("{context}", context))
        .collect()
}

/// What a clause sounds like when it arrives whole: the fixture's 300 ms, then its timings.
fn assert_fixture_clause(chunks: &[Result<TtsChunk, TtsError>]) {
    let [Ok(TtsChunk::Audio(audio)), Ok(TtsChunk::Words(words))] = chunks else {
        panic!("audio then words: {chunks:?}")
    };
    assert_eq!(audio.len(), 7_200, "100 + 200 ms at 24 kHz");
    let expected = [(0, 0), (3, 80), (10, 100)].map(|(byte_offset, start_ms)| WordTiming {
        byte_offset,
        start_ms,
    });
    assert_eq!(words.as_slice(), expected.as_slice());
}

/// The HTTP fallback's answer (`with_timestamps.jsonl`): 600 ms and its own timings.
fn assert_http_clause(chunks: &[Result<TtsChunk, TtsError>]) {
    let frames: usize = chunks
        .iter()
        .map(|chunk| match chunk {
            Ok(TtsChunk::Audio(samples)) => samples.len(),
            Ok(TtsChunk::Words(_)) => 0,
            Err(error) => panic!("{error}"),
        })
        .sum();
    assert_eq!(frames, 14_400);
}

fn context_of(message: &Value) -> String {
    message["context_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Plays ElevenLabs: answers each closed context with the fixture, logging what it received.
async fn elevenlabs(mut socket: ServerSocket, log: Log) {
    while let Some(text) = socket.recv().await {
        let message: Value = serde_json::from_str(&text).expect("client JSON");
        log.lock().expect("lock").push(message.clone());
        if message["close_context"] == json!(true) {
            for line in clause_messages(&context_of(&message)) {
                socket.send_text(&line).await;
            }
        }
    }
}

/// Answers an HTTP request with the `with-timestamps` fixture.
async fn http_fallback(pending: Pending) {
    let body = fixture("elevenlabs/with_timestamps.jsonl");
    pending.respond(200, "application/json", &body).await;
}

/// Reads until the client has closed `count` contexts, returning their ids in order.
async fn read_closes(socket: &mut ServerSocket, count: usize) -> Vec<String> {
    let mut closed = Vec::new();
    while closed.len() < count {
        let Some(text) = socket.recv().await else {
            break;
        };
        let message: Value = serde_json::from_str(&text).expect("client JSON");
        if message["close_context"] == json!(true) {
            closed.push(context_of(&message));
        }
    }
    closed
}

fn posts(server: &WsServer) -> usize {
    server.requests().len() - server.upgrades().len()
}

#[tokio::test]
async fn clauses_share_one_connection_and_the_alignment_becomes_exact_timings() {
    let log: Log = Arc::default();
    let server_log = Arc::clone(&log);
    let server = WsServer::start(move |pending| {
        let log = Arc::clone(&server_log);
        async move { elevenlabs(pending.accept().await, log).await }
    })
    .await;
    let engine = engine(&server);
    assert_fixture_clause(&speak(&engine, 7).await);
    assert_fixture_clause(&speak(&engine, 8).await);
    let upgrades = server.upgrades();
    assert_eq!(upgrades.len(), 1, "one connection for both clauses");
    let handshake = &upgrades[0];
    let expected_path = format!(
        "/v1/text-to-speech/{ELEVENLABS_DEFAULT_VOICE_ID}/multi-stream-input?\
         model_id=eleven_flash_v2_5&output_format=pcm_24000&sync_alignment=true&auto_mode=true\
         &inactivity_timeout=180"
    );
    assert_eq!(handshake.path, expected_path);
    assert_eq!(handshake.header("xi-api-key"), Some("xi-test-key"));
    assert_eq!(handshake.header("sec-websocket-version"), Some("13"));
    let sent = log.lock().expect("lock").clone();
    let expected: Vec<Value> = ["c0", "c1"]
        .iter()
        .flat_map(|context| {
            [
                json!({"text": "Hi there, café. ", "context_id": context}),
                json!({"context_id": context, "flush": true}),
                json!({"context_id": context, "close_context": true}),
            ]
        })
        .collect();
    assert_eq!(sent, expected);
    assert_eq!(posts(&server), 0);
}

#[tokio::test]
async fn warming_up_connects_before_the_first_clause() {
    let server = WsServer::start(|pending| async move {
        elevenlabs(pending.accept().await, Arc::default()).await;
    })
    .await;
    let engine = engine(&server);
    engine.prepare(&VoiceId::default());
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.upgrades().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("connected ahead");
    assert_fixture_clause(&speak(&engine, 1).await);
    assert_eq!(server.upgrades().len(), 1);
}

#[tokio::test]
async fn a_refused_key_fails_the_clause_without_trying_http() {
    let server = WsServer::start(|pending| async move {
        let body = br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#;
        pending.respond(401, "application/json", body).await;
    })
    .await;
    let chunks = speak(&engine(&server), 1).await;
    assert!(
        matches!(chunks.as_slice(), [Err(TtsError::Auth { .. })]),
        "{chunks:?}"
    );
    assert_eq!(posts(&server), 0, "HTTP would be refused the same way");
}

#[tokio::test]
async fn an_unreachable_websocket_sends_clauses_over_http_and_rests() {
    let server = WsServer::start(|pending| async move {
        if pending.is_upgrade() {
            pending
                .respond(503, "application/json", br#"{"detail":"down"}"#)
                .await;
        } else {
            http_fallback(pending).await;
        }
    })
    .await;
    let engine = engine(&server);
    assert_http_clause(&speak(&engine, 1).await);
    assert_http_clause(&speak(&engine, 2).await);
    assert_eq!(
        server.upgrades().len(),
        1,
        "the WebSocket rests after failing"
    );
    assert_eq!(posts(&server), 2);
}

#[tokio::test]
async fn a_connection_dropped_mid_clause_retries_it_over_http_then_reconnects() {
    let log: Log = Arc::default();
    let server_log = Arc::clone(&log);
    let server = WsServer::start(move |pending| {
        let log = Arc::clone(&server_log);
        async move {
            if !pending.is_upgrade() {
                return http_fallback(pending).await;
            }
            let first = pending.index == 0;
            let mut socket = pending.accept().await;
            if first {
                let closed = read_closes(&mut socket, 1).await;
                let messages = clause_messages(&closed[0]);
                socket.send_text(&messages[0]).await;
                socket.drop_connection();
            } else {
                elevenlabs(socket, log).await;
            }
        }
    })
    .await;
    let engine = engine(&server);
    assert_http_clause(&speak(&engine, 1).await);
    assert_fixture_clause(&speak(&engine, 2).await);
    assert_eq!(
        server.upgrades().len(),
        2,
        "a dropped socket is reopened, not rested"
    );
    assert_eq!(posts(&server), 1);
}

#[tokio::test]
async fn an_idle_connection_closed_by_the_server_is_reopened() {
    let server = WsServer::start(|pending| async move {
        let mut socket = pending.accept().await;
        let closed = read_closes(&mut socket, 1).await;
        for line in clause_messages(&closed[0]) {
            socket.send_text(&line).await;
        }
        socket.send_close(1000, "inactivity").await;
        while socket.recv().await.is_some() {}
    })
    .await;
    let engine = engine(&server);
    assert_fixture_clause(&speak(&engine, 1).await);
    // Let the close arrive before the next clause, as it would after a pause.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_fixture_clause(&speak(&engine, 2).await);
    assert_eq!(server.upgrades().len(), 2);
    assert_eq!(posts(&server), 0);
}

#[tokio::test]
async fn a_clause_stopped_after_it_was_sent_is_forgotten_and_its_late_audio_ignored() {
    let first_sent = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&first_sent);
    let server = WsServer::start(move |pending| {
        let notify = Arc::clone(&notify);
        async move {
            let mut socket = pending.accept().await;
            let mut closed = read_closes(&mut socket, 1).await;
            notify.notify_one();
            // The stopped clause's audio arrives only after the next clause was sent.
            closed.extend(read_closes(&mut socket, 1).await);
            for context in &closed {
                for line in clause_messages(context) {
                    socket.send_text(&line).await;
                }
            }
            while socket.recv().await.is_some() {}
        }
    })
    .await;
    let engine = engine(&server);
    let stopped = engine.synthesize(&clause(1), &VoiceId::default(), CancelToken::new());
    first_sent.notified().await;
    drop(stopped);
    assert_fixture_clause(&speak(&engine, 2).await);
    assert_eq!(server.upgrades().len(), 1);
}

#[tokio::test]
async fn a_clause_stopped_before_it_was_sent_never_reaches_the_server() {
    let log: Log = Arc::default();
    let server_log = Arc::clone(&log);
    let server = WsServer::start(move |pending| {
        let log = Arc::clone(&server_log);
        async move { elevenlabs(pending.accept().await, log).await }
    })
    .await;
    let engine = engine(&server);
    drop(engine.synthesize(&clause(1), &VoiceId::default(), CancelToken::new()));
    assert_fixture_clause(&speak(&engine, 2).await);
    let contexts: Vec<String> = log.lock().expect("lock").iter().map(context_of).collect();
    assert_eq!(
        contexts,
        ["c1", "c1", "c1"],
        "nothing of the stopped clause is billed"
    );
}

#[tokio::test]
async fn a_stalled_server_times_the_clause_out_and_the_connection_is_replaced() {
    let server = WsServer::start(|pending| async move {
        let stalls = pending.index == 0;
        let mut socket = pending.accept().await;
        if stalls {
            while socket.recv().await.is_some() {}
        } else {
            elevenlabs(socket, Arc::default()).await;
        }
    })
    .await;
    let engine = engine(&server);
    let started = std::time::Instant::now();
    let chunks = speak(&engine, 1).await;
    assert!(
        matches!(chunks.as_slice(), [Err(TtsError::Timeout)]),
        "{chunks:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_fixture_clause(&speak(&engine, 2).await);
    assert_eq!(server.upgrades().len(), 2);
}

#[tokio::test]
async fn a_slow_but_steady_server_still_completes_the_clause() {
    let server = WsServer::start(|pending| async move {
        let mut socket = pending.accept().await;
        let closed = read_closes(&mut socket, 1).await;
        socket.send_ping(b"still here").await;
        // Without the client's pong the clause is never answered.
        assert_eq!(
            socket.recv_pong().await.as_deref(),
            Some(&b"still here"[..])
        );
        for line in clause_messages(&closed[0]) {
            tokio::time::sleep(Duration::from_millis(600)).await;
            socket.send_text(&line).await;
        }
        while socket.recv().await.is_some() {}
    })
    .await;
    assert_fixture_clause(&speak(&engine(&server), 1).await);
}

#[tokio::test]
async fn a_quota_error_fails_the_clause_with_its_category() {
    let server = WsServer::start(|pending| async move {
        if !pending.is_upgrade() {
            return http_fallback(pending).await;
        }
        let mut socket = pending.accept().await;
        read_closes(&mut socket, 1).await;
        let refusal = json!({
            "message": "This request exceeds your quota of 10000.",
            "error": "quota_exceeded",
            "code": 1008
        });
        socket.send_text(&refusal.to_string()).await;
        socket.send_close(1008, "quota_exceeded").await;
        while socket.recv().await.is_some() {}
    })
    .await;
    let chunks = speak(&engine(&server), 1).await;
    assert!(
        matches!(chunks.as_slice(), [Err(TtsError::RateLimited { .. })]),
        "{chunks:?}"
    );
    assert_eq!(posts(&server), 0);
}

#[tokio::test]
async fn a_policy_close_alone_still_fails_the_clause() {
    let server = WsServer::start(|pending| async move {
        if !pending.is_upgrade() {
            return http_fallback(pending).await;
        }
        let mut socket = pending.accept().await;
        read_closes(&mut socket, 1).await;
        socket.send_close(1008, "quota_exceeded").await;
        while socket.recv().await.is_some() {}
    })
    .await;
    let chunks = speak(&engine(&server), 1).await;
    assert!(
        matches!(chunks.as_slice(), [Err(TtsError::RateLimited { .. })]),
        "{chunks:?}"
    );
    assert_eq!(posts(&server), 0);
}

/// Serves one clause's upgrade with `bad` instead of an answer, and HTTP from the fixture.
async fn malformed_then_http(bad: &'static [u8]) {
    let server = WsServer::start(move |pending| async move {
        if !pending.is_upgrade() {
            return http_fallback(pending).await;
        }
        let mut socket = pending.accept().await;
        read_closes(&mut socket, 1).await;
        socket.send_raw(bad).await;
        while socket.recv().await.is_some() {}
    })
    .await;
    let engine = engine(&server);
    assert_http_clause(&speak(&engine, 1).await);
    assert_http_clause(&speak(&engine, 2).await);
    assert_eq!(server.upgrades().len(), 1, "resting after bad output");
    assert_eq!(posts(&server), 2);
}

#[tokio::test]
async fn a_wrong_accept_key_is_refused_and_the_clause_goes_over_http() {
    let server = WsServer::start(|pending| async move {
        if !pending.is_upgrade() {
            return http_fallback(pending).await;
        }
        let mut socket = pending.accept_wrongly().await;
        while socket.recv().await.is_some() {}
    })
    .await;
    let engine = engine(&server);
    assert_http_clause(&speak(&engine, 1).await);
    assert_http_clause(&speak(&engine, 2).await);
    assert_eq!(server.upgrades().len(), 1, "resting after a bad handshake");
}

#[tokio::test]
async fn a_message_that_is_not_json_sends_the_clause_over_http_and_rests_the_websocket() {
    malformed_then_http(b"\x81\x08not json").await;
}

#[tokio::test]
async fn a_masked_server_frame_sends_the_clause_over_http_and_rests_the_websocket() {
    malformed_then_http(&[0x81, 0x81, 1, 2, 3, 4, 0x41]).await;
}

#[tokio::test]
async fn an_alignment_that_does_not_spell_the_clause_gives_no_timings() {
    let server = WsServer::start(|pending| async move {
        let mut socket = pending.accept().await;
        let closed = read_closes(&mut socket, 1).await;
        for line in clause_messages(&closed[0]) {
            socket.send_text(&line.replace("\"é\"", "\"e\"")).await;
        }
        while socket.recv().await.is_some() {}
    })
    .await;
    let chunks = speak(&engine(&server), 1).await;
    assert!(
        matches!(chunks.as_slice(), [Ok(TtsChunk::Audio(audio))] if audio.len() == 7_200),
        "audio only, so speech out estimates: {chunks:?}"
    );
}

#[tokio::test]
async fn clauses_past_the_context_limit_go_over_http() {
    let server = WsServer::start(|pending| async move {
        if !pending.is_upgrade() {
            return http_fallback(pending).await;
        }
        let mut socket = pending.accept().await;
        let closed = read_closes(&mut socket, 5).await;
        for context in &closed {
            for line in clause_messages(context) {
                socket.send_text(&line).await;
            }
        }
        while socket.recv().await.is_some() {}
    })
    .await;
    let engine = engine(&server);
    let streams: Vec<_> = (0..6)
        .map(|id| engine.synthesize(&clause(id), &VoiceId::default(), CancelToken::new()))
        .collect();
    let mut over_http = 0;
    for stream in streams {
        let chunks: Vec<_> = tokio::time::timeout(Duration::from_secs(20), stream.collect())
            .await
            .expect("the clause ends");
        let frames: usize = chunks
            .iter()
            .map(|chunk| match chunk {
                Ok(TtsChunk::Audio(samples)) => samples.len(),
                Ok(TtsChunk::Words(_)) | Err(_) => 0,
            })
            .sum();
        if frames == 14_400 {
            over_http += 1;
        } else {
            assert_fixture_clause(&chunks);
        }
    }
    assert_eq!(over_http, 1);
    assert_eq!(posts(&server), 1);
}

#[tokio::test]
async fn a_bad_voice_id_is_refused_before_anything_is_sent() {
    let server = WsServer::start(|pending| async move {
        elevenlabs(pending.accept().await, Arc::default()).await;
    })
    .await;
    let stream = engine(&server).synthesize(&clause(1), &VoiceId::new("../x"), CancelToken::new());
    let chunks: Vec<_> = stream.collect().await;
    assert!(matches!(
        chunks.as_slice(),
        [Err(TtsError::Provider { status: None, .. })]
    ));
    assert!(server.requests().is_empty());
}
