//! ElevenLabs text-to-speech (LLD §3.4), two ways:
//!
//! - [`ElevenLabsRealtime`], the default: the multi-context WebSocket
//!   (`/v1/text-to-speech/{voice}/multi-stream-input`), one connection kept warm across replies
//!   and one context per clause. It skips a request's setup per clause and lets a barge-in close a
//!   context without closing the connection.
//! - [`ElevenLabsVoice`]: `POST …/stream/with-timestamps` per clause. The realtime engine falls
//!   back to it for a clause the WebSocket cannot carry (no connection, a dropped socket).
//!
//! Both ask for 16-bit PCM at 24 kHz (the voice ring's rate) and the original text's character
//! alignment, which becomes exact word timings. The model defaults to Flash v2.5
//! (`eleven_flash_v2_5`, ~75 ms of model inference by ElevenLabs' docs). The key comes from the
//! [`SecretStore`](crate::settings::SecretStore) and is only ever placed in a sensitive header.

mod alignment;
mod http;
mod realtime;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

pub use http::ElevenLabsVoice;
pub use realtime::ElevenLabsRealtime;

use super::base64;
use super::{TtsError, VoiceId};
use crate::limits::ELEVENLABS_VOICE_ID_MAX_BYTES;
use crate::llm::{ProviderError, bounded_detail};
use crate::settings::Secret;

pub const ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io";

/// The voice used when settings say `default`: "Rachel", one of ElevenLabs' premade voices.
/// Which voice ships as the default is the owner's call (D-011); this keeps the engine usable.
pub const ELEVENLABS_DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";

/// The model when settings name none: Flash v2.5, ElevenLabs' lowest-latency model ("~75ms",
/// excluding network) at half the per-character price of Multilingual v2.
pub const ELEVENLABS_DEFAULT_MODEL: &str = "eleven_flash_v2_5";

/// 16-bit little-endian mono PCM at 24 kHz, the voice ring's rate, so nothing is converted.
const OUTPUT_FORMAT: &str = "pcm_24000";

#[derive(Clone, Debug)]
pub struct ElevenLabsOptions {
    pub base_url: Option<String>,
    pub api_key: Secret,
    /// `None` means [`ELEVENLABS_DEFAULT_MODEL`].
    pub model: Option<String>,
}

impl ElevenLabsOptions {
    fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or(ELEVENLABS_BASE_URL)
            .trim_end_matches('/')
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or(ELEVENLABS_DEFAULT_MODEL)
    }

    /// The key as a sensitive header, so no debug print of a request shows it.
    fn key_headers(&self) -> Result<HeaderMap, TtsError> {
        let mut key = HeaderValue::from_str(self.api_key.expose()).map_err(|_| TtsError::Auth {
            detail: "the ElevenLabs key has characters that cannot be sent".to_owned(),
        })?;
        key.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("xi-api-key"), key);
        Ok(headers)
    }
}

/// The voice id to put in a URL path: `default` is [`ELEVENLABS_DEFAULT_VOICE_ID`]; anything
/// else must look like an ElevenLabs id, so a setting can never reach another path.
fn voice_path(voice: &VoiceId) -> Result<&str, TtsError> {
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
    if valid {
        Ok(voice_id)
    } else {
        Err(TtsError::Provider {
            status: None,
            detail: "the ElevenLabs voice id is not valid".to_owned(),
        })
    }
}

/// Base64 16-bit little-endian PCM as samples in [-1, 1).
fn pcm_from_base64(audio: &str) -> Result<Vec<f32>, TtsError> {
    let bytes = base64::decode(audio).ok_or_else(|| bad_output("audio is not valid base64"))?;
    if bytes.len() % 2 != 0 {
        return Err(bad_output("audio has half a sample"));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
        .collect())
}

fn bad_output(detail: &str) -> TtsError {
    TtsError::BadOutput {
        detail: detail.to_owned(),
    }
}

fn network(error: &reqwest::Error) -> TtsError {
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
    use super::{ELEVENLABS_DEFAULT_VOICE_ID, pcm_from_base64, voice_path};
    use crate::speech_out::{TtsError, VoiceId};

    #[test]
    fn samples_are_signed_16_bit_little_endian() {
        assert_eq!(
            pcm_from_base64("AEAAwP9/AIA="),
            Ok(vec![0.5, -0.5, 32_767.0 / 32_768.0, -1.0])
        );
        assert!(matches!(
            pcm_from_base64("AEAA"),
            Err(TtsError::BadOutput { .. })
        ));
        assert!(matches!(
            pcm_from_base64("@@@@"),
            Err(TtsError::BadOutput { .. })
        ));
    }

    #[test]
    fn voice_ids_are_checked_before_they_reach_a_path() {
        assert_eq!(
            voice_path(&VoiceId::default()),
            Ok(ELEVENLABS_DEFAULT_VOICE_ID)
        );
        assert_eq!(voice_path(&VoiceId::new("abc_DEF-1")), Ok("abc_DEF-1"));
        for bad in ["../admin", "", "a b", &"x".repeat(65)] {
            assert!(voice_path(&VoiceId::new(bad)).is_err(), "{bad}");
        }
    }
}
