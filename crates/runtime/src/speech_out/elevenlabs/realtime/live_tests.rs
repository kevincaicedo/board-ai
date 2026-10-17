//! The live check against ElevenLabs (`just live-elevenlabs`): three short clauses over the
//! realtime WebSocket (the first on a cold connection) and one over HTTP, printing the time to
//! first audio, the alignment against the audio, and the characters billed. Never part of
//! `just check`: it is ignored and needs `BOARDAI_LIVE=1` and `ELEVENLABS_API_KEY` in the
//! environment. The key is only ever placed in the handshake's sensitive header.
//!
//! It records every server message, with the audio replaced by its length, to
//! `target/elevenlabs-live/messages.jsonl`, which keeps the test fixtures true to the wire.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use boardai_core::ids::ClauseId;
use futures_util::StreamExt;

use super::ElevenLabsRealtime;
use super::connection::Outcome;
use crate::cancel::CancelToken;
use crate::settings::Secret;
use crate::speech_out::elevenlabs::ElevenLabsOptions;
use crate::speech_out::timings::{duration_ms, word_starts};
use crate::speech_out::{ClauseText, TextToSpeech, TtsChunk, VoiceId};

const REALTIME_CLAUSES: [&str; 3] = [
    "Redis keeps the session in memory.",
    "Postgres stores it, so a restart loses nothing.",
    "Then the cache warms up again.",
];
const HTTP_CLAUSE: &str = "The worker reads the queue every second.";

