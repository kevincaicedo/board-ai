//! Kokoro-82M v1.0 on ONNX Runtime (LLD §3.4, D-041): the local voice, 24 kHz, voice
//! `af_heart`, English pronunciation from [`g2p`] without espeak-ng.
//!
//! The export used (`onnx-community/Kokoro-82M-v1.0-ONNX-timestamped`) also returns each
//! token's predicted duration; a duration frame is 600 samples (25 ms), so word timings are
//! exact. Inference runs on one dedicated thread that owns the session; clauses queue on it.
//!
//! The padding token in front of every sequence is given about 350 ms of silence by the model.
//! Between clauses that would read as a stammer, so all but [`LEAD_IN_KEEP_FRAMES`] of it is
//! trimmed (and the word timings shifted with it).

mod g2p;
mod lexicon;
mod numbers;
mod rules;
pub(crate) mod vocab;

use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;

use boardai_core::input::WordTiming;
use futures_util::stream;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use tokio::sync::mpsc;

use self::g2p::Phonemized;
use self::lexicon::Lexicon;
use super::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};
use crate::cancel::CancelToken;
use crate::limits::KOKORO_TOKENS_MAX;

/// Audio samples per predicted duration frame at 24 kHz.
const SAMPLES_PER_FRAME: usize = 600;
/// Lead-in silence kept before the first phoneme (50 ms).
const LEAD_IN_KEEP_FRAMES: usize = 2;
/// Style vectors in a voice file: one per token count from 1 to 510.
const STYLE_ROWS: usize = 510;
const STYLE_VALUES: usize = 256;
/// CPU threads for one inference.
const INTRA_THREADS: usize = 4;
/// The only voice this build ships with (D-011: one default voice).
pub const KOKORO_VOICE: &str = "af_heart";

/// The files Kokoro needs, from the model store.
#[derive(Clone, Debug)]
pub struct KokoroFiles {
    pub model: PathBuf,
    pub voice: PathBuf,
    pub lexicon: PathBuf,
}

struct Request {
    text: String,
    chunks: mpsc::UnboundedSender<Result<TtsChunk, TtsError>>,
    cancel: CancelToken,
}

