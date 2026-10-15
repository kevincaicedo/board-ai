//! ElevenLabs text-to-speech over HTTP streaming with timestamps (LLD §3.4).
//!
//! `POST /v1/text-to-speech/{voice_id}/stream/with-timestamps?output_format=pcm_24000` answers
//! with newline-delimited JSON objects (the official SDK splits lines and parses each), each
//! carrying `audio_base64` (16-bit little-endian PCM at 24 kHz, the voice ring's rate) and an
//! optional `alignment` of the original text's characters with start times. The alignment
//! becomes exact word timings; when it does not match the clause text the clause falls back to
//! estimated timings rather than guessing.
//!
//! The key comes from the [`SecretStore`](crate::settings::SecretStore) and is only placed in
//! a sensitive header.

use std::collections::VecDeque;

use boardai_core::input::WordTiming;
use futures_util::stream;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

use super::timings::word_starts;
use super::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};
use crate::audio::VOICE_RATE_HZ;
use crate::cancel::CancelToken;
use crate::limits::{
    ELEVENLABS_LINE_MAX_BYTES, ELEVENLABS_RESPONSE_MAX_BYTES, ELEVENLABS_VOICE_ID_MAX_BYTES,
};
use crate::llm::http::{build_client, endpoint, error_from_response};
use crate::llm::{ProviderError, bounded_detail};
use crate::settings::Secret;

pub const ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io";

/// The voice used when settings say `default`: "Rachel", one of ElevenLabs' premade voices.
/// Which voice ships as the default is the owner's call (D-011); this keeps the engine usable.
pub const ELEVENLABS_DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";

#[derive(Clone, Debug)]
pub struct ElevenLabsOptions {
    pub base_url: Option<String>,
    pub api_key: Secret,
    /// `None` leaves the model to ElevenLabs' default.
    pub model: Option<String>,
}

pub struct ElevenLabsVoice {
    client: reqwest::Client,
    base_url: String,
    headers: HeaderMap,
    model: Option<String>,
}

impl ElevenLabsVoice {
    pub fn new(options: ElevenLabsOptions) -> Result<Self, TtsError> {
        let base_url = options.base_url.as_deref().unwrap_or(ELEVENLABS_BASE_URL);
        endpoint(base_url, "v1").map_err(from_provider)?;
        let mut key =
            HeaderValue::from_str(options.api_key.expose()).map_err(|_| TtsError::Auth {
                detail: "the ElevenLabs key has characters that cannot be sent".to_owned(),
            })?;
        key.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("xi-api-key"), key);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(Self {
            client: build_client().map_err(from_provider)?,
            base_url: base_url.trim_end_matches('/').to_owned(),
            headers,
            model: options.model,
        })
    }

    fn request(
        &self,
        clause: &ClauseText,
        voice: &VoiceId,
    ) -> Result<reqwest::RequestBuilder, TtsError> {
        let voice_id = if voice.is_default() {
            ELEVENLABS_DEFAULT_VOICE_ID
        } else {
            voice.as_str()
        };
        let valid = !voice_id.is_empty()
            && voice_id.len() <= ELEVENLABS_VOICE_ID_MAX_BYTES
            && voice_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !valid {
            return Err(TtsError::Provider {
                status: None,
                detail: "the ElevenLabs voice id is not valid".to_owned(),
            });
        }
        let url = format!(
            "{}/v1/text-to-speech/{voice_id}/stream/with-timestamps?output_format=pcm_24000",
            self.base_url
        );
        let mut body = serde_json::json!({ "text": clause.text });
        if let Some(model) = &self.model {
            body["model_id"] = serde_json::Value::String(model.clone());
        }
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
            let response = request.send().await.map_err(network)?;
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
        match response.chunk().await.map_err(network)? {
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
pub(crate) struct Decoder {
    text: String,
    line: Vec<u8>,
    received_bytes: usize,
    audio_frames: usize,
    /// Each aligned character and its start, in seconds from the start of the clause.
    characters: Vec<(String, f64)>,
}

impl Decoder {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            line: Vec::new(),
            received_bytes: 0,
            audio_frames: 0,
            characters: Vec::new(),
        }
    }

    pub(crate) fn feed(
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
    pub(crate) fn finish(&mut self, out: &mut VecDeque<TtsChunk>) -> Result<(), TtsError> {
        let line = std::mem::take(&mut self.line);
        self.line_done(&line, out)?;
        if let Some(words) = self.word_timings() {
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
        let chunk_start_seconds = self.audio_frames as f64 / f64::from(VOICE_RATE_HZ);
        if let Some(audio) = chunk.audio_base64.filter(|audio| !audio.is_empty()) {
            let bytes =
                decode_base64(&audio).ok_or_else(|| bad_output("audio is not valid base64"))?;
            let samples: Vec<f32> = bytes
                .chunks_exact(2)
                .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                .collect();
            self.audio_frames += samples.len();
            out.push_back(TtsChunk::Audio(samples));
        }
        if let Some(alignment) = chunk.alignment {
            self.align(alignment, chunk_start_seconds);
        }
        Ok(())
    }

    /// The docs do not say whether a chunk's times count from the clause or from the chunk;
    /// times that go back to zero are taken as relative to the chunk's first frame.
    fn align(&mut self, alignment: WireAlignment, chunk_start_seconds: f64) {
        let last = self.characters.last().map_or(0.0, |(_, start)| *start);
        let first = alignment
            .character_start_times_seconds
            .first()
            .copied()
            .unwrap_or(0.0);
        let shift = if first + 1e-3 < last {
            chunk_start_seconds
        } else {
            0.0
        };
        let starts = alignment.character_start_times_seconds.into_iter();
        for (character, start) in alignment.characters.into_iter().zip(starts) {
            self.characters.push((character, start + shift));
        }
    }

    /// Word starts in the clause text from the character alignment, when the aligned
    /// characters spell exactly the clause text.
    fn word_timings(&self) -> Option<Vec<WordTiming>> {
        if self.characters.is_empty() {
            return None;
        }
        let mut spelled = String::new();
        // Byte offset in `spelled` where each aligned character begins, with its start.
        let mut starts: Vec<(usize, f64)> = Vec::with_capacity(self.characters.len());
        for (character, start) in &self.characters {
            starts.push((spelled.len(), *start));
            spelled.push_str(character);
        }
        if spelled != self.text {
            return None;
        }
        let words = word_starts(&self.text)
            .into_iter()
            .filter_map(|offset| {
                let index = starts.partition_point(|(begin, _)| *begin < offset as usize);
                let (begin, start) = starts.get(index)?;
                (*begin == offset as usize).then(|| WordTiming {
                    byte_offset: offset,
                    start_ms: (start.max(0.0) * 1_000.0).round() as u32,
                })
            })
            .collect();
        Some(words)
    }
}

/// Standard base64 with optional padding; `None` for anything else.
pub(crate) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let trimmed = text.trim_end_matches('=');
    if text.len() - trimmed.len() > 2 || trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    for group in trimmed.as_bytes().chunks(4) {
        let mut bits: u32 = 0;
        for byte in group {
            bits = (bits << 6) | value(*byte)?;
        }
        bits <<= 6 * (4 - group.len() as u32);
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..group.len()]);
    }
    Some(out)
}

