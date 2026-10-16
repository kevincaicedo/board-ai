//! Microphone permission: the app's one entry point over the runtime's `AVAudioApplication`
//! calls (the FFI lives in `runtime::audio::coreaudio`, the only place `unsafe` is allowed).
//! The first-run screen asks through it, and so does audio startup when setup was skipped.

use std::time::Duration;

use boardai_core::setup::MicPermission;
use boardai_runtime::audio::{MicrophoneAccess, microphone_access, request_microphone_access};

/// How long a request waits for the user to answer macOS's prompt. Past it the request
/// returns whatever macOS reports then (still undetermined, most likely), so the first-run
/// card never waits forever on a prompt that didn't show.
pub const MICROPHONE_ANSWER_WAIT_MS: u64 = 120_000;

/// The current permission. Never prompts.
pub fn status() -> MicPermission {
    from_access(microphone_access())
}

/// Asks once (macOS shows its prompt only while undetermined) and returns the answer.
pub async fn request() -> MicPermission {
    let wait = Duration::from_millis(MICROPHONE_ANSWER_WAIT_MS);
    match tokio::time::timeout(wait, request_microphone_access()).await {
        Ok(answer) => from_access(answer),
        Err(_) => {
            tracing::warn!("no answer to the microphone prompt in time");
            status()
        }
    }
}

/// The permission, asking first while it is undetermined (audio startup after a dev script
/// skipped setup; after setup it is already answered).
pub async fn ask_once() -> MicPermission {
    match status() {
        MicPermission::Undetermined => request().await,
        known @ (MicPermission::Granted | MicPermission::Denied) => known,
    }
}

const fn from_access(access: MicrophoneAccess) -> MicPermission {
    match access {
        MicrophoneAccess::Granted => MicPermission::Granted,
        MicrophoneAccess::Denied => MicPermission::Denied,
        MicrophoneAccess::Undetermined => MicPermission::Undetermined,
    }
}
