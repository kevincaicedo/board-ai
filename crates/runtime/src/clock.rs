//! Runtime time (LLD §3.1). All runtime time comes through [`Clock`], so the session can run on
//! a fake clock in tests and replay. The core only ever sees the [`SessionTime`] it returns.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use boardai_core::time::SessionTime;

/// Monotonic milliseconds since the session started.
pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> SessionTime;
}

/// The real clock: `Instant` is monotonic, so session time never runs backwards even when the
/// wall clock is adjusted.
#[derive(Clone, Copy, Debug)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    /// Starts a session clock at zero now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> SessionTime {
        // A session lasting u64::MAX milliseconds saturates rather than wrapping.
        let elapsed_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        SessionTime::from_ms(elapsed_ms)
    }
}

/// Session time on tokio's clock. In the app it reads the same monotonic time as
/// [`MonotonicClock`]; under a paused tokio runtime (tests) it follows tokio's time, so the
/// session's timers and the session time it stamps on inputs always agree.
#[derive(Clone, Copy, Debug)]
pub struct TokioClock {
    start: tokio::time::Instant,
}

impl TokioClock {
    /// Starts a session clock at zero now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            start: tokio::time::Instant::now(),
        }
    }

    /// The tokio instant at which session time reads `at`, for timers.
    #[must_use]
    pub fn instant_at(&self, at: SessionTime) -> tokio::time::Instant {
        self.start + std::time::Duration::from_millis(at.as_ms())
    }
}

impl Clock for TokioClock {
    fn now(&self) -> SessionTime {
        let elapsed_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        SessionTime::from_ms(elapsed_ms)
    }
}

/// A clock that moves only when told to. Clones share the same time.
#[derive(Clone, Debug, Default)]
pub struct FakeClock {
    now_ms: Arc<AtomicU64>,
}

impl FakeClock {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn at(time: SessionTime) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(time.as_ms())),
        }
    }

    /// Moves time forward; it saturates instead of wrapping, so it never goes backwards.
    pub fn advance_ms(&self, ms: u64) {
        let mut current = self.now_ms.load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(ms);
            match self.now_ms.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SessionTime {
        SessionTime::from_ms(self.now_ms.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, FakeClock, MonotonicClock};
    use boardai_core::time::SessionTime;

    #[test]
    fn fake_clock_moves_only_forward_and_is_shared() {
        let clock = FakeClock::at(SessionTime::from_ms(5));
        let view = clock.clone();
        clock.advance_ms(10);
        assert_eq!(view.now(), SessionTime::from_ms(15));
        clock.advance_ms(u64::MAX);
        assert_eq!(view.now(), SessionTime::from_ms(u64::MAX));
    }

    #[tokio::test(start_paused = true)]
    async fn tokio_clock_follows_paused_time() {
        let clock = super::TokioClock::start();
        tokio::time::advance(std::time::Duration::from_millis(1_250)).await;
        assert_eq!(clock.now(), SessionTime::from_ms(1_250));
        assert_eq!(
            clock.instant_at(SessionTime::from_ms(1_250)),
            tokio::time::Instant::now()
        );
    }

    #[test]
    fn monotonic_clock_starts_near_zero_and_never_decreases() {
        let clock = MonotonicClock::start();
        let first = clock.now();
        let second = clock.now();
        assert!(first.as_ms() < 1_000);
        assert!(second >= first);
    }
}
