//! Real speech in for a session (LLD §3.1): [`SpeechIn`]'s inputs forwarded into the session's
//! [`SpeechInLink`], and the session's [`Listening`] carried back to it, so the mic is muted
//! whenever the core would not hear it and Whisper expects the words on the board.
//!
//! One task does both. Inference stays on speech in's own two threads; this task only moves
//! messages, so the orchestrator never waits on a decode.

use boardai_core::input::Input;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::inbox::{SessionClosed, SpeechInLink};
use super::listening::Listening;
use crate::cancel::CancelToken;
use crate::speech_in::SpeechIn;

/// Owns speech in while the session runs. [`LiveListener::shutdown`] stops its threads.
pub struct LiveListener {
    stop: CancelToken,
    task: JoinHandle<SpeechIn>,
}

impl std::fmt::Debug for LiveListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveListener")
            .finish_non_exhaustive()
    }
}

impl LiveListener {
    /// Starts forwarding. `repo_terms` (the opened repo's symbols, best first) follow the
    /// recent board terms in the vocabulary; Whisper keeps as many as its prompt holds.
    #[must_use]
    pub fn spawn(
        speech_in: SpeechIn,
        inputs: mpsc::Receiver<Input>,
        link: SpeechInLink,
        listening: watch::Receiver<Listening>,
        repo_terms: Vec<String>,
    ) -> Self {
        let stop = CancelToken::new();
        let follower = Follower {
            speech_in,
            repo_terms,
            muted: None,
            recent_terms: None,
        };
        let task = tokio::spawn(follower.run(inputs, link, listening, stop.clone()));
        Self { stop, task }
    }

    /// Stops forwarding, then stops speech in's threads (waiting for at most one decode).
    pub async fn shutdown(self) {
        self.stop.cancel();
        match self.task.await {
            Ok(speech_in) => {
                if tokio::task::spawn_blocking(move || speech_in.shutdown())
                    .await
                    .is_err()
                {
                    tracing::warn!("speech in did not shut down cleanly");
                }
            }
            Err(error) => tracing::error!(%error, "the speech-in forwarder ended abnormally"),
        }
    }
}

struct Follower {
    speech_in: SpeechIn,
    repo_terms: Vec<String>,
    /// What was last applied, so speech in hears of changes only.
    muted: Option<bool>,
    recent_terms: Option<Vec<String>>,
}

impl Follower {
    async fn run(
        mut self,
        mut inputs: mpsc::Receiver<Input>,
        link: SpeechInLink,
        mut listening: watch::Receiver<Listening>,
        stop: CancelToken,
    ) -> SpeechIn {
        let first = listening.borrow_and_update().clone();
        self.apply(&first);
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break,
                input = inputs.recv() => {
                    let Some(input) = input else { break };
                    if forward(&link, input).await.is_err() {
                        break;
                    }
                }
                changed = listening.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let latest = listening.borrow_and_update().clone();
                    self.apply(&latest);
                }
            }
        }
        // Nothing hears the mic once the session is gone.
        self.speech_in.set_muted(true);
        self.speech_in
    }

    fn apply(&mut self, listening: &Listening) {
        let muted = !listening.open;
        if self.muted != Some(muted) {
            self.speech_in.set_muted(muted);
            self.muted = Some(muted);
        }
        let recent: Vec<String> = listening.recent_terms.iter().cloned().collect();
        if self.recent_terms.as_ref() != Some(&recent) {
            let mut terms = recent.clone();
            terms.extend(self.repo_terms.iter().cloned());
            self.speech_in.set_vocabulary(terms);
            self.recent_terms = Some(recent);
        }
    }
}

async fn forward(link: &SpeechInLink, input: Input) -> Result<(), SessionClosed> {
    match input {
        Input::VoiceOnset => link.voice_onset().await,
        Input::VoiceStart => link.voice_start().await,
        Input::VoiceEnd {
            voiced_ms,
            silence_ms,
        } => link.voice_end(voiced_ms, silence_ms).await,
        Input::Transcript { text, is_final } => link.transcript(text, is_final).await,
        Input::EarlyFinal { text } => link.early_final(text).await,
        Input::VoiceResumed => link.voice_resumed().await,
        // Speech in produces only the six above; anything else is a broken invariant there.
        other @ (Input::TalkerDelta { .. }
        | Input::TalkerToolCall { .. }
        | Input::TalkerDone { .. }
        | Input::ClauseReady { .. }
        | Input::SynthesisFailed { .. }
        | Input::PlaybackAt { .. }
        | Input::PlaybackDrained
        | Input::VoiceStopped { .. }
        | Input::ToolResult { .. }
        | Input::Harness { .. }
        | Input::Command(_)
        | Input::Tick
        | Input::MemoryFailed { .. }
        | Input::Source { .. }) => {
            debug_assert!(false, "speech in produced {other:?}");
            tracing::error!(
                ?other,
                "speech in produced an input it does not own; dropped"
            );
            Ok(())
        }
    }
}
