//! Microphone permission through `AVAudioApplication` (macOS 14+). Reading the state never
//! prompts; only an explicit request does, and only while the user has not answered yet.
//!
//! Without permission macOS hands the voice-processing unit silence rather than an error, so
//! the app asks here first and says out loud when it cannot hear (LLD §3.4).

use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};

use crate::audio::MicrophoneAccess;

pub(crate) fn microphone_access() -> MicrophoneAccess {
    // SAFETY: `sharedInstance` returns the process's application object; reading its
    // `recordPermission` has no preconditions and never shows a prompt.
    let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
    match permission {
        AVAudioApplicationRecordPermission::Granted => MicrophoneAccess::Granted,
        AVAudioApplicationRecordPermission::Undetermined => MicrophoneAccess::Undetermined,
        AVAudioApplicationRecordPermission::Denied => MicrophoneAccess::Denied,
        other => {
            // A value newer than this code: treated as no access, so nothing is recorded.
            tracing::warn!(raw = other.0, "unknown microphone permission state");
            MicrophoneAccess::Denied
        }
    }
}

/// Asks for permission (macOS shows its prompt if the user has not answered yet) and calls
/// `answered` once with the outcome, on whatever thread macOS chooses.
pub(crate) fn request_microphone_access(answered: impl FnOnce(bool) + Send + 'static) {
    let answered = Mutex::new(Some(answered));
    let block = RcBlock::new(move |granted: Bool| {
        // The handler is called once; the `Option` makes a second call harmless.
        let callback = answered.lock().ok().and_then(|mut slot| slot.take());
        if let Some(callback) = callback {
            callback(granted.as_bool());
        }
    });
    // SAFETY: `block` is a live block; AVFAudio copies it before returning, and its closure
    // captures only a `Mutex` around a `Send` callback, so calling it on another thread is sound.
    unsafe {
        AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block);
    }
}
