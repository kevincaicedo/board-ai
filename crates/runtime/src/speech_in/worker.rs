//! The speech-in threads and the handle that owns them (see the module comment in `mod.rs`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use boardai_core::input::Input;
use tokio::sync::mpsc;

use super::stt_thread::{SttMessage, SttWorker};
use super::vad::{EARLY_FINAL_AFTER_QUIET_MS, MIN_SPEECH_MS, VadEvent, VadMachine};
use super::{SpeechToText, VAD_WINDOW_FRAMES, VoiceDetector};
use crate::audio::{MIC_RATE_HZ, MicReader};
use crate::limits::{
    SPEECH_IN_EVENT_QUEUE, SPEECH_PREROLL_MS, STT_QUEUE_MESSAGES, VAD_POLL_INTERVAL_MS,
};

/// Audio kept from before `Start` is reported: the minimum speech, one window of slack, and the
/// pre-roll.
const RECENT_FRAMES: usize =
    ((MIN_SPEECH_MS + SPEECH_PREROLL_MS) * MIC_RATE_HZ / 1_000) as usize + 2 * VAD_WINDOW_FRAMES;
const PREROLL_FRAMES: u64 = (SPEECH_PREROLL_MS * MIC_RATE_HZ / 1_000) as u64;

/// What was dropped at a limit, counted (LLD §5).
#[derive(Debug, Default)]
pub struct SpeechInCounters {
    /// Partial transcripts dropped because the orchestrator's queue was full.
    pub partials_dropped: AtomicU64,
    /// Windows dropped because the speech-to-text queue was full.
    pub windows_dropped: AtomicU64,
    /// Windows the detector failed on; they count as silence.
    pub detector_errors: AtomicU64,
    /// Decodes that failed; a failed final is reported as an empty transcript.
    pub decode_errors: AtomicU64,
}

/// Owns the two threads. `shutdown` stops and joins them; dropping the handle only asks them
/// to stop, so a drop never blocks on a decode.
pub struct SpeechIn {
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    control: SyncSender<SttMessage>,
    counters: Arc<SpeechInCounters>,
    threads: Vec<JoinHandle<()>>,
}

impl SpeechIn {
    /// Starts both threads. Inputs arrive on the returned receiver (bounded at
    /// [`SPEECH_IN_EVENT_QUEUE`]; see that limit for what happens when it is full).
    pub fn spawn(
        mic: MicReader,
        detector: Box<dyn VoiceDetector>,
        stt: Box<dyn SpeechToText>,
    ) -> std::io::Result<(Self, mpsc::Receiver<Input>)> {
        let (events, receiver) = mpsc::channel(SPEECH_IN_EVENT_QUEUE);
        let (control, stt_messages) = std_mpsc::sync_channel(STT_QUEUE_MESSAGES);
        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(SpeechInCounters::default());
        let stt_thread = {
            let worker = SttWorker::new(
                stt,
                events.clone(),
                Arc::clone(&counters),
                Arc::clone(&stop),
            );
            std::thread::Builder::new()
                .name("speech-in-stt".to_owned())
                .spawn(move || worker.run(&stt_messages))?
        };
        let vad = VadWorker {
            mic,
            detector,
            machine: VadMachine::default(),
            recent: VecDeque::with_capacity(RECENT_FRAMES),
            quiet_sent: false,
            stt: control.clone(),
            events,
            counters: Arc::clone(&counters),
            stop: Arc::clone(&stop),
            muted: Arc::clone(&muted),
        };
        let vad_thread = std::thread::Builder::new()
            .name("speech-in-vad".to_owned())
            .spawn(move || vad.run())?;
        let handle = Self {
            stop,
            muted,
            control,
            counters,
            threads: vec![vad_thread, stt_thread],
        };
        Ok((handle, receiver))
    }

    /// Terms that bias recognition from the next decode on.
    pub fn set_vocabulary(&self, terms: Vec<String>) {
        let _ = self.control.try_send(SttMessage::Vocabulary(terms));
    }

    /// While muted, mic audio is discarded unheard and any utterance in progress is dropped.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    #[must_use]
    pub fn counters(&self) -> &SpeechInCounters {
        &self.counters
    }

