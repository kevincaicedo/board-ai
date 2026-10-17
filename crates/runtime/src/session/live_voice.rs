//! The real voice as the session's [`SpeechOut`] port (LLD §3.1): effects become
//! [`SpeechOutCommand`]s for the speech-out actor, and the actor's reports (`ClauseReady`,
//! `SynthesisFailed`, `PlaybackDrained`, `VoiceStopped`, and the playback watch) are forwarded
//! into the session's [`SpeechOutLink`] by one task.
//!
//! Ducking skips the actor's queue: it sets the ring's gain atomic directly, so a barge-in's
//! duck can never be lost to a full queue or wait behind a synthesis report. Every other command
//! goes through the queue, in order.

use std::sync::Arc;

use boardai_core::effect::Gain;
use boardai_core::ids::ClauseId;
use boardai_core::input::Input;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::inbox::{SessionClosed, SpeechOutLink};
use super::ports::{SpeechOut, SpeechOutError};
use crate::audio::{VoiceGain, VoiceOutput};
use crate::speech_out::{
    SpeechOut as SpeechOutActor, SpeechOutChannels, SpeechOutCommand, SpeechOutConfig, TextToSpeech,
};

/// The port the orchestrator holds. Dropping it stops the actor (its queue closes) and the
/// forwarding task.
pub struct LiveVoice {
    commands: mpsc::Sender<SpeechOutCommand>,
    gain: VoiceGain,
    forwarder: JoinHandle<()>,
    /// Kept so the actor lives as long as the port; it stops when every sender is gone.
    _actor: SpeechOutActor,
}

impl std::fmt::Debug for LiveVoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LiveVoice").finish_non_exhaustive()
    }
}

impl LiveVoice {
    /// Starts speech out on `output` with `engine`, answering through `link`. Must run inside
    /// a tokio runtime.
    #[must_use]
    pub fn spawn(
        link: SpeechOutLink,
        engine: Arc<dyn TextToSpeech>,
        config: SpeechOutConfig,
        output: VoiceOutput,
    ) -> Self {
        let gain = output.gain_control();
        let (actor, channels) = SpeechOutActor::spawn(engine, config, output);
        let forwarder = tokio::spawn(forward(link, channels));
        Self {
            commands: actor.sender(),
            gain,
            forwarder,
            _actor: actor,
        }
    }

    fn command(&self, command: SpeechOutCommand) -> Result<(), SpeechOutError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SpeechOutError::Full,
                mpsc::error::TrySendError::Closed(_) => SpeechOutError::Closed,
            })
    }
}

impl Drop for LiveVoice {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

impl SpeechOut for LiveVoice {
    fn synthesize(&mut self, clause: ClauseId, text: String) -> Result<(), SpeechOutError> {
        self.command(SpeechOutCommand::Synthesize { clause, text })
    }

    fn duck(&mut self, gain: Gain) {
        self.gain.set(gain);
    }

    fn stop(&mut self) -> Result<(), SpeechOutError> {
        self.command(SpeechOutCommand::StopVoice)
    }

    fn reset(&mut self) {
        self.gain.set(Gain::FULL);
        if let Err(error) = self.command(SpeechOutCommand::Reset) {
            tracing::warn!(%error, "the voice did not take its reset");
        }
    }
}

/// Moves the actor's reports into the session until either side closes. Reports come before
/// positions: a clause's `ClauseReady` is always queued before its first position exists.
async fn forward(link: SpeechOutLink, mut channels: SpeechOutChannels) {
    loop {
        tokio::select! {
            biased;
            report = channels.events.recv() => {
                let Some(report) = report else { return };
                if deliver(&link, report).await.is_err() {
                    return;
                }
            }
            changed = channels.playback.changed() => {
                if changed.is_err() {
                    return;
                }
                let position = *channels.playback.borrow_and_update();
                if let Some(position) = position {
                    link.playback_at(position);
                }
            }
        }
    }
}

async fn deliver(link: &SpeechOutLink, report: Input) -> Result<(), SessionClosed> {
    match report {
        Input::ClauseReady { clause, timings } => link.clause_ready(clause, timings).await,
        Input::SynthesisFailed { clause } => link.synthesis_failed(clause).await,
        Input::PlaybackDrained => link.drained().await,
        Input::VoiceStopped { last } => link.voice_stopped(last).await,
        // Speech out reports only the four above; anything else is a broken invariant there.
        other @ (Input::VoiceOnset
        | Input::VoiceStart
        | Input::VoiceEnd { .. }
        | Input::Transcript { .. }
        | Input::TalkerDelta { .. }
        | Input::TalkerToolCall { .. }
        | Input::TalkerDone { .. }
        | Input::PlaybackAt { .. }
        | Input::ToolResult { .. }
        | Input::Harness { .. }
        | Input::Command(_)
        | Input::Tick
        | Input::MemoryFailed { .. }
        | Input::Source { .. }) => {
            debug_assert!(false, "speech out reported {other:?}");
            tracing::error!(
                ?other,
                "speech out reported an input it does not own; dropped"
            );
            Ok(())
        }
    }
}
