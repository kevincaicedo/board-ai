//! The mic ring: one writer (the real-time capture callback) and one reader (speech in).
//!
//! When the reader falls more than the capacity behind, the writer overwrites the oldest frames
//! and the reader counts them as dropped (LLD §5). The writer never waits and never allocates,
//! so the real-time thread never blocks (law 7). `rtrb` cannot do this: its producer cannot
//! discard the oldest items, only refuse new ones, which would keep stale audio and lose the
//! words being spoken now.
//!
//! Samples live as `f32` bits in atomics, so torn reads are impossible without `unsafe`. A read
//! that races an overwrite is detected with the seqlock pattern: the writer announces how far it
//! is about to write (`claimed`) before touching the slots, and the reader checks that
//! announcement after copying and discards anything that may have been overwritten.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// Creates a ring of `capacity_frames` frames.
#[must_use]
pub fn mic_ring(capacity_frames: usize) -> (MicWriter, MicReader) {
    let capacity = capacity_frames.max(1);
    let shared = Arc::new(Shared {
        slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
        claimed: AtomicU64::new(0),
        written: AtomicU64::new(0),
    });
    (
        MicWriter {
            shared: Arc::clone(&shared),
            written: 0,
        },
        MicReader {
            shared,
            read: 0,
            dropped_frames: 0,
        },
    )
}

#[derive(Debug)]
struct Shared {
    slots: Box<[AtomicU32]>,
    /// Frames the writer may be overwriting right now: everything below `claimed - capacity`
    /// is gone or going.
    claimed: AtomicU64,
    /// Frames completely written.
    written: AtomicU64,
}

impl Shared {
    fn capacity(&self) -> u64 {
        self.slots.len() as u64
    }

    fn slot(&self, frame: u64) -> &AtomicU32 {
        // The modulo keeps the index inside the slice; `slots` is never empty.
        let index = usize::try_from(frame % self.capacity()).unwrap_or(0);
        &self.slots[index]
    }
}

/// The real-time side. Allocation-free and lock-free.
#[derive(Debug)]
pub struct MicWriter {
    shared: Arc<Shared>,
    written: u64,
}

impl MicWriter {
    pub fn write(&mut self, samples: &[f32]) {
        let end = self.written + samples.len() as u64;
        self.shared.claimed.store(end, Ordering::Relaxed);
        // Orders the announcement before the slot stores below (seqlock writer).
        fence(Ordering::Release);
        for (frame, sample) in (self.written..end).zip(samples) {
            self.shared
                .slot(frame)
                .store(sample.to_bits(), Ordering::Relaxed);
        }
        self.written = end;
        self.shared.written.store(end, Ordering::Release);
    }
}

/// The speech-in side.
#[derive(Debug)]
pub struct MicReader {
    shared: Arc<Shared>,
    read: u64,
    dropped_frames: u64,
}