fn bad_output(detail: &str) -> TtsError {
    TtsError::BadOutput {
        detail: detail.to_owned(),
    }
}

fn network(error: reqwest::Error) -> TtsError {
    if error.is_timeout() {
        TtsError::Timeout
    } else {
        TtsError::Network {
            detail: bounded_detail(&error.to_string()),
        }
    }
}

/// Keeps the category of an HTTP failure.
fn from_provider(error: ProviderError) -> TtsError {
    match error {
        ProviderError::Auth { detail } => TtsError::Auth { detail },
        ProviderError::RateLimited {
            retry_after,
            detail,
        }
        | ProviderError::Overloaded {
            retry_after,
            detail,
        } => TtsError::RateLimited {
            retry_after,
            detail,
        },
        ProviderError::Network { detail } => TtsError::Network { detail },
        ProviderError::Timeout { .. } => TtsError::Timeout,
        ProviderError::BadOutput { detail } => TtsError::BadOutput { detail },
        ProviderError::InvalidRequest { status, detail } => TtsError::Provider { status, detail },
        ProviderError::Server { status, detail } => TtsError::Provider {
            status: Some(status),
            detail,
        },
        ProviderError::Provider { detail, .. } => TtsError::Provider {
            status: None,
            detail,
        },
        ProviderError::NotConfigured { detail } => TtsError::Unavailable { detail },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{Decoder, TtsChunk, decode_base64};

    #[test]
    fn base64_decodes_with_and_without_padding() {
        assert_eq!(decode_base64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode_base64("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(decode_base64("AAEC/w=="), Some(vec![0, 1, 2, 255]));
        assert_eq!(decode_base64(""), Some(Vec::new()));
        assert_eq!(decode_base64("a"), None);
        assert_eq!(decode_base64("ab!c"), None);
        assert_eq!(decode_base64("ab==="), None);
    }

    fn line(audio: &[i16], characters: &[&str], starts: &[f64]) -> String {
        let bytes: Vec<u8> = audio
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let audio = base64(&bytes);
        let alignment = serde_json::json!({
            "characters": characters,
            "character_start_times_seconds": starts,
            "character_end_times_seconds": starts,
        });
        format!(
            "{}\n",
            serde_json::json!({"audio_base64": audio, "alignment": alignment})
        )
    }

    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for group in bytes.chunks(3) {
            let mut padded = [0_u8; 3];
            padded[..group.len()].copy_from_slice(group);
            let bits = u32::from_be_bytes([0, padded[0], padded[1], padded[2]]);
            for index in 0..=group.len() {
                out.push(TABLE[((bits >> (18 - 6 * index)) & 63) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn lines_split_anywhere_give_audio_then_word_timings() {
        let body = format!(
            "{}{}",
            line(&[16_384, -16_384], &["H", "i", " "], &[0.0, 0.05, 0.1]),
            line(&[0], &["y", "o", "u"], &[0.2, 0.25, 0.3]),
        );
        for split in 1..body.len() {
            let mut decoder = Decoder::new("Hi you");
            let mut out = VecDeque::new();
            assert!(decoder.feed(&body.as_bytes()[..split], &mut out).is_ok());
            assert!(decoder.feed(&body.as_bytes()[split..], &mut out).is_ok());
            assert!(decoder.finish(&mut out).is_ok());
            let chunks: Vec<TtsChunk> = out.into_iter().collect();
            assert_eq!(chunks.len(), 3, "split at {split}");
            assert_eq!(chunks[0], TtsChunk::Audio(vec![0.5, -0.5]));
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
