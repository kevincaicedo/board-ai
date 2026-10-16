//! whisper.cpp through `whisper-rs` with Metal (D-042): `base.en` by default, `small.en`
//! optional. Each decode covers the current utterance only, greedy, without timestamps or
//! context from earlier utterances; the vocabulary goes in as the initial prompt.

use std::path::Path;
use std::sync::Once;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use super::{SpeechToText, SttError};
use crate::audio::MIC_RATE_HZ;
use crate::limits::{STT_UTTERANCE_MAX_MS, STT_VOCABULARY_TOKENS_MAX};

/// Whisper is trained on at least a second of audio; shorter input is padded with silence so a
/// one-word utterance is not dropped.
const MIN_DECODE_FRAMES: usize = MIC_RATE_HZ as usize;

/// CPU threads for the parts whisper.cpp does not run on the GPU.
const DECODE_THREADS: i32 = 4;

const UTTERANCE_MAX_FRAMES: usize =
    (STT_UTTERANCE_MAX_MS as usize) * (MIC_RATE_HZ as usize) / 1_000;

static LOG_HOOKS: Once = Once::new();

/// Where whisper.cpp runs: Metal (the default), or the CPU only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhisperDevice {
    Gpu,
    Cpu,
}

/// The encoder always sees its full 30 s window (the utterance padded with silence). Shrinking
/// it to the utterance (`audio_ctx`) was measured in spike S4 and rejected: on base.en a 2.5 s
/// question came back as the same sentence repeated eight times, and decodes took up to 1.9 s.
pub struct WhisperStt {
    context: WhisperContext,
    state: WhisperState,
    audio: Vec<f32>,
    /// The vocabulary, tokenized and bounded.
    prompt: Vec<i32>,
    /// Frames dropped past [`STT_UTTERANCE_MAX_MS`] in the current utterance.
    dropped_frames: usize,
}

impl WhisperStt {
    pub fn new(model_path: &Path) -> Result<Self, SttError> {
        Self::new_on(model_path, WhisperDevice::Gpu)
    }

    /// On a chosen device. Speech in uses the GPU: on the CPU a partial took 166 ms against
    /// 37 ms, and 195 ms against 57 ms while Ollama generated on the GPU (`spike_turns`).
    pub fn new_on(model_path: &Path, device: WhisperDevice) -> Result<Self, SttError> {
        // whisper.cpp logs to stderr by default; route it to `tracing` once per process.
        LOG_HOOKS.call_once(whisper_rs::install_logging_hooks);
        let model = |error: whisper_rs::WhisperError| SttError::Model {
            detail: error.to_string(),
        };
        let mut parameters = WhisperContextParameters::default();
        parameters.use_gpu(device == WhisperDevice::Gpu);
        let context = WhisperContext::new_with_params(model_path, parameters).map_err(model)?;
        let state = context.create_state().map_err(model)?;
        Ok(Self {
            context,
            state,
            audio: Vec::with_capacity(UTTERANCE_MAX_FRAMES),
            prompt: Vec::new(),
            dropped_frames: 0,
        })
    }

    fn decode(&mut self) -> Result<String, SttError> {
        let failed = |error: whisper_rs::WhisperError| SttError::Decode {
            detail: error.to_string(),
        };
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(DECODE_THREADS);
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        params.set_suppress_blank(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_tokens(&self.prompt);
        let padding = MIN_DECODE_FRAMES.saturating_sub(self.audio.len());
        self.audio.extend(std::iter::repeat_n(0.0, padding));
        let outcome = self.state.full(params, &self.audio);
        self.audio.truncate(self.audio.len() - padding);
        outcome.map_err(failed)?;
        let mut text = String::new();
        for segment in self.state.as_iter() {
            text.push_str(&segment.to_str_lossy().map_err(failed)?);
        }
        Ok(text.trim().to_owned())
    }
}

impl SpeechToText for WhisperStt {
    fn push(&mut self, pcm: &[f32]) {
        let room = UTTERANCE_MAX_FRAMES.saturating_sub(self.audio.len());
        let kept = pcm.len().min(room);
        self.audio.extend_from_slice(&pcm[..kept]);
        self.dropped_frames += pcm.len() - kept;
    }

    fn partial(&mut self) -> Result<Option<String>, SttError> {
        if self.audio.is_empty() {
            return Ok(None);
        }
        let text = self.decode()?;
        Ok((!text.is_empty()).then_some(text))
    }

    fn finish(&mut self) -> Result<String, SttError> {
        if self.dropped_frames > 0 {
            tracing::warn!(
                dropped_frames = self.dropped_frames,
                "utterance longer than the speech-to-text window; the rest was dropped"
            );
        }
        let text = if self.audio.is_empty() {
            Ok(String::new())
        } else {
            self.decode()
        };
        self.audio.clear();
        self.dropped_frames = 0;
        text
    }

    fn discard(&mut self) {
        self.audio.clear();
        self.dropped_frames = 0;
    }

    fn set_vocabulary(&mut self, terms: &[String]) {
        self.prompt = vocabulary_prompt(terms, |text| {
            self.context
                .tokenize(text, STT_VOCABULARY_TOKENS_MAX + 1)
                .map(|tokens| tokens.into_iter().collect())
                .unwrap_or_default()
        });
    }
}

/// Terms joined with ", ", in order, while they fit in [`STT_VOCABULARY_TOKENS_MAX`] tokens; a
/// term that would cross the limit and every term after it are left out.
fn vocabulary_prompt(terms: &[String], tokenize: impl Fn(&str) -> Vec<i32>) -> Vec<i32> {
    let mut prompt = String::new();
    let mut tokens = Vec::new();
    for term in terms
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
    {
        let candidate = if prompt.is_empty() {
            term.to_owned()
        } else {
            format!("{prompt}, {term}")
        };
        let candidate_tokens = tokenize(&candidate);
        if candidate_tokens.len() > STT_VOCABULARY_TOKENS_MAX {
            break;
        }
        prompt = candidate;
        tokens = candidate_tokens;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::vocabulary_prompt;
    use crate::limits::STT_VOCABULARY_TOKENS_MAX;

    /// One token per character: easy to count.
    fn tokenize(text: &str) -> Vec<i32> {
        text.chars().map(|character| character as i32).collect()
    }

    #[test]
    fn vocabulary_keeps_whole_terms_within_the_limit() {
        let terms: Vec<String> = ["Redis", " ", "getUserById"]
            .iter()
            .map(|t| (*t).into())
            .collect();
        let prompt = vocabulary_prompt(&terms, tokenize);
        let text: String = prompt
            .iter()
            .filter_map(|t| char::from_u32(*t as u32))
            .collect();
        assert_eq!(text, "Redis, getUserById");
        let many: Vec<String> = (0..100).map(|index| format!("term{index:03}")).collect();
        let prompt = vocabulary_prompt(&many, tokenize);
        assert!(prompt.len() <= STT_VOCABULARY_TOKENS_MAX);
        // Each term adds 9 tokens ("term000" plus ", "): 22 fit in 200.
        assert_eq!(prompt.len(), 7 + 21 * 9);
    }
}