impl MicReader {
    /// Copies up to `out.len()` of the oldest unread frames into `out` and returns how many.
    /// Frames overwritten before they could be read are skipped and counted.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let written = self.shared.written.load(Ordering::Acquire);
        self.skip_overwritten(written);
        let available = usize::try_from(written - self.read).unwrap_or(usize::MAX);
        let count = available.min(out.len());
        for (offset, slot) in out[..count].iter_mut().enumerate() {
            let bits = self
                .shared
                .slot(self.read + offset as u64)
                .load(Ordering::Relaxed);
            *slot = f32::from_bits(bits);
        }
        // Pairs with the writer's fence: if any copied value came from a newer write, the
        // claim made before that write is visible now (seqlock reader).
        fence(Ordering::Acquire);
        let claimed = self.shared.claimed.load(Ordering::Relaxed);
        let valid_from = claimed.saturating_sub(self.shared.capacity());
        let torn = usize::try_from(valid_from.saturating_sub(self.read))
            .unwrap_or(usize::MAX)
            .min(count);
        if torn > 0 {
            out.copy_within(torn..count, 0);
            self.dropped_frames += torn as u64;
        }
        let valid = count - torn;
        self.read += count as u64;
        valid
    }

    /// Frames unread right now (an overrun not yet skipped is counted as available).
    #[must_use]
    pub fn available(&self) -> usize {
        let written = self.shared.written.load(Ordering::Acquire);
        let unread = written - self.read;
        usize::try_from(unread.min(self.shared.capacity())).unwrap_or(usize::MAX)
    }

    /// Frames lost because the reader fell more than the ring's capacity behind.
    #[must_use]
    pub const fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Discards everything unread, for example while the mic is muted.
    pub fn skip_all(&mut self) {
        self.read = self.shared.written.load(Ordering::Acquire);
    }

    fn skip_overwritten(&mut self, written: u64) {
        let oldest = written.saturating_sub(self.shared.capacity());
        if self.read < oldest {
            self.dropped_frames += oldest - self.read;
            self.read = oldest;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mic_ring;

    fn ramp(begin: u32, count: u32) -> Vec<f32> {
        (begin..begin + count).map(|value| value as f32).collect()
    }

    #[test]
    fn frames_come_out_in_order() {
        let (mut writer, mut reader) = mic_ring(8);
        writer.write(&ramp(0, 5));
        let mut out = [0.0; 3];
        assert_eq!(reader.read(&mut out), 3);
        assert_eq!(out, [0.0, 1.0, 2.0]);
        writer.write(&ramp(5, 2));
        let mut out = [0.0; 8];
        assert_eq!(reader.read(&mut out), 4);
        assert_eq!(out[..4], [3.0, 4.0, 5.0, 6.0]);
        assert_eq!(reader.read(&mut out), 0);
        assert_eq!(reader.dropped_frames(), 0);
    }

    #[test]
    fn a_full_ring_drops_the_oldest_frames_and_counts_them() {
        let (mut writer, mut reader) = mic_ring(4);
        writer.write(&ramp(0, 10));
        assert_eq!(reader.available(), 4);
        let mut out = [0.0; 8];
        assert_eq!(reader.read(&mut out), 4);
        assert_eq!(out[..4], [6.0, 7.0, 8.0, 9.0]);
        assert_eq!(reader.dropped_frames(), 6);
    }

    #[test]
    fn skip_all_discards_unread_frames_without_counting_them() {
        let (mut writer, mut reader) = mic_ring(4);
        writer.write(&ramp(0, 3));
        reader.skip_all();
        writer.write(&ramp(3, 1));
        let mut out = [0.0; 4];
        assert_eq!(reader.read(&mut out), 1);
        assert_eq!(out[0], 3.0);
        assert_eq!(reader.dropped_frames(), 0);
    }

    /// The writer laps the reader constantly. Every frame the reader gets must be exactly the
    /// next one it has not read or seen dropped: a stale or torn value fails.
    #[test]
    fn concurrent_overwrites_never_yield_stale_frames() {
        let (mut writer, mut reader) = mic_ring(64);
        let total: u32 = 2_000_000;
        let producer = std::thread::spawn(move || {
            for chunk_begin in (0..total).step_by(37) {
                writer.write(&ramp(chunk_begin, 37.min(total - chunk_begin)));
            }
        });
        let mut out = [0.0_f32; 60];
        let mut received: u64 = 0;
        loop {
            let count = reader.read(&mut out);
            // Frames before this read, plus any this read skipped as overwritten.
            let first = received + reader.dropped_frames();
            for (offset, value) in out[..count].iter().enumerate() {
                assert_eq!(
                    *value,
                    (first + offset as u64) as f32,
                    "frame {}",
                    first + offset as u64
                );
            }
            received += count as u64;
            if producer.is_finished() && reader.available() == 0 {
                break;
            }
        }
        assert!(producer.join().is_ok());
        assert_eq!(received + reader.dropped_frames(), u64::from(total));
    }
}
