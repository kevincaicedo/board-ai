//! Real audio for the session (LLD §3.1, §3.4): the voice through VoiceProcessingIO, and speech
//! in on the same unit's microphone once macOS allows it.
//!
//! Each part falls back on its own and says why with a `Problem` caption: without an audio
//! device the virtual voice keeps captions and strokes in time with no sound; without microphone
//! access or speech models BoardAI still speaks but cannot hear. The app stays usable either
//! way, replay included. The first-run screen asks for the microphone before the session
//! starts; an undetermined permission (a dev script skipped setup) is asked for here, once,
//! through the same `crate::permission`.

use std::path::PathBuf;

use boardai_core::setup::MicPermission;
use boardai_runtime::audio::{
    AudioConfig, AudioError, AudioIo, InputChoice, MicReader, VoiceOutput,
};
use boardai_runtime::models::ModelStore;
use boardai_runtime::session::{
    LiveListener, LiveVoice, ProblemLink, SessionChannels, SpeechOut, VirtualVoice,
    VirtualVoiceConfig,
};
use boardai_runtime::settings::Settings;
use boardai_runtime::speech_in::{SpeechIn, load_speech_in, repo_terms};
use boardai_runtime::speech_out::{ChosenVoice, TtsError, choose_voice};

use crate::permission;
use crate::secrets::KeychainSecrets;

/// What keeps real audio running; dropped after the session closes.
pub struct RunningAudio {
    /// Dropping it stops the unit.
    _unit: AudioIo,
    listener: Option<LiveListener>,
}

impl RunningAudio {
    /// Stops speech in's threads, then the unit.
    pub async fn stop(self) {
        if let Some(listener) = self.listener {
            listener.shutdown().await;
        }
    }
}

/// Starts the voice and, when allowed, the microphone. Returns the session's speech out and
/// what keeps it running (`None` with the virtual voice).
pub async fn start(
    settings: &Settings,
    channels: &SessionChannels,
    models_root: PathBuf,
) -> (Box<dyn SpeechOut>, Option<RunningAudio>) {
    let problems = channels.problems();
    let models = match ModelStore::new(models_root) {
        Ok(models) => models,
        Err(error) => {
            tracing::error!(%error, "the model folder is unusable");
            return virtual_voice(channels, &problems, "the model folder is unusable");
        }
    };
    let Some(voice) = choose(settings, models.clone(), &problems).await else {
        return virtual_voice(channels, &problems, "no voice could start");
    };
    let access = permission::ask_once().await;
    let Some((unit, mic, output)) = open_unit(settings.audio.input.clone(), &problems).await else {
        return virtual_voice(channels, &problems, "the audio device would not start");
    };
    let speech = LiveVoice::spawn(channels.speech_out(), voice.engine, voice.config, output);
    let listener = listen(settings, channels, mic, models, access, &problems).await;
    tracing::info!(
        voice = ?settings.tts.kind,
        hearing = listener.is_some(),
        output_latency_ms = unit.output_latency_ms(),
        "real audio started"
    );
    let running = RunningAudio {
        _unit: unit,
        listener,
    };
    (Box::new(speech), Some(running))
}

fn virtual_voice(
    channels: &SessionChannels,
    problems: &ProblemLink,
    why: &str,
) -> (Box<dyn SpeechOut>, Option<RunningAudio>) {
    problems.report(
        "I can't play sound right now, so my answers show as captions only.",
        why,
    );
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::default());
    (Box::new(voice), None)
}

/// The configured engine, or the system voice in its place (said out loud unless it is only
/// waiting for its download, as on a first run).
async fn choose(
    settings: &Settings,
    models: ModelStore,
    problems: &ProblemLink,
) -> Option<ChosenVoice> {
    let (tts, voice) = (settings.tts.clone(), settings.voice.id.clone());
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        choose_voice(&tts, &voice, &KeychainSecrets, &models)
    })
    .await;
    match chosen {
        Ok(Ok(chosen)) => {
            match &chosen.replaced {
                Some(TtsError::Unavailable { detail }) => {
                    tracing::info!(%detail, "speaking with the system voice for now");
                }
                Some(error) => problems.report(
                    "My voice isn't available, so I'm using the Mac's built-in one.",
                    error.to_string(),
                ),
                None => {}
            }
            Some(chosen)
        }
        Ok(Err(error)) => {
            tracing::error!(%error, "not even the system voice could start");
            None
        }
        Err(error) => {
            tracing::error!(%error, "choosing the voice failed");
            None
        }
    }
}

/// Opens VoiceProcessingIO on the configured microphone, or on the system default when no
/// device has that name.
async fn open_unit(
    input: InputChoice,
    problems: &ProblemLink,
) -> Option<(AudioIo, MicReader, VoiceOutput)> {
    let open =
        |input| tauri::async_runtime::spawn_blocking(move || AudioIo::start(AudioConfig { input }));
    let opened = match open(input).await {
        Ok(Err(error @ AudioError::NoSuchInput { .. })) => {
            problems.report(
                "I can't find that microphone, so I'm listening on the system default.",
                error.to_string(),
            );
            open(InputChoice::SystemDefault).await
        }
        other => other,
    };
    match opened {
        Ok(Ok(started)) => Some(started),
        Ok(Err(error)) => {
            tracing::error!(%error, "the audio unit would not start");
            None
        }
        Err(error) => {
            tracing::error!(%error, "opening the audio unit failed");
            None
        }
    }
}

/// Speech in on the unit's microphone, when macOS allows it and the models are downloaded.
async fn listen(
    settings: &Settings,
    channels: &SessionChannels,
    mic: MicReader,
    models: ModelStore,
    access: MicPermission,
    problems: &ProblemLink,
) -> Option<LiveListener> {
    if access != MicPermission::Granted {
        problems.report(
            "I can't hear you: microphone access is off for BoardAI.",
            "Turn it on in System Settings → Privacy & Security → Microphone, then reopen \
             BoardAI. Replays still work.",
        );
        return None;
    }
    let (stt_model, roots) = (settings.stt.model.clone(), settings.code_roots.clone());
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        let engines = load_speech_in(&models, &stt_model);
        (engines, repo_terms(&roots))
    })
    .await;
    let ((vad, stt), terms) = match loaded {
        Ok((Ok(engines), terms)) => (engines, terms),
        Ok((Err(error), _)) => {
            problems.report(
                "I can't hear you yet: my speech models aren't downloaded.",
                error.to_string(),
            );
            return None;
        }
        Err(error) => {
            tracing::error!(%error, "loading speech in failed");
            return None;
        }
    };
    match SpeechIn::spawn(mic, Box::new(vad), Box::new(stt)) {
        Ok((speech_in, inputs)) => Some(LiveListener::spawn(
            speech_in,
            inputs,
            channels.speech_in(),
            channels.listening(),
            terms,
        )),
        Err(error) => {
            problems.report("I can't hear you right now.", error.to_string());
            None
        }
    }
}
