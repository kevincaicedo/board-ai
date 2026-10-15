//! Audio I/O (LLD §3.1, §3.4): the VoiceProcessingIO unit (echo cancellation, noise
//! suppression, gain control) feeding two rings.
//!
//! - **Mic ring**: 16 kHz mono `f32`, [`MIC_RING_MS`]. The capture callback writes it; speech in
//!   reads it through [`MicReader`]. When full, the oldest frames are dropped and counted.
//! - **Voice ring**: 24 kHz mono `f32`, [`VOICE_RING_MS`]. Speech out writes it through
//!   [`VoiceOutput`]; the playback callback reads it through [`PlaybackRenderer`], which owns the
//!   played-frames counter, the duck gain and the stop fade.
//!
//! [`AudioIo::start`] opens the unit and returns both ring ends. Tests and the conversation
//! simulator build the same rings with [`mic_ring`] and [`voice_ring`] and drive the renderer
//! themselves, so everything above the device runs without hardware.

pub(crate) mod coreaudio;
mod mic;
pub mod resample;
mod voice;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use coreaudio::run_main_loop_until;
pub use mic::{MicReader, MicWriter, mic_ring};
pub use voice::{PlaybackRenderer, StopAcknowledged, VoiceOutput, voice_ring};

use self::resample::ResamplerError;
use crate::limits::{MIC_RING_MS, VOICE_RING_MS};

/// The mic ring's rate: what the VAD and Whisper take.
pub const MIC_RATE_HZ: u32 = 16_000;
/// The voice ring's rate: what every text-to-speech engine is converted to.
pub const VOICE_RATE_HZ: u32 = 24_000;

/// Frames in the mic ring.
pub const MIC_RING_FRAMES: usize = (MIC_RING_MS * MIC_RATE_HZ / 1_000) as usize;
/// Frames in the voice ring.
pub const VOICE_RING_FRAMES: usize = (VOICE_RING_MS * VOICE_RATE_HZ / 1_000) as usize;

/// Which microphone the unit listens to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputChoice {
    /// The system's default input.
    #[default]
    SystemDefault,
    /// The Mac's built-in microphone, for when the default is a virtual device (an audio
    /// interface's mixer, a meeting app's loopback) that hears nothing.
    BuiltIn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioConfig {
    pub input: InputChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The system has no VoiceProcessingIO unit.
    VoiceProcessingUnavailable,
    /// The built-in microphone was asked for and none was found.
    NoBuiltInInput,
    /// The unit runs at a rate the converter cannot handle.
    UnsupportedRate { rate_hz: u32 },
    /// A CoreAudio call failed with this status.
    Os { call: &'static str, status: i32 },
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VoiceProcessingUnavailable => {
                formatter.write_str("the voice processing audio unit is not available")
            }
            Self::NoBuiltInInput => formatter.write_str("this Mac has no built-in microphone"),
            Self::UnsupportedRate { rate_hz } => {
                write!(
                    formatter,
                    "the audio unit runs at an unsupported {rate_hz} Hz"
                )
            }
            Self::Os { call, status } => write!(formatter, "{call} failed with status {status}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<ResamplerError> for AudioError {
    fn from(error: ResamplerError) -> Self {
        match error {
            ResamplerError::ZeroRate => Self::UnsupportedRate { rate_hz: 0 },
            ResamplerError::UnsupportedRatio { input_hz, .. } => {
                Self::UnsupportedRate { rate_hz: input_hz }
            }
        }
    }
}

/// A count shared with the real-time thread: a relaxed atomic add, never a lock.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    pub fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// What went wrong on the real-time thread, counted instead of logged (law 7).
#[derive(Debug, Default)]
pub struct AudioCounters {
    /// `AudioUnitRender` failed for the mic; that cycle's audio is lost.
    pub capture_errors: Counter,
    /// A cycle asked for more frames than the preallocated buffers hold; it got silence.
    pub oversized_cycles: Counter,
    /// A callback panicked (a broken invariant); the cycle was dropped.
    pub callback_panics: Counter,
}

/// The running audio unit. Dropping it stops the unit; the ring ends stay valid but go quiet.
pub struct AudioIo {
    unit: coreaudio::VoiceProcessingUnit,
    counters: Arc<AudioCounters>,
}

impl fmt::Debug for AudioIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioIo")
            .field("facts", &self.unit.facts())
            .finish_non_exhaustive()
    }
}

impl AudioIo {
    /// Opens VoiceProcessingIO and starts it. Needs microphone permission: without it macOS
    /// delivers silence rather than an error.
    pub fn start(config: AudioConfig) -> Result<(Self, MicReader, VoiceOutput), AudioError> {
        let (mic_writer, mic_reader) = mic_ring(MIC_RING_FRAMES);
        let counters = Arc::new(AudioCounters::default());
        // The latency is only known once the unit runs; the voice output is rebuilt with it.
        let (pending_output, renderer) = voice_ring(VOICE_RING_FRAMES, 0);
        let unit = coreaudio::VoiceProcessingUnit::start(
            config.input,
            mic_writer,
            renderer,
            Arc::clone(&counters),
        )?;
        let latency_seconds = unit.facts().output_latency_seconds;
        let latency_frames = (latency_seconds * f64::from(VOICE_RATE_HZ)).round() as u64;
        let output = pending_output.with_output_latency_frames(latency_frames);
        Ok((Self { unit, counters }, mic_reader, output))
    }

    /// The unit's client rate; capture and playback are converted from and to it.
    #[must_use]
    pub fn unit_rate_hz(&self) -> u32 {
        self.unit.facts().unit_rate_hz
    }

    /// From a voice frame leaving the ring to the speaker.
    #[must_use]
    pub fn output_latency_ms(&self) -> f64 {
        self.unit.facts().output_latency_seconds * 1_000.0
    }

    /// Whether macOS accepted the request to duck other apps' audio as little as possible.
    #[must_use]
    pub fn other_audio_ducking_minimized(&self) -> bool {
        self.unit.facts().other_audio_ducking_minimized
    }

    #[must_use]
    pub fn counters(&self) -> &AudioCounters {
        &self.counters
    }
}