/// The Kokoro engine. Dropping it ends the inference thread after the clause in progress.
pub struct KokoroVoice {
    requests: Option<std_mpsc::Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl KokoroVoice {
    /// Loads the model, voice and lexicon on the inference thread and waits for it to be ready.
    pub fn new(files: KokoroFiles) -> Result<Self, TtsError> {
        let (requests, receiver) = std_mpsc::channel::<Request>();
        let (ready, loaded) = std_mpsc::channel::<Result<(), TtsError>>();
        let thread = std::thread::Builder::new()
            .name("kokoro".to_owned())
            .spawn(move || {
                let mut engine = match Engine::load(&files) {
                    Ok(engine) => {
                        let _ = ready.send(Ok(()));
                        engine
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                while let Ok(request) = receiver.recv() {
                    engine.serve(&request);
                }
            })
            .map_err(|error| unavailable(&error.to_string()))?;
        loaded
            .recv()
            .map_err(|_| unavailable("the Kokoro thread stopped while loading"))??;
        Ok(Self {
            requests: Some(requests),
            thread: Some(thread),
        })
    }
}

impl Drop for KokoroVoice {
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl TextToSpeech for KokoroVoice {
    fn synthesize(&self, clause: &ClauseText, voice: &VoiceId, cancel: CancelToken) -> TtsStream {
        let (chunks, receiver) = mpsc::unbounded_channel();
        let request = Request {
            text: clause.text.clone(),
            chunks: chunks.clone(),
            cancel,
        };
        if !voice.is_default() && voice.as_str() != KOKORO_VOICE {
            let _ = chunks.send(Err(unavailable("Kokoro has only the af_heart voice")));
        } else if self
            .requests
            .as_ref()
            .is_none_or(|requests| requests.send(request).is_err())
        {
            let _ = chunks.send(Err(unavailable("the Kokoro thread stopped")));
        }
        // The channel holds one clause: at most a few chunks of audio and its timings.
        Box::pin(stream::unfold(receiver, |mut receiver| async move {
            let item = receiver.recv().await?;
            Some((item, receiver))
        }))
    }
}

fn unavailable(detail: &str) -> TtsError {
    TtsError::Unavailable {
        detail: detail.to_owned(),
    }
}

/// What the inference thread owns.
struct Engine {
    session: Session,
    styles: Vec<f32>,
    lexicon: Lexicon,
}

impl Engine {
    fn load(files: &KokoroFiles) -> Result<Self, TtsError> {
        let model = |error: ort::Error| unavailable(&format!("Kokoro model: {error}"));
        let session = Session::builder()
            .map_err(model)?
            .with_intra_threads(INTRA_THREADS)
            .map_err(|error| model(error.into()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| model(error.into()))?
            .commit_from_file(&files.model)
            .map_err(model)?;
        let styles = load_styles(&files.voice)?;
        let lexicon = Lexicon::load(&files.lexicon).map_err(|error| unavailable(&error))?;
        Ok(Self {
            session,
            styles,
            lexicon,
        })
    }

    fn serve(&mut self, request: &Request) {
        if request.cancel.is_cancelled() {
            return;
        }
        let phonemized = g2p::phonemize(&request.text, &self.lexicon);
        let mut audio_samples = 0;
        let mut words = Vec::new();
        for chunk in split(&phonemized) {
            if request.cancel.is_cancelled() {
                return;
            }
            match self.infer(&chunk) {
                Ok((audio, starts)) => {
                    let offset_ms = audio_samples * 1_000 / 24_000;
                    words.extend(starts.into_iter().map(|word| WordTiming {
                        byte_offset: word.byte_offset,
                        start_ms: word.start_ms + u32::try_from(offset_ms).unwrap_or(u32::MAX),
                    }));
                    audio_samples += audio.len();
                    if request.chunks.send(Ok(TtsChunk::Audio(audio))).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = request.chunks.send(Err(error));
                    return;
                }
            }
        }
        let _ = request.chunks.send(Ok(TtsChunk::Words(words)));
    }

    /// One model run: the audio (lead-in trimmed) and the word starts within it.
    fn infer(&mut self, chunk: &Phonemized) -> Result<(Vec<f32>, Vec<WordTiming>), TtsError> {
        if chunk.tokens.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let failed = |error: ort::Error| TtsError::BadOutput {
            detail: format!("Kokoro inference: {error}"),
        };
        let mut ids = Vec::with_capacity(chunk.tokens.len() + 2);
        ids.push(0);
        ids.extend_from_slice(&chunk.tokens);
        ids.push(0);
        let row = chunk.tokens.len().clamp(1, STYLE_ROWS) - 1;
        let style = self.styles[row * STYLE_VALUES..(row + 1) * STYLE_VALUES].to_vec();
        let length = ids.len();
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => Tensor::from_array(([1_usize, length], ids)).map_err(failed)?,
                "style" => Tensor::from_array(([1_usize, STYLE_VALUES], style)).map_err(failed)?,
                "speed" => Tensor::from_array(([1_usize], vec![1.0_f32])).map_err(failed)?,
            ])
            .map_err(failed)?;
        let (_, waveform) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;
        let (_, durations) = outputs["durations"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;
        if durations.len() != length {
            return Err(TtsError::BadOutput {
                detail: format!("{} durations for {length} tokens", durations.len()),
            });
        }
        Ok(timed_audio(waveform, durations, &chunk.words))
    }
}

/// Word starts from durations, then the lead-in trimmed from audio and timings alike.
fn timed_audio(
    waveform: &[f32],
    durations: &[f32],
    words: &[(u32, usize)],
) -> (Vec<f32>, Vec<WordTiming>) {
    let frames: Vec<usize> = durations
        .iter()
        .map(|duration| duration.round_ties_even().max(1.0) as usize)
        .collect();
    // Where token `index` (model position `index + 1`) starts, in samples.
    let starts: Vec<usize> = frames
        .iter()
        .scan(0, |sum, frames| {
            let start = *sum;
            *sum += frames;
            Some(start * SAMPLES_PER_FRAME)
        })
        .collect();
    let expected = frames.iter().sum::<usize>() * SAMPLES_PER_FRAME;
    // The export's waveform matches the durations exactly; if a model ever disagrees, scale.
    let scale = if expected == 0 {
        1.0
    } else {
        waveform.len() as f64 / expected as f64
    };
    let trim = frames.first().map_or(0, |lead_in| {
        lead_in.saturating_sub(LEAD_IN_KEEP_FRAMES) * SAMPLES_PER_FRAME
    });
    let trim = ((trim as f64 * scale) as usize).min(waveform.len());
    let timings = words
        .iter()
        .filter_map(|(byte_offset, token_index)| {
            let start = *starts.get(token_index + 1)?;
            let start = ((start as f64 * scale) as usize).saturating_sub(trim);
            Some(WordTiming {
                byte_offset: *byte_offset,
                start_ms: u32::try_from(start * 1_000 / 24_000).unwrap_or(u32::MAX),
            })
        })
        .collect();
    (waveform[trim..].to_vec(), timings)
}

/// Splits a clause at spaces so each part fits the model's [`KOKORO_TOKENS_MAX`] tokens; a
/// single word longer than that is cut.
fn split(phonemized: &Phonemized) -> Vec<Phonemized> {
    let space = vocab::token(' ');
    let mut parts = Vec::new();
    let mut begin = 0;
    let tokens = &phonemized.tokens;
    while begin < tokens.len() {
        let mut end = (begin + KOKORO_TOKENS_MAX).min(tokens.len());
        if end < tokens.len()
            && let Some(cut) = tokens[begin..end].iter().rposition(|id| Some(*id) == space)
        {
            end = begin + cut.max(1);
        }
        let words = phonemized
            .words
            .iter()
            .filter(|(_, index)| (begin..end).contains(index))
            .map(|(offset, index)| (*offset, index - begin))
            .collect();
        parts.push(Phonemized {
            tokens: tokens[begin..end].to_vec(),
            words,
        });
        begin = end;
        while tokens.get(begin).is_some_and(|id| Some(*id) == space) {
            begin += 1;
        }
    }
    parts
}

/// A voice file: [`STYLE_ROWS`] × [`STYLE_VALUES`] little-endian `f32`.
fn load_styles(path: &Path) -> Result<Vec<f32>, TtsError> {
    let bytes =
        std::fs::read(path).map_err(|error| unavailable(&format!("Kokoro voice: {error}")))?;
    if bytes.len() != STYLE_ROWS * STYLE_VALUES * 4 {
        return Err(unavailable("the Kokoro voice file has the wrong size"));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{Phonemized, SAMPLES_PER_FRAME, split, timed_audio};
    use crate::limits::KOKORO_TOKENS_MAX;

    #[test]
    fn durations_become_word_starts_after_the_trimmed_lead_in() {
        // Pad 14 frames, three tokens of 2 frames, pad 1 frame.
        let durations = [13.6, 2.2, 1.5, 2.5, 0.2];
        let frames: usize = 14 + 2 + 2 + 2 + 1;
        let waveform = vec![0.1; frames * SAMPLES_PER_FRAME];
        let words = [(0, 0), (6, 2)];
        let (audio, timings) = timed_audio(&waveform, &durations, &words);
        // 12 lead-in frames (300 ms) are trimmed, 2 kept.
        assert_eq!(audio.len(), waveform.len() - 12 * SAMPLES_PER_FRAME);
        let starts: Vec<(u32, u32)> = timings
            .iter()
            .map(|w| (w.byte_offset, w.start_ms))
            .collect();
        // Token 0 starts after the kept lead-in (50 ms); token 2 after two more tokens (100 ms).
        assert_eq!(starts, vec![(0, 50), (6, 150)]);
    }

    #[test]
    fn long_clauses_split_at_spaces_within_the_limit() {
        let space = super::vocab::token(' ').unwrap_or_default();
        let mut tokens = Vec::new();
        let mut words = Vec::new();
        for word in 0..200_u32 {
            words.push((word * 4, tokens.len()));
            tokens.extend([43, 44, 45, space]);
        }
        let parts = split(&Phonemized { tokens, words });
        assert!(parts.len() >= 2);
        assert!(
            parts
                .iter()
                .all(|part| part.tokens.len() <= KOKORO_TOKENS_MAX)
        );
        assert!(parts.iter().all(|part| part.tokens.first() != Some(&space)));
        let total_words: usize = parts.iter().map(|part| part.words.len()).sum();
        assert_eq!(total_words, 200);
        assert_eq!(parts[1].words[0].1, 0, "indices restart in each part");
    }
}