    /// Stops both threads and waits for them (at most one decode).
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for SpeechIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct VadWorker {
    mic: MicReader,
    detector: Box<dyn VoiceDetector>,
    machine: VadMachine,
    /// The latest audio, for the pre-roll.
    recent: VecDeque<f32>,
    /// The speech-to-text thread was told the speaker went quiet (and has not been told that
    /// speech resumed).
    quiet_sent: bool,
    stt: SyncSender<SttMessage>,
    events: mpsc::Sender<Input>,
    counters: Arc<SpeechInCounters>,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

impl VadWorker {
    fn run(mut self) {
        let mut window = [0.0_f32; VAD_WINDOW_FRAMES];
        let mut filled = 0;
        let mut was_muted = false;
        while !self.stop.load(Ordering::Acquire) {
            if self.muted.load(Ordering::Acquire) {
                if !was_muted {
                    self.mute();
                    was_muted = true;
                }
                self.mic.skip_all();
                filled = 0;
                std::thread::sleep(Duration::from_millis(VAD_POLL_INTERVAL_MS));
                continue;
            }
            was_muted = false;
            filled += self.mic.read(&mut window[filled..]);
            if filled < VAD_WINDOW_FRAMES {
                std::thread::sleep(Duration::from_millis(VAD_POLL_INTERVAL_MS));
                continue;
            }
            filled = 0;
            if !self.window(&window) {
                return;
            }
        }
    }

    /// Handles one full window; false when the orchestrator is gone.
    fn window(&mut self, window: &[f32; VAD_WINDOW_FRAMES]) -> bool {
        self.remember(window);
        let probability = self.detector.probability(window).unwrap_or_else(|error| {
            self.counters
                .detector_errors
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%error, "voice detector failed on a window");
            0.0
        });
        let was_speaking = self.machine.in_speech();
        match self.machine.step(probability) {
            // The orchestrator hears of the start and end before the speech-to-text thread does,
            // so a transcript never arrives ahead of the `VoiceEnd` it belongs to.
            Some(VadEvent::Start { onset_frame }) => {
                let delivered = self.events.blocking_send(Input::VoiceStart).is_ok();
                let audio = self.onset_audio(onset_frame);
                let _ = self.stt.send(SttMessage::Begin(audio));
                self.quiet_sent = false;
                delivered
            }
            Some(VadEvent::End { voiced_ms }) => {
                let delivered = self
                    .events
                    .blocking_send(Input::VoiceEnd { voiced_ms })
                    .is_ok();
                self.send_audio(window);
                let _ = self.stt.send(SttMessage::End);
                self.quiet_sent = false;
                delivered
            }
            None => {
                if was_speaking {
                    self.send_audio(window);
                    self.track_quiet();
                }
                true
            }
        }
    }

    /// Tells the speech-to-text thread when the speaker has gone quiet long enough to decode the
    /// final early, and when they speak again.
    fn track_quiet(&mut self) {
        let quiet_ms = self.machine.quiet_ms();
        if quiet_ms >= EARLY_FINAL_AFTER_QUIET_MS && !self.quiet_sent {
            self.quiet_sent = self.stt.try_send(SttMessage::Quiet).is_ok();
        } else if quiet_ms == 0 && self.quiet_sent {
            let _ = self.stt.send(SttMessage::Resumed);
            self.quiet_sent = false;
        }
    }

    fn remember(&mut self, window: &[f32]) {
        let excess = (self.recent.len() + window.len()).saturating_sub(RECENT_FRAMES);
        self.recent.drain(..excess.min(self.recent.len()));
        self.recent.extend(window);
    }

    /// The audio from the pre-roll before the onset up to now.
    fn onset_audio(&self, onset_frame: u64) -> Vec<f32> {
        let wanted = self.machine.next_frame() - onset_frame + PREROLL_FRAMES;
        let count = usize::try_from(wanted)
            .unwrap_or(usize::MAX)
            .min(self.recent.len());
        self.recent
            .range(self.recent.len() - count..)
            .copied()
            .collect()
    }

    fn send_audio(&self, window: &[f32]) {
        match self.stt.try_send(SttMessage::Audio(window.to_vec())) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                self.counters
                    .windows_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Drops the utterance in progress unheard. The core is muted, so no `VoiceEnd` follows.
    fn mute(&mut self) {
        if self.machine.in_speech() {
            let _ = self.stt.send(SttMessage::Discard);
        }
        self.machine.reset();
        self.detector.reset();
        self.recent.clear();
    }
}
