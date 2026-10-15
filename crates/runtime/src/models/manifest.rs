//! Every model file the app can download: exact URLs pinned to a commit, sizes and SHA-256
//! digests. The digests were checked by downloading each file with [`super::ModelStore`]; a
//! moved or changed file fails verification instead of loading.

/// A file to download and verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile<'a> {
    /// The name it is stored under in the models folder.
    pub file_name: &'a str,
    pub url: &'a str,
    pub size_bytes: u64,
    /// Lowercase hex.
    pub sha256: &'a str,
    /// The file's license, for the first-run screen.
    pub license: &'a str,
}

/// The files the app knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelId {
    /// Whisper `base.en` (ggml), the default speech-to-text model.
    WhisperBaseEn,
    /// Whisper `small.en` (ggml), more accurate and slower.
    WhisperSmallEn,
    /// Silero VAD v5.1.2 (ONNX), the voice activity detector.
    SileroVad,
    /// Kokoro-82M v1.0, full precision, exported with per-token durations.
    KokoroModel,
    /// Kokoro-82M v1.0, 8-bit quantized, with durations.
    KokoroModelQuantized,
    /// Kokoro's `af_heart` voice: 510 style vectors of 256 floats.
    KokoroVoiceHeart,
    /// misaki's American English "gold" lexicon (word → phonemes).
    MisakiUsGold,
}

impl ModelId {
    pub const ALL: [Self; 7] = [
        Self::WhisperBaseEn,
        Self::WhisperSmallEn,
        Self::SileroVad,
        Self::KokoroModel,
        Self::KokoroModelQuantized,
        Self::KokoroVoiceHeart,
        Self::MisakiUsGold,
    ];

    /// The id as typed on the command line and in settings.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::WhisperBaseEn => "whisper-base.en",
            Self::WhisperSmallEn => "whisper-small.en",
            Self::SileroVad => "silero-vad",
            Self::KokoroModel => "kokoro",
            Self::KokoroModelQuantized => "kokoro-quantized",
            Self::KokoroVoiceHeart => "kokoro-voice-af-heart",
            Self::MisakiUsGold => "misaki-us-gold",
        }
    }

    /// The Whisper model for `stt.model` (`base.en` or `small.en`).
    #[must_use]
    pub fn whisper(name: &str) -> Option<Self> {
        match name {
            "base.en" => Some(Self::WhisperBaseEn),
            "small.en" => Some(Self::WhisperSmallEn),
            _ => None,
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.name() == name)
    }

    #[must_use]
    pub const fn file(self) -> ModelFile<'static> {
        match self {
            Self::WhisperBaseEn => ModelFile {
                file_name: "ggml-base.en.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin",
                size_bytes: 147_964_211,
                sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
                license: "MIT",
            },
            Self::WhisperSmallEn => ModelFile {
                file_name: "ggml-small.en.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.en.bin",
                size_bytes: 487_614_201,
                sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
                license: "MIT",
            },
            Self::SileroVad => ModelFile {
                file_name: "silero_vad_v5.onnx",
                url: "https://raw.githubusercontent.com/snakers4/silero-vad/6478567951ae5c9979ad7b234185b5515f4be7a1/src/silero_vad/data/silero_vad.onnx",
                size_bytes: 2_327_524,
                sha256: "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f",
                license: "MIT",
            },
            Self::KokoroModel => ModelFile {
                file_name: "kokoro-v1.0-timestamped.onnx",
                url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX-timestamped/resolve/dd4401a9add81ac692d20e240d22ec9dda82cc29/onnx/model.onnx",
                size_bytes: 325_532_171,
                sha256: "651ea8291843a92276a4a003581a215cb07d15e47dde6fcfb1b768f9a1682054",
                license: "Apache-2.0",
            },
            Self::KokoroModelQuantized => ModelFile {
                file_name: "kokoro-v1.0-timestamped-quantized.onnx",
                url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX-timestamped/resolve/dd4401a9add81ac692d20e240d22ec9dda82cc29/onnx/model_quantized.onnx",
                size_bytes: 92_361_055,
                sha256: "c0c02b3299fd97c34ea92a98e6d41eaa1a739c8f77bf685aac34bd7b34c1132c",
                license: "Apache-2.0",
            },
            Self::KokoroVoiceHeart => ModelFile {
                file_name: "kokoro-voice-af_heart.bin",
                url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX-timestamped/resolve/dd4401a9add81ac692d20e240d22ec9dda82cc29/voices/af_heart.bin",
                size_bytes: 522_240,
                sha256: "d583ccff3cdca2f7fae535cb998ac07e9fcb90f09737b9a41fa2734ec44a8f0b",
                license: "Apache-2.0",
            },
            Self::MisakiUsGold => ModelFile {
                file_name: "misaki-us_gold.json",
                url: "https://raw.githubusercontent.com/hexgrad/misaki/fba1236595f2d2bf21d414ba6e57d25256afada3/misaki/data/us_gold.json",
                size_bytes: 3_000_469,
                sha256: "dc414872a49a28ae6c141463d502fd945f3b2fde040484fdc47d00cc4612686f",
                license: "Apache-2.0",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ModelId;

    #[test]
    fn every_entry_is_well_formed_and_named_once() {
        for id in ModelId::ALL {
            let file = id.file();
            assert_eq!(ModelId::parse(id.name()), Some(id));
            assert_eq!(file.sha256.len(), 64, "{}", id.name());
            assert!(
                file.sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
            assert!(file.url.starts_with("https://"), "{}", id.name());
            assert!(file.size_bytes > 0);
            assert!(!file.file_name.contains('/'));
        }
        assert_eq!(ModelId::whisper("base.en"), Some(ModelId::WhisperBaseEn));
        assert_eq!(ModelId::whisper("small.en"), Some(ModelId::WhisperSmallEn));
        assert_eq!(ModelId::whisper("large"), None);
        let mut names: Vec<&str> = ModelId::ALL.iter().map(|id| id.file().file_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ModelId::ALL.len());
    }
}
