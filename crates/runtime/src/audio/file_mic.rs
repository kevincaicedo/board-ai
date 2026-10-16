//! A microphone made of audio clips (LLD §7): one thread writes the mic ring in real time,
//! silence between clips and a clip whenever one is queued, so speech in hears it exactly as it
//! would hear a person: same ring, same pace, same windows. The end-to-end test speaks through
//! it; nothing needs a microphone or its permission.
//!
//! Writes go in [`FILE_MIC_CHUNK_MS`] slices on absolute deadlines (a late wake-up catches up,
//! it never drifts), about the size of a real device's IO buffer. `speed` above 1 writes faster
//! than real time, for tests that only care about what is heard.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::{MIC_RATE_HZ, MicWriter};
use crate::limits::FILE_MIC_QUEUE_CLIPS;

/// One write to the ring: about a real device's IO buffer.
pub const FILE_MIC_CHUNK_MS: u64 = 10;
const CHUNK_FRAMES: usize = (MIC_RATE_HZ as u64 * FILE_MIC_CHUNK_MS / 1_000) as usize;

/// When a clip went into the ring: its first and last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipPlayed {
    pub started: Instant,
    pub ended: Instant,
}

/// Why a clip was not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMicError {
    /// [`FILE_MIC_QUEUE_CLIPS`] clips are already waiting.
    Full,
    /// The mic's thread has stopped.
    Stopped,
}

struct Clip {
    samples: Arc<[f32]>,
    played: oneshot::Sender<ClipPlayed>,
}

/// The running mic. Dropping it stops the thread.
pub struct FileMic {
    clips: SyncSender<Clip>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FileMic {
    /// Starts writing silence into `writer`; `speed` 1 is real time.
    pub fn start(writer: MicWriter, speed: f64) -> std::io::Result<Self> {
        let (clips, queue) = sync_channel(FILE_MIC_QUEUE_CLIPS);
        let stop = Arc::new(AtomicBool::new(false));
        let speed = if speed.is_finite() && speed > 0.0 {
            speed
        } else {
            1.0
        };
        let chunk = Duration::from_secs_f64(FILE_MIC_CHUNK_MS as f64 / 1_000.0 / speed);
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("file-mic".to_owned())
                .spawn(move || run(writer, &queue, &stop, chunk))?
        };
        Ok(Self {
            clips,
            stop,
            thread: Some(thread),
        })
    }

    /// Queues `samples` (16 kHz mono) to play after any clip before it; the answer says when
    /// it went into the ring.
    pub fn play(&self, samples: Arc<[f32]>) -> Result<oneshot::Receiver<ClipPlayed>, FileMicError> {
        let (played, answer) = oneshot::channel();
        match self.clips.try_send(Clip { samples, played }) {
            Ok(()) => Ok(answer),
            Err(TrySendError::Full(_)) => Err(FileMicError::Full),
            Err(TrySendError::Disconnected(_)) => Err(FileMicError::Stopped),
        }
    }
}

impl Drop for FileMic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The clip being written and how far.
struct Playing {
    clip: Clip,
    written: usize,
    started: Instant,
}

fn run(mut writer: MicWriter, queue: &Receiver<Clip>, stop: &AtomicBool, chunk: Duration) {
    let silence = [0.0_f32; CHUNK_FRAMES];
    let mut playing: Option<Playing> = None;
    let mut due = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if playing.is_none() {
            match queue.try_recv() {
                Ok(clip) => {
                    playing = Some(Playing {
                        clip,
                        written: 0,
                        started: Instant::now(),
                    });
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let finished = match playing.as_mut() {
            Some(current) => {
                let samples = &current.clip.samples;
                let end = (current.written + CHUNK_FRAMES).min(samples.len());
                writer.write(&samples[current.written..end]);
                // A short last slice is padded, so the ring keeps real time.
                writer.write(&silence[..CHUNK_FRAMES - (end - current.written)]);
                current.written = end;
                end == samples.len()
            }
            None => {
                writer.write(&silence);
                false
            }
        };
        if finished && let Some(done) = playing.take() {
            let _ = done.clip.played.send(ClipPlayed {
                started: done.started,
                ended: Instant::now(),
            });
        }
        due += chunk;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{FileMic, FileMicError};
    use crate::audio::{MIC_RATE_HZ, mic_ring};
    use crate::limits::FILE_MIC_QUEUE_CLIPS;

    #[tokio::test]
    async fn clips_play_in_order_between_silence_at_the_requested_pace() {
        let (writer, mut reader) = mic_ring(MIC_RATE_HZ as usize * 2);
        let mic = FileMic::start(writer, 1.0).expect("starts");
        let first: Arc<[f32]> = vec![0.5; 1_600].into();
        let second: Arc<[f32]> = vec![-0.5; 800].into();
        let first = mic.play(first).expect("queued");
        let second = mic.play(second).expect("queued");
        let first = first.await.expect("played");
        let second = second.await.expect("played");
        assert!(second.started >= first.ended);
        // 100 ms of audio takes about 100 ms to write: the pace is real time.
        assert!(first.ended - first.started >= Duration::from_millis(80));
        drop(mic);
        let mut heard = vec![0.0; MIC_RATE_HZ as usize * 2];
        let count = reader.read(&mut heard);
        let heard = &heard[..count];
        let loud: Vec<f32> = heard.iter().copied().filter(|s| s.abs() > 0.1).collect();
        assert_eq!(loud.len(), 2_400);
        assert!(loud[..1_600].iter().all(|s| *s > 0.0));
        assert!(loud[1_600..].iter().all(|s| *s < 0.0));
    }

    #[test]
    fn a_full_queue_refuses_more_clips() {
        let (writer, _reader) = mic_ring(1_024);
        let mic = FileMic::start(writer, 1.0).expect("starts");
        let long: Arc<[f32]> = vec![0.1; MIC_RATE_HZ as usize * 10].into();
        let mut refused = None;
        for _ in 0..FILE_MIC_QUEUE_CLIPS + 2 {
            if let Err(error) = mic.play(Arc::clone(&long)) {
                refused = Some(error);
                break;
            }
        }
        assert_eq!(refused, Some(FileMicError::Full));
    }
}
