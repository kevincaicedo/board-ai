//! The voice ring and its real-time renderer (LLD §3.1).
//!
//! Speech out writes 24 kHz audio into an `rtrb` ring through [`VoiceOutput`]; the playback
//! callback reads it through [`PlaybackRenderer`]. They share only atomics:
//!
//! - `played_frames`: frames taken from the ring and sent to the device. Discarded frames are
//!   never counted, so the counter is exactly the audio that will be heard (with the device's
//!   output latency).
//! - the target gain, which the renderer approaches per sample ([`VOICE_GAIN_RAMP_MS`]);
//! - a stop request and its acknowledgement, numbered so an old answer is never mistaken for a
//!   new one. The renderer fades out over [`VOICE_FADE_MS`], drops what is left in the ring and
//!   publishes the counter at that point.
//!
//! The renderer never allocates, locks, logs or blocks (law 7).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use boardai_core::effect::Gain;

use super::VOICE_RATE_HZ;
use crate::limits::{VOICE_FADE_MS, VOICE_GAIN_RAMP_MS};

/// Creates the voice ring. `output_latency_frames` is the device's latency in 24 kHz frames,
/// measured by the audio unit (or chosen by a test).
#[must_use]
pub fn voice_ring(
    capacity_frames: usize,
    output_latency_frames: u64,
) -> (VoiceOutput, PlaybackRenderer) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity_frames.max(1));
    let shared = Arc::new(PlaybackShared {
        played_frames: AtomicU64::new(0),
        target_gain_bits: AtomicU32::new(Gain::FULL.value().to_bits()),
        stop_requested: AtomicU64::new(0),
        stop_acknowledged: AtomicU64::new(0),
        stopped_at_frames: AtomicU64::new(0),
    });
    let output = VoiceOutput {
        producer,
        shared: Arc::clone(&shared),
        output_latency_frames,
        stop_generation: 0,
    };
    let renderer = PlaybackRenderer {
        consumer,
        shared,
        gain: Gain::FULL.value(),
        target_gain: Gain::FULL.value(),
        played_frames: 0,
        fade: Fade::Idle,
        acknowledged: 0,
    };
    (output, renderer)
}

#[derive(Debug)]
struct PlaybackShared {
    played_frames: AtomicU64,
    target_gain_bits: AtomicU32,
    stop_requested: AtomicU64,
    stop_acknowledged: AtomicU64,
    /// `played_frames` when the stop numbered `stop_acknowledged` finished; written before the
    /// acknowledgement is published.
    stopped_at_frames: AtomicU64,
}

/// The speech-out side of the voice ring.
#[derive(Debug)]
pub struct VoiceOutput {
    producer: rtrb::Producer<f32>,
    shared: Arc<PlaybackShared>,
    output_latency_frames: u64,
    stop_generation: u64,
}

/// The voice's gain, set from outside speech out. One atomic store; the renderer ramps to it.
#[derive(Clone, Debug)]
pub struct VoiceGain {
    shared: Arc<PlaybackShared>,
}

impl VoiceGain {
    pub fn set(&self, gain: Gain) {
        self.shared
            .target_gain_bits
            .store(gain.value().to_bits(), Ordering::Release);
    }

    /// The gain the renderer is ramping to.
    #[must_use]
    pub fn target(&self) -> Gain {
        Gain::new(f32::from_bits(
            self.shared.target_gain_bits.load(Ordering::Acquire),
        ))
    }
}

/// A stop the renderer has finished: the counter at the end of the fade. Every frame up to it
/// reaches the speaker (the last [`VOICE_FADE_MS`] attenuated); nothing after it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopAcknowledged {
    pub played_frames: u64,
}

impl VoiceOutput {
    /// The same output with the latency measured once the device runs.
    #[must_use]
    pub fn with_output_latency_frames(self, output_latency_frames: u64) -> Self {
        Self {
            output_latency_frames,
            ..self
        }
    }