fn key() -> Option<Secret> {
    if std::env::var("BOARDAI_LIVE").ok().as_deref() != Some("1") {
        println!("skipped: set BOARDAI_LIVE=1 and ELEVENLABS_API_KEY");
        return None;
    }
    std::env::var("ELEVENLABS_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .map(|key| Secret::new(key.trim().to_owned()))
}

fn clause(index: usize, text: &str) -> ClauseText {
    ClauseText {
        id: ClauseId(u32::try_from(index).unwrap_or(0)),
        text: text.to_owned(),
    }
}

/// Where the voice starts and ends: 10 ms blocks at or above a tenth of the loudest block.
fn voiced_span_ms(audio: &[f32]) -> (u32, u32) {
    let block = 240;
    let levels: Vec<f32> = audio
        .chunks(block)
        .map(|samples| {
            (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32)
                .sqrt()
        })
        .collect();
    let loudest = levels.iter().copied().fold(0.0_f32, f32::max);
    let voiced = |level: &f32| *level >= loudest * 0.1;
    let first = levels.iter().position(voiced).unwrap_or(0);
    let last = levels.iter().rposition(voiced).unwrap_or(0);
    (first as u32 * 10, (last as u32 + 1) * 10)
}

/// Speaks one clause and prints what it measured; `Err` says what was wrong with it.
async fn realtime_clause(
    engine: &ElevenLabsRealtime,
    index: usize,
    text: &str,
) -> Result<(), String> {
    let started = Instant::now();
    let voice = engine.speak(
        &clause(index, text),
        super::super::ELEVENLABS_DEFAULT_VOICE_ID,
    );
    let waiting = voice.ok_or("the realtime connection did not start")?;
    let outcome = tokio::time::timeout(Duration::from_secs(20), waiting.outcome)
        .await
        .map_err(|_| "no answer in time".to_owned())?
        .map_err(|_| "the connection dropped the clause".to_owned())?;
    let total = started.elapsed();
    let Outcome::Complete {
        audio,
        words,
        timing,
    } = outcome
    else {
        return Err(format!("clause {index} failed: {outcome:?}"));
    };
    let length_ms = duration_ms(audio.len());
    let (onset_ms, voiced_end_ms) = voiced_span_ms(&audio);
    println!(
        "realtime clause {index}: {} chars, {length_ms} ms of audio; cold connection: {}; \
         from the call: answer {} ms; from sending: first audio {:?} ms, last audio {:?} ms, \
         final {} ms, {} messages",
        text.chars().count(),
        timing.fresh_connection,
        total.as_millis(),
        timing.first_audio.map(|at| at.as_millis()),
        timing.last_audio.map(|at| at.as_millis()),
        timing.finished.as_millis(),
        timing.messages,
    );
    let words = words.ok_or(format!("clause {index}: no exact timings"))?;
    let first_ms = words.first().map_or(0, |word| word.start_ms);
    let last_ms = words.last().map_or(0, |word| word.start_ms);
    println!(
        "  alignment: first word at {first_ms} ms (voice onset {onset_ms} ms), last word at \
         {last_ms} ms (voice ends {voiced_end_ms} ms); starts {:?}",
        words.iter().map(|word| word.start_ms).collect::<Vec<_>>()
    );
    if words.len() != word_starts(text).len() {
        return Err(format!("clause {index}: not every word is timed"));
    }
    if !words
        .windows(2)
        .all(|pair| pair[0].start_ms <= pair[1].start_ms)
    {
        return Err(format!("clause {index}: word starts go back"));
    }
    if last_ms >= voiced_end_ms {
        return Err(format!(
            "clause {index}: the last word starts after the voice"
        ));
    }
    Ok(())
}

async fn http_clause(engine: &ElevenLabsRealtime) {
    let started = Instant::now();
    let mut stream = engine.http.synthesize(
        &clause(9, HTTP_CLAUSE),
        &VoiceId::default(),
        CancelToken::new(),
    );
    let mut first_audio = None;
    let mut frames = 0;
    let mut timed = false;
    while let Some(chunk) = stream.next().await {
        match chunk.expect("the HTTP clause") {
            TtsChunk::Audio(samples) => {
                first_audio.get_or_insert_with(|| started.elapsed());
                frames += samples.len();
            }
            TtsChunk::Words(_) => timed = true,
        }
    }
    println!(
        "http clause: {} chars, {} ms of audio, first audio {:?} ms, complete {} ms, exact \
         timings: {timed}",
        HTTP_CLAUSE.chars().count(),
        duration_ms(frames),
        first_audio.map(|at| at.as_millis()),
        started.elapsed().as_millis()
    );
}

/// The recorded messages, audio replaced by its length so the file stays small and holds no
/// voice.
fn write_record(messages: &[String]) {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/elevenlabs-live");
    let lines: Vec<String> = messages
        .iter()
        .map(|message| {
            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(message) else {
                return message.clone();
            };
            if let Some(audio) = value.get("audio").and_then(serde_json::Value::as_str) {
                value["audio"] = format!("<{} base64 bytes>", audio.len()).into();
            }
            value.to_string()
        })
        .collect();
    let written = std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::write(folder.join("messages.jsonl"), lines.join("\n") + "\n"));
    match written {
        Ok(()) => println!("recorded {} messages in {}", lines.len(), folder.display()),
        Err(error) => println!("could not record the messages: {error}"),
    }
}

#[tokio::test]
#[ignore = "live: needs BOARDAI_LIVE=1 and ELEVENLABS_API_KEY"]
async fn elevenlabs_realtime_live() {
    let Some(api_key) = key() else {
        return;
    };
    let engine = ElevenLabsRealtime::new(ElevenLabsOptions {
        base_url: None,
        api_key,
        model: None,
    })
    .expect("the engine builds");
    let (recorder, recorded) = std::sync::mpsc::channel();
    assert!(engine.connection_with(Some(recorder)).is_some());
    let mut problems = Vec::new();
    for (index, text) in REALTIME_CLAUSES.iter().enumerate() {
        if let Err(problem) = realtime_clause(&engine, index, text).await {
            problems.push(problem);
            // A clause that failed says enough; the rest would only cost characters.
            break;
        }
    }
    if problems.is_empty() {
        http_clause(&engine).await;
    }
    let characters: usize = REALTIME_CLAUSES
        .iter()
        .chain([&HTTP_CLAUSE])
        .map(|text| text.chars().count())
        .sum();
    println!("characters in this run's clauses: {characters} (fewer were sent if one failed)");
    write_record(&recorded.try_iter().collect::<Vec<_>>());
    assert!(problems.is_empty(), "{problems:?}");
}
