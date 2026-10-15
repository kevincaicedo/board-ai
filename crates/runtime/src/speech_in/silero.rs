//! Silero VAD v5 on ONNX Runtime (D-041): one 512-sample window at a time, with the model's
//! recurrent state and the 64 samples before the window carried between calls, as the
//! reference implementation does.
//!
//! whisper.cpp also ships Silero, but its `whisper_vad_detect_speech` resets the recurrent
//! state on every call (and allocates and logs per call), so it cannot follow a live stream in
//! 32 ms steps; it is built for whole files.

use std::path::Path;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorRef};

use super::{VAD_WINDOW_FRAMES, VadError, VoiceDetector};
use crate::audio::MIC_RATE_HZ;

/// Samples of the previous window the model sees before each new one.
const CONTEXT_FRAMES: usize = 64;
/// The recurrent state: two layers of 128 values for a batch of one.
const STATE_VALUES: usize = 2 * 128;

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    input: [f32; CONTEXT_FRAMES + VAD_WINDOW_FRAMES],
}

impl SileroVad {
    /// Loads the model on one thread: a window takes well under a millisecond, and more
    /// threads would only contend with Whisper.
    pub fn new(model_path: &Path) -> Result<Self, VadError> {
        let model = |error: ort::Error| VadError::Model {
            detail: error.to_string(),
        };
        let session = Session::builder()
            .map_err(model)?
            .with_intra_threads(1)
            .map_err(|error| model(error.into()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| model(error.into()))?
            .commit_from_file(model_path)
            .map_err(model)?;
        Ok(Self {
            session,
            state: vec![0.0; STATE_VALUES],
            input: [0.0; CONTEXT_FRAMES + VAD_WINDOW_FRAMES],
        })
    }
}

impl VoiceDetector for SileroVad {
    fn probability(&mut self, window: &[f32; VAD_WINDOW_FRAMES]) -> Result<f32, VadError> {
        let failed = |error: ort::Error| VadError::Inference {
            detail: error.to_string(),
        };
        self.input[CONTEXT_FRAMES..].copy_from_slice(window);
        let input = TensorRef::from_array_view(([1_usize, self.input.len()], &self.input[..]))
            .map_err(failed)?;
        let state =
            TensorRef::from_array_view(([2_usize, 1, 128], &self.state[..])).map_err(failed)?;
        let rate = Tensor::from_array(((), vec![i64::from(MIC_RATE_HZ)])).map_err(failed)?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input, "state" => state, "sr" => rate])
            .map_err(failed)?;
        let (_, probability) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;
        let (_, next_state) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;
        let Some(probability) = probability.first().copied() else {
            return Err(VadError::Inference {
                detail: "the model returned no probability".to_owned(),
            });
        };
        if next_state.len() != STATE_VALUES {
            return Err(VadError::Inference {
                detail: format!("the model returned a state of {} values", next_state.len()),
            });
        }
        self.state.copy_from_slice(next_state);
        self.input.copy_within(VAD_WINDOW_FRAMES.., 0);
        Ok(probability)
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
        self.input.fill(0.0);
    }
}
