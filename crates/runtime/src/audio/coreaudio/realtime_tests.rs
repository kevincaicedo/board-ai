//! Law 7 as a test: the code the CoreAudio callbacks run never allocates. A counting global
//! allocator (this test binary only) counts allocations made on the current thread while a
//! region is being watched; a few thousand capture and playback cycles must count zero.
//!
//! It lives here because a global allocator is an `unsafe impl`, and this module is the only
//! place `unsafe` is allowed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use super::unit::{CaptureState, PlaybackState, convert_capture, render_playback};
use crate::audio::resample::Resampler;
use crate::audio::{AudioCounters, MIC_RATE_HZ, VOICE_RATE_HZ, mic_ring, voice_ring};
use crate::limits::AUDIO_CALLBACK_FRAMES_MAX;

struct CountingAllocator;

thread_local! {
    // `const` initialization: reading these never allocates, so the allocator cannot recurse.
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn note_allocation() {
    if WATCHING.with(Cell::get) {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: every call is forwarded unchanged to the system allocator, which upholds the
// `GlobalAlloc` contract; counting touches only const-initialized thread-locals.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller's layout contract passes through unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: `pointer` came from this allocator (that is, from `System`) with `layout`.
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` came from this allocator (that is, from `System`) with `layout`.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations `work` makes on this thread.
fn allocations_in(work: impl FnOnce()) -> u64 {
    ALLOCATIONS.with(|count| count.set(0));
    WATCHING.with(|watching| watching.set(true));
    work();
    WATCHING.with(|watching| watching.set(false));
    ALLOCATIONS.with(Cell::get)
}

const UNIT_RATE_HZ: u32 = 44_100;
/// What VoiceProcessingIO asks for per cycle at 44.1 kHz.
const CYCLE_FRAMES: usize = 470;

#[test]
fn the_callbacks_never_allocate() {
    let counters = Arc::new(AudioCounters::default());
    let (writer, mut reader) = mic_ring(MIC_RATE_HZ as usize * 2);
    let mut capture = CaptureState {
        unit: std::ptr::null_mut(),
        rendered: vec![0.25; AUDIO_CALLBACK_FRAMES_MAX].into_boxed_slice(),
        converted: vec![0.0; AUDIO_CALLBACK_FRAMES_MAX].into_boxed_slice(),
        resampler: Resampler::new(UNIT_RATE_HZ, MIC_RATE_HZ).expect("ratio"),
        writer,
        counters: Arc::clone(&counters),
    };
    let (mut output, renderer) = voice_ring(VOICE_RATE_HZ as usize * 4, 0);
    let mut playback = PlaybackState {
        renderer,
        resampler: Resampler::new(VOICE_RATE_HZ, UNIT_RATE_HZ).expect("ratio"),
        counters,
    };
    output.push(&vec![0.5; VOICE_RATE_HZ as usize * 2]);
    let mut out = vec![0.0; CYCLE_FRAMES];
    let allocations = allocations_in(|| {
        for cycle in 0..2_000 {
            convert_capture(&mut capture, CYCLE_FRAMES);
            render_playback(&mut playback, &mut out);
            if cycle == 500 {
                output.set_gain(boardai_core::effect::Gain::DUCKED);
            }
            if cycle == 1_000 {
                output.request_stop();
            }
        }
    });
    assert_eq!(allocations, 0, "the real-time path allocated");
    // The work was real: audio reached the mic ring and the voice was played, then stopped.
    assert!(reader.available() > 0);
    assert!(output.stop_acknowledged().is_some());
    let mut drained = vec![0.0; 16];
    assert_eq!(reader.read(&mut drained), 16);
}

#[test]
fn the_counter_sees_an_allocation() {
    // The check above can fail: a single allocation in the watched region is counted.
    let allocations = allocations_in(|| {
        let boxed = Box::new(7_u64);
        std::hint::black_box(&boxed);
    });
    assert_eq!(allocations, 1);
}
