//! A cancellation signal shared by the effect that starts work and the work itself.
//!
//! Dropping a stream is always a valid cancellation (AGENTS.md async rules). The token covers
//! what a drop cannot: a harness that must be asked to stop (SIGINT, then a grace period) before
//! it is killed, and a stream whose owner wants it to end without dropping it. A dozen lines here
//! instead of a `tokio-util` dependency.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// Cancelled once, forever; clones share the state.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Resolves once the token is cancelled. Safe to drop and re-create at any time.
    pub async fn cancelled(&self) {
        loop {
            // `notified()` is registered at creation, so a `cancel` between the check and the
            // await still wakes it.
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;

    #[tokio::test]
    async fn waiters_wake_on_cancel_and_late_waiters_return_at_once() {
        let token = CancelToken::new();
        let waiter = token.clone();
        let task = tokio::spawn(async move { waiter.cancelled().await });
        tokio::task::yield_now().await;
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(task.await.is_ok());
        token.cancelled().await;
        assert!(token.is_cancelled());
    }
}