    /// Copies as much of `samples` as fits and returns how many frames were taken.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let (pushed, _rest) = self.producer.push_partial_slice(samples);
        pushed.len()
    }

    /// Room in the ring, in frames.
    #[must_use]
    pub fn free_frames(&self) -> usize {
        self.producer.slots()
    }

    /// Frames sent to the device so far.
    #[must_use]
    pub fn played_frames(&self) -> u64 {
        self.shared.played_frames.load(Ordering::Acquire)
    }

    /// The device's output latency in 24 kHz frames: a frame counted as played is heard this
    /// much later.
    #[must_use]
    pub const fn output_latency_frames(&self) -> u64 {
        self.output_latency_frames
    }

    /// Sets the gain the renderer ramps to.
    pub fn set_gain(&self, gain: Gain) {
        self.shared
            .target_gain_bits
            .store(gain.value().to_bits(), Ordering::Release);
    }

    /// A handle on the gain alone, for a caller that must duck without waiting behind speech
    /// out's queue (a barge-in has 100 ms).
    #[must_use]
    pub fn gain_control(&self) -> VoiceGain {
        VoiceGain {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Asks the renderer to fade out and drop the rest of the ring. Answered through
    /// [`VoiceOutput::stop_acknowledged`].
    pub fn request_stop(&mut self) {
        self.stop_generation += 1;
        self.shared
            .stop_requested
            .store(self.stop_generation, Ordering::Release);
    }

    /// The answer to the latest [`VoiceOutput::request_stop`], once the renderer has given it.
    #[must_use]
    pub fn stop_acknowledged(&self) -> Option<StopAcknowledged> {
        let acknowledged = self.shared.stop_acknowledged.load(Ordering::Acquire);
        (acknowledged == self.stop_generation && acknowledged > 0).then(|| StopAcknowledged {
            played_frames: self.shared.stopped_at_frames.load(Ordering::Acquire),
        })
    }

    /// Whether a stop is still being carried out by the renderer.
    #[must_use]
    pub fn stop_pending(&self) -> bool {
        self.shared.stop_acknowledged.load(Ordering::Acquire) != self.stop_generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    Idle,
    /// Frames left in the fade, and the request it answers.
    Running {
        remaining_frames: u32,
        request: u64,
    },
}

const FADE_FRAMES: u32 = VOICE_FADE_MS * VOICE_RATE_HZ / 1_000;
const GAIN_STEP_PER_FRAME: f32 = 1.0 / (VOICE_GAIN_RAMP_MS * VOICE_RATE_HZ / 1_000) as f32;

/// The real-time side of the voice ring. Owned by the playback callback.
#[derive(Debug)]
pub struct PlaybackRenderer {
    consumer: rtrb::Consumer<f32>,
    shared: Arc<PlaybackShared>,
    gain: f32,
    target_gain: f32,
    played_frames: u64,
    fade: Fade,
    acknowledged: u64,
}

impl PlaybackRenderer {
    /// Reads the controls once per device cycle, so a cycle sees one consistent gain target and
    /// stop request.
    pub fn begin_cycle(&mut self) {
        self.target_gain = f32::from_bits(self.shared.target_gain_bits.load(Ordering::Acquire));
        let requested = self.shared.stop_requested.load(Ordering::Acquire);
        if requested != self.acknowledged && self.fade == Fade::Idle {
            self.fade = Fade::Running {
                remaining_frames: FADE_FRAMES,
                request: requested,
            };
        }
    }

    /// The next output frame: voice with gain and fade applied, or silence when the ring is
    /// empty (silence is not counted as played).
    pub fn next_frame(&mut self) -> f32 {
        if let Fade::Running {
            remaining_frames: 0,
            request,
        } = self.fade
        {
            self.finish_stop(request);
        }
        let Ok(sample) = self.consumer.pop() else {
            if let Fade::Running { request, .. } = self.fade {
                // Nothing left to fade: the stop is complete now.
                self.finish_stop(request);
            }
            return 0.0;
        };
        self.played_frames += 1;
        self.step_gain();
        let fade = match &mut self.fade {
            Fade::Idle => 1.0,
            Fade::Running {
                remaining_frames, ..
            } => {
                *remaining_frames -= 1;
                *remaining_frames as f32 / FADE_FRAMES as f32
            }
        };
        sample * self.gain * fade
    }

    /// Fills `out` with consecutive frames.
    pub fn render(&mut self, out: &mut [f32]) {
        for frame in out {
            *frame = self.next_frame();
        }
    }

    /// Publishes the counter; called once per device cycle, after the frames are rendered.
    pub fn end_cycle(&mut self) {
        if let Fade::Running {
            remaining_frames: 0,
            request,
        } = self.fade
        {
            self.finish_stop(request);
        }
        self.shared
            .played_frames
            .store(self.played_frames, Ordering::Release);
    }

    fn step_gain(&mut self) {
        let difference = self.target_gain - self.gain;
        self.gain += difference.clamp(-GAIN_STEP_PER_FRAME, GAIN_STEP_PER_FRAME);
    }

    fn finish_stop(&mut self, request: u64) {
        if let Ok(chunk) = self.consumer.read_chunk(self.consumer.slots()) {
            chunk.commit_all();
        }
        self.fade = Fade::Idle;
        self.acknowledged = request;
        self.shared
            .played_frames
            .store(self.played_frames, Ordering::Release);
        self.shared
            .stopped_at_frames
            .store(self.played_frames, Ordering::Release);
        self.shared
            .stop_acknowledged
            .store(request, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::effect::Gain;

    use super::{FADE_FRAMES, voice_ring};

    fn render(renderer: &mut super::PlaybackRenderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        renderer.begin_cycle();
        renderer.render(&mut out);
        renderer.end_cycle();
        out
    }

    #[test]
    fn played_frames_count_voice_but_not_silence() {
        let (mut output, mut renderer) = voice_ring(1_000, 0);
        assert_eq!(output.push(&[0.5; 300]), 300);
        let out = render(&mut renderer, 400);
        assert!(out[..300].iter().all(|sample| (*sample - 0.5).abs() < 1e-6));
        assert!(out[300..].iter().all(|sample| *sample == 0.0));
        assert_eq!(output.played_frames(), 300);
    }

    #[test]
    fn a_full_ring_takes_only_what_fits() {
        let (mut output, _renderer) = voice_ring(100, 0);
        assert_eq!(output.push(&[0.1; 150]), 100);
        assert_eq!(output.free_frames(), 0);
    }

    #[test]
    fn ducking_ramps_without_jumps() {
        let (mut output, mut renderer) = voice_ring(10_000, 0);
        output.push(&[1.0; 2_000]);
        output.set_gain(Gain::DUCKED);
        let out = render(&mut renderer, 2_000);
        let largest_step = out
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(largest_step < 0.01, "a click: {largest_step}");
        assert!((out[1_999] - 0.25).abs() < 1e-3, "{}", out[1_999]);
        // The ramp reaches the duck level within the ramp time (10 ms = 240 frames).
        assert!((out[300] - 0.25).abs() < 1e-3, "{}", out[300]);
    }

    #[test]
    fn stop_fades_out_drops_the_rest_and_reports_the_counter() {
        let (mut output, mut renderer) = voice_ring(10_000, 0);
        output.push(&[1.0; 5_000]);
        render(&mut renderer, 1_000);
        output.request_stop();
        assert!(output.stop_pending());
        assert_eq!(output.stop_acknowledged(), None);
        let out = render(&mut renderer, 2_000);
        let fade = FADE_FRAMES as usize;
        assert!(out[..fade].windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(out[fade..].iter().all(|sample| *sample == 0.0));
        let acknowledged = output.stop_acknowledged().map(|stop| stop.played_frames);
        assert_eq!(acknowledged, Some(1_000 + u64::from(FADE_FRAMES)));
        assert!(!output.stop_pending());
        // The ring is empty: new audio plays from the start.
        output.push(&[0.5; 10]);
        let out = render(&mut renderer, 10);
        assert!((out[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn stopping_an_empty_ring_is_acknowledged_at_once() {
        let (mut output, mut renderer) = voice_ring(100, 0);
        output.request_stop();
        render(&mut renderer, 10);
        assert_eq!(
            output.stop_acknowledged().map(|stop| stop.played_frames),
            Some(0)
        );
    }

    #[test]
    fn an_old_acknowledgement_never_answers_a_new_stop() {
        let (mut output, mut renderer) = voice_ring(100, 0);
        output.request_stop();
        render(&mut renderer, 10);
        assert!(output.stop_acknowledged().is_some());
        output.request_stop();
        assert_eq!(output.stop_acknowledged(), None);
        render(&mut renderer, 10);
        assert!(output.stop_acknowledged().is_some());
    }
}
