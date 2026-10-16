//! A speaker that plays nowhere (LLD §7): one thread renders the voice ring in real time and
//! discards it, so speech out, its playback clock and the strokes it drives run exactly as on a
//! device, with no audio hardware and nothing to hear. Its output latency is zero.
//!
//! It is not the real-time audio thread, but it renders like one: buffers are allocated once,
//! and cycles run on absolute deadlines so the clock never drifts.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{PlaybackRenderer, VOICE_RATE_HZ};

/// One render cycle: about a device's IO buffer.
pub const NULL_OUTPUT_CYCLE_MS: u64 = 10;
const CYCLE_FRAMES: usize = (VOICE_RATE_HZ as u64 * NULL_OUTPUT_CYCLE_MS / 1_000) as usize;
/// A frame louder than this counts as voice in [`NullOutput::voiced_frames`].
const VOICED_LEVEL: f32 = 0.01;

/// The running output. Dropping it stops the thread.
pub struct NullOutput {
    stop: Arc<AtomicBool>,
    voiced: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl NullOutput {
    pub fn start(mut renderer: PlaybackRenderer) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let voiced = Arc::new(AtomicU64::new(0));
        let thread = {
            let stop = Arc::clone(&stop);
            let voiced = Arc::clone(&voiced);
            std::thread::Builder::new()
                .name("null-output".to_owned())
                .spawn(move || {
                    let mut buffer = [0.0_f32; CYCLE_FRAMES];
                    let cycle = Duration::from_millis(NULL_OUTPUT_CYCLE_MS);
                    let mut due = Instant::now();
                    while !stop.load(Ordering::Acquire) {
                        renderer.begin_cycle();
                        renderer.render(&mut buffer);
                        renderer.end_cycle();
                        let loud = buffer.iter().filter(|s| s.abs() > VOICED_LEVEL).count();
                        voiced.fetch_add(loud as u64, Ordering::Relaxed);
                        due += cycle;
                        let now = Instant::now();
                        if due > now {
                            std::thread::sleep(due - now);
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            voiced,
            thread: Some(thread),
        })
    }

    /// Frames played so far that carried voice (above a whisper), at 24 kHz.
    #[must_use]
    pub fn voiced_frames(&self) -> u64 {
        self.voiced.load(Ordering::Relaxed)
    }
}

impl Drop for NullOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::NullOutput;
    use crate::audio::{VOICE_RATE_HZ, voice_ring};

    #[test]
    fn it_plays_the_ring_in_real_time() {
        let (mut output, renderer) = voice_ring(VOICE_RATE_HZ as usize, 0);
        let tenth = VOICE_RATE_HZ as usize / 10;
        assert_eq!(output.push(&vec![0.5; 2 * tenth]), 2 * tenth);
        let started = Instant::now();
        let device = NullOutput::start(renderer).expect("starts");
        while output.played_frames() < 2 * tenth as u64 {
            assert!(started.elapsed() < Duration::from_secs(2), "it plays");
            std::thread::sleep(Duration::from_millis(5));
        }
        // 200 ms of audio takes about 200 ms to play: the clock is real time.
        assert!(started.elapsed() >= Duration::from_millis(180));
        assert_eq!(device.voiced_frames(), 2 * tenth as u64);
    }
}
