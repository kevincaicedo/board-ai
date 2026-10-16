//! The macOS system voice as a [`TextToSpeech`] engine (LLD §3.4): no download, exact word
//! timings from the synthesizer's markers. It is the first voice the app uses and the fallback
//! whenever another engine fails.
//!
//! The synthesizer's callbacks run on the main queue, so the process's main thread must run an
//! event loop: the app's does; headless binaries call `audio::run_main_loop_until`. Without one
//! a clause fails with `Unavailable` after [`SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS`].
//!
//! [`SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS`]: crate::limits::SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS

use std::sync::Arc;

use boardai_core::input::WordTiming;
use futures_util::stream;
use tokio::sync::mpsc;

use super::timings::utf16_to_byte;
use super::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};
use crate::audio::VOICE_RATE_HZ;
use crate::audio::coreaudio::system_voice::{
    SpeechEvent, SystemSynthesizer, SystemVoiceFailure, voices,
};
use crate::audio::resample::Resampler;
use crate::cancel::CancelToken;

pub use crate::audio::coreaudio::system_voice::SystemVoiceInfo;

/// The installed system voices for American English, best quality first, for the settings
/// screen. Which one sounds best is the owner's call.
#[must_use]
pub fn system_voices() -> Vec<SystemVoiceInfo> {
    voices("en-US")
}

/// The system voice engine. Cheap to share; one synthesizer thread serves every clause.
pub struct SystemVoice {
    synthesizer: Arc<SystemSynthesizer>,
}

/// What the voice says, into nowhere, to warm up.
const WARM_UP_TEXT: &str = "Hello.";

impl SystemVoice {
    /// Starts the synthesizer thread.
    pub fn new() -> Result<Self, TtsError> {
        let synthesizer = SystemSynthesizer::spawn().map_err(|error| TtsError::Unavailable {
            detail: format!("the system voice thread did not start: {error}"),
        })?;
        Ok(Self {
            synthesizer: Arc::new(synthesizer),
        })
    }

    /// Says a word into nowhere, so loading `voice` does not delay the first clause: the first
    /// utterance of a process took 512–628 ms to synthesize in `just e2e`, later ones 43–122 ms.
    /// It returns at once; the synthesizer thread does it before any clause queued after it.
    pub fn warm_up(&self, voice: &VoiceId) {
        let identifier = (!voice.is_default()).then(|| voice.as_str().to_owned());
        // The events are not wanted: the receiver is dropped, and the thread's sends fail
        // silently.
        let _events =
            self.synthesizer
                .speak(WARM_UP_TEXT.to_owned(), identifier, CancelToken::new());
    }
}

impl TextToSpeech for SystemVoice {
    fn synthesize(&self, clause: &ClauseText, voice: &VoiceId, cancel: CancelToken) -> TtsStream {
        let identifier = (!voice.is_default()).then(|| voice.as_str().to_owned());
        let events = self
            .synthesizer
            .speak(clause.text.clone(), identifier, cancel.clone());
        let state = Decoder {
            events,
            text: clause.text.clone(),
            resampler: None,
            source_rate_hz: 0,
            bytes_per_frame: 4,
            words: Vec::new(),
            done: false,
            cancel,
        };
        Box::pin(stream::unfold(state, |mut state| async move {
            let item = state.next().await?;
            Some((item, state))
        }))
    }
}

/// Turns synthesizer events into chunks: audio converted to 24 kHz as it arrives, and every
/// word timing at the end, when the output rate is known for certain.
struct Decoder {
    events: mpsc::UnboundedReceiver<SpeechEvent>,
    text: String,
    resampler: Option<Resampler>,
    source_rate_hz: u32,
    bytes_per_frame: u32,
    /// UTF-16 index and byte offset into the output, as the markers give them.
    words: Vec<(usize, u64)>,
    done: bool,
    cancel: CancelToken,
}

impl Decoder {
    async fn next(&mut self) -> Option<Result<TtsChunk, TtsError>> {
        if self.done || self.cancel.is_cancelled() {
            return None;
        }
        loop {
            let event = tokio::select! {
                event = self.events.recv() => event,
                () = self.cancel.cancelled() => return None,
            };
            let Some(event) = event else {
                self.done = true;
                return Some(Err(TtsError::Unavailable {
                    detail: "the system voice stopped".to_owned(),
                }));
            };
            match event {
                SpeechEvent::Audio {
                    samples,
                    rate_hz,
                    bytes_per_frame,
                } => return Some(self.audio(&samples, rate_hz, bytes_per_frame)),
                SpeechEvent::Word {
                    utf16_index,
                    byte_offset,
                } => self.words.push((utf16_index, byte_offset)),
                SpeechEvent::Finished => {
                    self.done = true;
                    return Some(Ok(TtsChunk::Words(self.word_timings())));
                }
                SpeechEvent::Failed(failure) => {
                    self.done = true;
                    return Some(Err(error(&failure)));
                }
            }
        }
    }

    fn audio(
        &mut self,
        samples: &[f32],
        rate_hz: f64,
        bytes_per_frame: u32,
    ) -> Result<TtsChunk, TtsError> {
        let rate = rate_hz.round();
        if !(1.0..=f64::from(u32::MAX)).contains(&rate) {
            return Err(TtsError::BadOutput {
                detail: "the system voice reported no sample rate".to_owned(),
            });
        }
        let rate = rate as u32;
        if self.resampler.is_none() || self.source_rate_hz != rate {
            let resampler =
                Resampler::new(rate, VOICE_RATE_HZ).map_err(|_| TtsError::BadOutput {
                    detail: format!("the system voice writes an unsupported {rate} Hz"),
                })?;
            self.resampler = Some(resampler);
            self.source_rate_hz = rate;
        }
        self.bytes_per_frame = bytes_per_frame.max(1);
        let mut converted = Vec::with_capacity(samples.len() * 2);
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.push(samples, |sample| converted.push(sample));
        }
        Ok(TtsChunk::Audio(converted))
    }

    /// Markers converted: UTF-16 index to byte offset, output bytes to milliseconds.
    fn word_timings(&self) -> Vec<WordTiming> {
        let rate = u64::from(self.source_rate_hz.max(1));
        self.words
            .iter()
            .filter_map(|(utf16_index, byte_offset)| {
                let byte = utf16_to_byte(&self.text, *utf16_index)?;
                let frames = byte_offset / u64::from(self.bytes_per_frame);
                let start_ms = u32::try_from(frames * 1_000 / rate).ok()?;
                Some(WordTiming {
                    byte_offset: byte,
                    start_ms,
                })
            })
            .collect()
    }
}

fn error(failure: &SystemVoiceFailure) -> TtsError {
    let detail = match failure {
        SystemVoiceFailure::UnknownVoice => "no system voice has that identifier",
        SystemVoiceFailure::NoAudio => {
            "the system voice sent no audio; the main thread must run an event loop"
        }
        SystemVoiceFailure::TimedOut => return TtsError::Timeout,
        SystemVoiceFailure::UnsupportedFormat => {
            return TtsError::BadOutput {
                detail: "the system voice wrote an unsupported sample format".to_owned(),
            };
        }
        SystemVoiceFailure::Stopped => "the system voice stopped",
    };
    TtsError::Unavailable {
        detail: detail.to_owned(),
    }
}
