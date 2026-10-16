//! Speech in's engines from the model folder (LLD §3.4): Silero for voice activity and the
//! configured Whisper model. Missing files are an error naming them, so the app can say what
//! to download instead of listening to nothing.

use std::fmt;

use super::{SileroVad, WhisperStt};
use crate::models::{ModelId, ModelStatus, ModelStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechInSetupError {
    /// `stt.model` names no Whisper model we know.
    UnknownModel { name: String },
    /// These model files are not downloaded yet (`just models <names>`).
    Missing { names: Vec<&'static str> },
    /// A file is there but would not load.
    Load { detail: String },
}

impl fmt::Display for SpeechInSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModel { name } => write!(formatter, "no Whisper model is called {name}"),
            Self::Missing { names } => {
                write!(formatter, "not downloaded yet: {}", names.join(", "))
            }
            Self::Load { detail } => write!(formatter, "a speech model would not load: {detail}"),
        }
    }
}

impl std::error::Error for SpeechInSetupError {}

/// Loads Silero and the Whisper model `stt_model` (`base.en`, `small.en`). Blocking: Whisper
/// takes a few hundred milliseconds to load.
pub fn load_speech_in(
    models: &ModelStore,
    stt_model: &str,
) -> Result<(SileroVad, WhisperStt), SpeechInSetupError> {
    let whisper = ModelId::whisper(stt_model).ok_or_else(|| SpeechInSetupError::UnknownModel {
        name: stt_model.to_owned(),
    })?;
    let path = |id: ModelId| match models.status(&id.file()) {
        ModelStatus::Ready { path } => Some(path),
        ModelStatus::Missing { .. } | ModelStatus::Partial { .. } => None,
    };
    let (vad_path, whisper_path) = (path(ModelId::SileroVad), path(whisper));
    let (Some(vad_path), Some(whisper_path)) = (&vad_path, &whisper_path) else {
        let mut names = Vec::new();
        if vad_path.is_none() {
            names.push(ModelId::SileroVad.name());
        }
        if whisper_path.is_none() {
            names.push(whisper.name());
        }
        return Err(SpeechInSetupError::Missing { names });
    };
    let load = |detail: String| SpeechInSetupError::Load { detail };
    let vad = SileroVad::new(vad_path).map_err(|error| load(error.to_string()))?;
    let stt = WhisperStt::new(whisper_path).map_err(|error| load(error.to_string()))?;
    Ok((vad, stt))
}

#[cfg(test)]
mod tests {
    use super::{SpeechInSetupError, load_speech_in};
    use crate::models::ModelStore;

    #[test]
    fn missing_models_are_named_and_unknown_ones_refused() {
        let folder = std::env::temp_dir().join(format!("boardai-setup-{}", std::process::id()));
        let models = ModelStore::new(folder).expect("store");
        let Err(SpeechInSetupError::Missing { names }) = load_speech_in(&models, "base.en") else {
            panic!("missing");
        };
        assert_eq!(names, vec!["silero-vad", "whisper-base.en"]);
        assert!(matches!(
            load_speech_in(&models, "large"),
            Err(SpeechInSetupError::UnknownModel { .. })
        ));
    }
}
