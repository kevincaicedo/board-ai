//! The configured text-to-speech engine, built from settings (LLD §3.8). A local engine whose
//! files are not downloaded yet fails with `Unavailable`; the caller then speaks with the system
//! voice, as LLD §3.4 asks during first-run downloads.

use std::sync::Arc;

use super::{
    ElevenLabsOptions, ElevenLabsVoice, KokoroFiles, KokoroVoice, SystemVoice, TextToSpeech,
    TtsError,
};
use crate::models::{ModelId, ModelStatus, ModelStore};
use crate::settings::{SecretStore, TtsKind, TtsSettings};

/// `tts.model` value that picks the 8-bit Kokoro export (slower on Apple CPUs; see spike S2).
pub const KOKORO_QUANTIZED_MODEL: &str = "kokoro-quantized";

/// Builds the engine settings ask for. Reading the ElevenLabs key may block on the Keychain, so
/// call this from a blocking context.
pub fn engine_for(
    settings: &TtsSettings,
    secrets: &dyn SecretStore,
    models: &ModelStore,
) -> Result<Arc<dyn TextToSpeech>, TtsError> {
    match settings.kind {
        TtsKind::System => Ok(Arc::new(SystemVoice::new()?)),
        TtsKind::Kokoro => {
            let model = match settings.model.as_deref() {
                Some(KOKORO_QUANTIZED_MODEL) => ModelId::KokoroModelQuantized,
                Some(_) | None => ModelId::KokoroModel,
            };
            let files = KokoroFiles {
                model: ready(models, model)?,
                voice: ready(models, ModelId::KokoroVoiceHeart)?,
                lexicon: ready(models, ModelId::MisakiUsGold)?,
            };
            Ok(Arc::new(KokoroVoice::new(files)?))
        }
        TtsKind::ElevenLabs => {
            let name = settings.secret.as_ref().ok_or_else(|| TtsError::Auth {
                detail: "no ElevenLabs key is configured".to_owned(),
            })?;
            let api_key = secrets.get(name).map_err(|error| TtsError::Auth {
                detail: error.to_string(),
            })?;
            let options = ElevenLabsOptions {
                base_url: settings.base_url.clone(),
                api_key,
                model: settings.model.clone(),
            };
            Ok(Arc::new(ElevenLabsVoice::new(options)?))
        }
    }
}

fn ready(models: &ModelStore, id: ModelId) -> Result<std::path::PathBuf, TtsError> {
    match models.status(&id.file()) {
        ModelStatus::Ready { path } => Ok(path),
        ModelStatus::Missing { .. } | ModelStatus::Partial { .. } => Err(TtsError::Unavailable {
            detail: format!("{} is not downloaded yet", id.name()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::engine_for;
    use crate::models::ModelStore;
    use crate::settings::{MemorySecretStore, Secret, SecretName, TtsKind, TtsSettings};
    use crate::speech_out::TtsError;

    fn settings(kind: TtsKind) -> TtsSettings {
        TtsSettings {
            kind,
            model: None,
            base_url: None,
            secret: SecretName::parse("elevenlabs"),
        }
    }

    fn empty_store() -> ModelStore {
        let folder = std::env::temp_dir().join(format!("boardai-engine-{}", std::process::id()));
        ModelStore::new(folder).expect("store")
    }

    #[test]
    fn a_local_engine_without_its_files_is_unavailable() {
        let outcome = engine_for(
            &settings(TtsKind::Kokoro),
            &MemorySecretStore::new(),
            &empty_store(),
        );
        let Err(TtsError::Unavailable { detail }) = outcome else {
            panic!("unavailable")
        };
        assert!(detail.contains("not downloaded"), "{detail}");
    }

    #[test]
    fn elevenlabs_needs_its_key() {
        let missing = engine_for(
            &settings(TtsKind::ElevenLabs),
            &MemorySecretStore::new(),
            &empty_store(),
        );
        assert!(matches!(missing, Err(TtsError::Auth { .. })));
        let mut secrets = MemorySecretStore::new();
        if let Some(name) = SecretName::parse("elevenlabs") {
            secrets.insert(name, Secret::new("xi-key".to_owned()));
        }
        assert!(engine_for(&settings(TtsKind::ElevenLabs), &secrets, &empty_store()).is_ok());
    }
}
