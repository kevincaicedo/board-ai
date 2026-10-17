//! ElevenLabs over HTTP streaming with timestamps, one request per clause: the realtime
//! engine's fallback (LLD §3.4).
//!
//! `POST /v1/text-to-speech/{voice_id}/stream/with-timestamps?output_format=pcm_24000` answers
//! with newline-delimited JSON objects (the official SDK splits lines and parses each), each
//! carrying `audio_base64` and an optional `alignment` of the original text's characters with
//! start times in seconds.

use std::collections::VecDeque;

use futures_util::stream;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;

use super::alignment::Alignment;
use super::{
    ElevenLabsOptions, OUTPUT_FORMAT, bad_output, from_provider, network, pcm_from_base64,
    voice_path,
};
use crate::audio::VOICE_RATE_HZ;
use crate::cancel::CancelToken;
use crate::limits::{ELEVENLABS_LINE_MAX_BYTES, ELEVENLABS_RESPONSE_MAX_BYTES};
use crate::llm::http::{build_client, endpoint, error_from_response};
use crate::speech_out::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};

pub struct ElevenLabsVoice {
    client: reqwest::Client,
    base_url: String,
    headers: HeaderMap,
    model: String,
}

impl ElevenLabsVoice {
    pub fn new(options: ElevenLabsOptions) -> Result<Self, TtsError> {
        endpoint(options.base_url(), "v1").map_err(from_provider)?;
        let mut headers = options.key_headers()?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(Self {
            client: build_client().map_err(from_provider)?,
            base_url: options.base_url().to_owned(),
            headers,
            model: options.model().to_owned(),
        })
    }

    fn request(
        &self,
        clause: &ClauseText,
        voice: &VoiceId,
    ) -> Result<reqwest::RequestBuilder, TtsError> {
        let voice_id = voice_path(voice)?;
        let url = format!(
            "{}/v1/text-to-speech/{voice_id}/stream/with-timestamps?output_format={OUTPUT_FORMAT}",
            self.base_url
        );
        let body = serde_json::json!({ "text": clause.text, "model_id": self.model });
        Ok(self
            .client
            .post(url)
            .headers(self.headers.clone())
            .body(body.to_string()))
    }
}

impl TextToSpeech for ElevenLabsVoice {
    fn synthesize(&self, clause: &ClauseText, voice: &VoiceId, cancel: CancelToken) -> TtsStream {
        let request = match self.request(clause, voice) {
            Ok(request) => request,
            Err(error) => return Box::pin(stream::once(std::future::ready(Err(error)))),
        };
        let state = Session {
            request: Some(request),
            response: None,
            decoder: Decoder::new(&clause.text),
            pending: VecDeque::new(),
            cancel,
            finished: false,
        };
        Box::pin(stream::unfold(state, |mut state| async move {
            let item = state.next().await?;
            Some((item, state))
        }))
    }
}

struct Session {
    request: Option<reqwest::RequestBuilder>,
    response: Option<reqwest::Response>,
    decoder: Decoder,
    pending: VecDeque<TtsChunk>,
    cancel: CancelToken,
    finished: bool,
}

impl Session {
    async fn next(&mut self) -> Option<Result<TtsChunk, TtsError>> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Some(Ok(chunk));
            }
            if self.finished || self.cancel.is_cancelled() {
                return None;
            }
            let cancel = self.cancel.clone();
            let step = tokio::select! {
                step = self.step() => step,
                () = cancel.cancelled() => return None,
            };
            if let Err(error) = step {
                self.finished = true;
                return Some(Err(error));
            }
        }
    }

    /// Sends the request, or reads the next piece of the body into `pending`.
    async fn step(&mut self) -> Result<(), TtsError> {
        if let Some(request) = self.request.take() {
            let response = request.send().await.map_err(|error| network(&error))?;
            if !response.status().is_success() {
                return Err(from_provider(error_from_response(response).await));
            }
            self.response = Some(response);
            return Ok(());
        }
        let Some(response) = self.response.as_mut() else {
            self.finished = true;
            return Ok(());
        };
        match response.chunk().await.map_err(|error| network(&error))? {
            Some(bytes) => self.decoder.feed(&bytes, &mut self.pending),
            None => {
                self.decoder.finish(&mut self.pending)?;
                self.finished = true;
                Ok(())
            }
        }
    }
}

#[derive(Deserialize)]
struct WireChunk {
    audio_base64: Option<String>,
    alignment: Option<WireAlignment>,
}

#[derive(Deserialize)]
struct WireAlignment {
    characters: Vec<String>,
    character_start_times_seconds: Vec<f64>,
}

/// Splits the body into lines and turns each into audio; collects the alignment for the end.
pub(super) struct Decoder {
    text: String,
    line: Vec<u8>,
    received_bytes: usize,
    audio_frames: usize,
    alignment: Alignment,
}

