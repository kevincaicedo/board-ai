//! One run at a time of an ask whose answer is shared (hygiene, review 3): the settings page
//! asks for the status and saves at once, and each asked Ollama for its models. Asks made while
//! one runs wait for its answer; the first ask after it answered runs again, so an answer is
//! never older than the ask.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::OnceCell;

/// The ask in flight, which every concurrent ask shares.
#[derive(Debug)]
pub(crate) struct SingleFlight<T> {
    // Held only to take or clear the cell, never across an await.
    current: Mutex<Option<Arc<OnceCell<T>>>>,
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
}

impl<T: Clone> SingleFlight<T> {
    /// `ask`'s answer, or the answer of the run already in flight. A run whose caller went away
    /// is taken over by the next waiter (`OnceCell`), so dropping an ask is a valid cancellation.
    pub(crate) async fn run<F>(&self, ask: impl FnOnce() -> F) -> T
    where
        F: Future<Output = T>,
    {
        let cell = {
            let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(current.get_or_insert_with(Arc::default))
        };
        let answer = cell.get_or_init(ask).await.clone();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, &cell))
        {
            *current = None;
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::SingleFlight;

    /// Two asks at once run once and share the answer; the next ask runs again.
    #[test]
    fn concurrent_asks_share_one_run() {
        tauri::async_runtime::block_on(async {
            let flight = SingleFlight::<usize>::default();
            let runs = Arc::new(AtomicUsize::new(0));
            let ask = || {
                let runs = Arc::clone(&runs);
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    runs.fetch_add(1, Ordering::SeqCst) + 1
                }
            };
            let (first, second) = tokio::join!(flight.run(ask), flight.run(ask));
            assert_eq!((first, second), (1, 1));
            assert_eq!(flight.run(ask).await, 2, "a later ask runs again");
        });
    }
}
