//! The main thread's event loop, for headless binaries.
//!
//! `AVSpeechSynthesizer` delivers its buffers and markers on the main dispatch queue, whichever
//! thread asked for them (measured on macOS 26). The app's main thread runs AppKit's event loop,
//! which drains that queue; a probe, a spike or an example has to run the loop itself, from
//! `main`, while its real work happens on other threads.

use std::time::Duration;

use objc2_foundation::{NSDate, NSRunLoop, NSThread};

/// One turn of the loop; short enough that `done` is noticed quickly.
const SLICE: Duration = Duration::from_millis(20);

/// Runs the current thread's run loop until `done` returns true. Call it from `main`: on any
/// other thread the main queue is not drained and the system voice never answers.
pub fn run_main_loop_until(mut done: impl FnMut() -> bool) {
    debug_assert!(NSThread::isMainThread_class(), "call from the main thread");
    let run_loop = NSRunLoop::currentRunLoop();
    while !done() {
        let until = NSDate::dateWithTimeIntervalSinceNow(SLICE.as_secs_f64());
        run_loop.runUntilDate(&until);
    }
}