impl Decoder {
    pub(super) fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            line: Vec::new(),
            received_bytes: 0,
            audio_frames: 0,
            alignment: Alignment::default(),
        }
    }

    pub(super) fn feed(
        &mut self,
        bytes: &[u8],
        out: &mut VecDeque<TtsChunk>,
    ) -> Result<(), TtsError> {
        self.received_bytes += bytes.len();
        if self.received_bytes > ELEVENLABS_RESPONSE_MAX_BYTES {
            return Err(bad_output("the response is larger than its limit"));
        }
        for byte in bytes {
            if *byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.line_done(&line, out)?;
            } else if self.line.len() >= ELEVENLABS_LINE_MAX_BYTES {
                return Err(bad_output("a line is longer than its limit"));
            } else {
                self.line.push(*byte);
            }
        }
        Ok(())
    }

    /// The body ended: a last line without a newline still counts, then the word timings.
    pub(super) fn finish(&mut self, out: &mut VecDeque<TtsChunk>) -> Result<(), TtsError> {
        let line = std::mem::take(&mut self.line);
        self.line_done(&line, out)?;
        let audio_ms = self.audio_frames as f64 * 1_000.0 / f64::from(VOICE_RATE_HZ);
        if let Some(words) = self.alignment.word_timings(&self.text, audio_ms) {
            out.push_back(TtsChunk::Words(words));
        }
        Ok(())
    }

    fn line_done(&mut self, line: &[u8], out: &mut VecDeque<TtsChunk>) -> Result<(), TtsError> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let chunk: WireChunk = serde_json::from_slice(line)
            .map_err(|error| bad_output(&format!("a chunk is not valid JSON: {error}")))?;
        let chunk_start_ms = self.audio_frames as f64 * 1_000.0 / f64::from(VOICE_RATE_HZ);
        if let Some(audio) = chunk.audio_base64.filter(|audio| !audio.is_empty()) {
            let samples = pcm_from_base64(&audio)?;
            self.audio_frames += samples.len();
            out.push_back(TtsChunk::Audio(samples));
        }
        if let Some(alignment) = chunk.alignment {
            let starts_ms = alignment
                .character_start_times_seconds
                .into_iter()
                .map(|seconds| seconds * 1_000.0);
            let characters = alignment.characters.into_iter().zip(starts_ms);
            self.alignment.push_message(characters, chunk_start_ms);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{Decoder, TtsChunk};
    use crate::speech_out::base64;

    fn line(audio: &[i16], characters: &[&str], starts: &[f64]) -> String {
        let bytes: Vec<u8> = audio
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let alignment = serde_json::json!({
            "characters": characters,
            "character_start_times_seconds": starts,
            "character_end_times_seconds": starts,
        });
        format!(
            "{}\n",
            serde_json::json!({"audio_base64": base64::encode(&bytes), "alignment": alignment})
        )
    }

    #[test]
    fn lines_split_anywhere_give_audio_then_word_timings() {
        // 100 ms, then 300 ms; the second line's times go on from the first's.
        let mut first = vec![0_i16; 2_400];
        first[..2].copy_from_slice(&[16_384, -16_384]);
        let body = format!(
            "{}{}",
            line(&first, &["H", "i", " "], &[0.0, 0.05, 0.1]),
            line(&[0; 7_200], &["y", "o", "u"], &[0.2, 0.25, 0.3]),
        );
        for split in (1..body.len()).step_by(97).chain(1..64) {
            let mut decoder = Decoder::new("Hi you");
            let mut out = VecDeque::new();
            assert!(decoder.feed(&body.as_bytes()[..split], &mut out).is_ok());
            assert!(decoder.feed(&body.as_bytes()[split..], &mut out).is_ok());
            assert!(decoder.finish(&mut out).is_ok());
            let chunks: Vec<TtsChunk> = out.into_iter().collect();
            assert_eq!(chunks.len(), 3, "split at {split}");
            let TtsChunk::Audio(audio) = &chunks[0] else {
                panic!("audio first")
            };
            assert_eq!((audio.len(), audio[0], audio[1]), (2_400, 0.5, -0.5));
            let TtsChunk::Words(words) = &chunks[2] else {
                panic!("words last")
            };
            let pairs: Vec<(u32, u32)> =
                words.iter().map(|w| (w.byte_offset, w.start_ms)).collect();
            assert_eq!(pairs, vec![(0, 0), (3, 200)]);
        }
    }

    #[test]
    fn an_alignment_that_does_not_spell_the_text_gives_no_timings() {
        let mut decoder = Decoder::new("Hi you");
        let mut out = VecDeque::new();
        let body = line(&[0], &["H", "e", "y"], &[0.0, 0.1, 0.2]);
        assert!(decoder.feed(body.as_bytes(), &mut out).is_ok());
        assert!(decoder.finish(&mut out).is_ok());
        assert!(out.iter().all(|chunk| matches!(chunk, TtsChunk::Audio(_))));
    }

    #[test]
    fn malformed_lines_fail_with_bad_output() {
        let mut decoder = Decoder::new("x");
        let mut out = VecDeque::new();
        assert!(decoder.feed(b"{not json}\n", &mut out).is_err());
        let mut decoder = Decoder::new("x");
        assert!(
            decoder
                .feed(b"{\"audio_base64\":\"@@@@\"}\n", &mut out)
                .is_err()
        );
    }
}
