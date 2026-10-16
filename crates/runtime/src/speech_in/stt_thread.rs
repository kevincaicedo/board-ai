//! The speech-to-text thread: the first partials at fixed times after the voice onset, so a cue
//! word can stop BoardAI early, then a 500 ms grid (a slot that comes while a decode is busy is
//! skipped), an early final once the speaker goes quiet, and the final at the end of the
//! utterance.
//!
//! The early final is what keeps the final fast: the VAD reports the end only after 250 ms of
//! silence, so decoding as soon as the speaker has been quiet for 96 ms usually finishes before
//! `VoiceEnd`. If they speak again it is thrown away and the final is decoded at the end.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use boardai_core::input::Input;
use tokio::sync::mpsc;

use super::SpeechToText;
use super::worker::SpeechInCounters;
use crate::limits::{
    STT_EARLY_PARTIALS_AFTER_ONSET_MS, STT_PARTIAL_INTERVAL_MS, VAD_POLL_INTERVAL_MS,
};

/// How long the thread waits for work when no partial is due before checking it should run.
const IDLE_WAIT: Duration = Duration::from_millis(250);

pub(super) enum SttMessage {
    /// A new utterance, starting with its pre-roll and onset audio; its onset was this long ago
    /// in audio time.
    Begin {
        audio: Vec<f32>,
        since_onset: Duration,
    },
    Audio(Vec<f32>),
    /// The speaker has been quiet for a while inside the utterance.
    Quiet,
    /// The speaker spoke again after `Quiet`.
    Resumed,
    End,
    /// The utterance is dropped unheard (the mic was muted mid-utterance).
    Discard,
    Vocabulary(Vec<String>),
}

pub(super) struct SttWorker {
    stt: Box<dyn SpeechToText>,
    events: mpsc::Sender<Input>,
    counters: Arc<SpeechInCounters>,
    stop: Arc<AtomicBool>,
    utterance: Option<Utterance>,
}

struct Utterance {
    onset: Instant,
    /// The next of [`STT_EARLY_PARTIALS_AFTER_ONSET_MS`] to schedule once `next_partial` is done.
    next_early: usize,
    next_partial: Instant,
    new_audio: bool,
    quiet: bool,
    last_partial_text: String,
    /// The final, decoded while the speaker was quiet; valid until they speak again.
    early_final: Option<String>,
}

impl SttWorker {
    pub(super) fn new(
        stt: Box<dyn SpeechToText>,
        events: mpsc::Sender<Input>,
        counters: Arc<SpeechInCounters>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stt,
            events,
            counters,
            stop,
            utterance: None,
        }
    }

    pub(super) fn run(mut self, messages: &Receiver<SttMessage>) {
        while !self.stop.load(Ordering::Acquire) {
            match messages.recv_timeout(self.wait()) {
                Ok(message) => {
                    if !self.handle(message) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // Work that arrived during a decode comes before the next partial.
            while let Ok(message) = messages.try_recv() {
                if !self.handle(message) {
                    return;
                }
            }
            if !self.partial_if_due() {
                return;
            }
        }
    }

    /// Until the next partial is due; never zero, so a due partial with no new audio does not
    /// spin.
    fn wait(&self) -> Duration {
        let minimum = Duration::from_millis(VAD_POLL_INTERVAL_MS);
        match &self.utterance {
            Some(current) if !current.quiet => current
                .next_partial
                .saturating_duration_since(Instant::now())
                .max(minimum),
            Some(_) | None => IDLE_WAIT,
        }
    }

    /// Handles one message; false when the orchestrator is gone.
    fn handle(&mut self, message: SttMessage) -> bool {
        match message {
            SttMessage::Begin { audio, since_onset } => {
                self.stt.discard();
                self.stt.push(&audio);
                let now = Instant::now();
                let onset = now.checked_sub(since_onset).unwrap_or(now);
                self.utterance = Some(Utterance {
                    onset,
                    next_early: 1,
                    next_partial: onset + early_partial(0),
                    new_audio: true,
                    quiet: false,
                    last_partial_text: String::new(),
                    early_final: None,
                });
            }
            SttMessage::Audio(audio) => {
                if let Some(current) = self.utterance.as_mut() {
                    self.stt.push(&audio);
                    current.new_audio = true;
                }
            }
            SttMessage::Quiet => self.decode_early_final(),
            SttMessage::Resumed => {
                if let Some(current) = self.utterance.as_mut() {
                    current.quiet = false;
                    current.early_final = None;
                }
            }
            SttMessage::End => return self.finish(),
            SttMessage::Discard => {
                self.utterance = None;
                self.stt.discard();
            }
            SttMessage::Vocabulary(terms) => self.stt.set_vocabulary(&terms),
        }
        true
    }

    fn decode_early_final(&mut self) {
        let Some(current) = self.utterance.as_mut() else {
            return;
        };
        current.quiet = true;
        match self.stt.partial() {
            Ok(text) => current.early_final = Some(text.unwrap_or_default()),
            Err(error) => {
                self.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(%error, "early final transcript failed");
            }
        }
    }

    /// Sends the final: the early one if it is still valid, else a decode now.
    fn finish(&mut self) -> bool {
        let Some(current) = self.utterance.take() else {
            return true;
        };
        let text = if let Some(text) = current.early_final {
            self.stt.discard();
            text
        } else {
            self.stt.finish().unwrap_or_else(|error| {
                self.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(%error, "final transcript failed");
                String::new()
            })
        };
        let input = Input::Transcript {
            text,
            is_final: true,
        };
        self.events.blocking_send(input).is_ok()
    }

    /// Decodes and sends a partial when its slot has come; a full queue drops it (a newer one or
    /// the final follows). False when the orchestrator is gone.
    fn partial_if_due(&mut self) -> bool {
        let now = Instant::now();
        let Some(current) = self.utterance.as_mut() else {
            return true;
        };
        if current.quiet || !current.new_audio || now < current.next_partial {
            return true;
        }
        current.schedule_after(now);
        current.new_audio = false;
        let text = match self.stt.partial() {
            Ok(Some(text)) if text != current.last_partial_text => text,
            Ok(Some(_) | None) => return true,
            Err(error) => {
                self.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(%error, "partial transcript failed");
                return true;
            }
        };
        current.last_partial_text.clone_from(&text);
        let partial = Input::Transcript {
            text,
            is_final: false,
        };
        match self.events.try_send(partial) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.counters
                    .partials_dropped
                    .fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

/// The early partial at `index`, after the onset.
fn early_partial(index: usize) -> Duration {
    Duration::from_millis(
        STT_EARLY_PARTIALS_AFTER_ONSET_MS
            .get(index)
            .copied()
            .unwrap_or(0),
    )
}

impl Utterance {
    /// The slot after the one being decoded at `now`: the next early partial (at once if its
    /// time has passed), else the next grid slot after `now`, skipping those a decode overran.
    fn schedule_after(&mut self, now: Instant) {
        if self.next_early < STT_EARLY_PARTIALS_AFTER_ONSET_MS.len() {
            self.next_partial = self.onset + early_partial(self.next_early);
            self.next_early += 1;
            return;
        }
        let interval = Duration::from_millis(STT_PARTIAL_INTERVAL_MS);
        while self.next_partial <= now {
            self.next_partial += interval;
        }
    }
}
