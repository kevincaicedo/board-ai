//! Model downloads started from the first-run screen. Each file downloads at most once at a
//! time (two writers would corrupt its `.part`), progress goes to the screen's channel at most
//! every [`SETUP_PROGRESS_INTERVAL_MS`], and dropping the download (the app quitting) keeps the
//! partial file for the next attempt.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use boardai_core::setup::{DownloadEvent, DownloadProblem};
use boardai_runtime::models::{DownloadProgress, ModelError, ModelId, ModelStore};

/// Least time between two progress events on the screen's channel; the last one always goes.
/// The runtime already reports at most every MiB, so a fast connection is what this bounds.
pub const SETUP_PROGRESS_INTERVAL_MS: u64 = 100;

/// The files downloading now. At most one entry per `ModelId`, so it is bounded by the
/// manifest.
#[derive(Clone, Debug, Default)]
pub struct Downloads {
    // Held only to insert, remove or look up one id.
    running: Arc<Mutex<BTreeSet<ModelId>>>,
}

/// A running download's registration; dropping it (the download ended or was dropped)
/// unregisters the file.
#[derive(Debug)]
pub struct Running {
    running: Arc<Mutex<BTreeSet<ModelId>>>,
    id: ModelId,
}

impl Drop for Running {
    fn drop(&mut self) {
        match self.running.lock() {
            Ok(mut running) => {
                running.remove(&self.id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&self.id);
            }
        }
    }
}

impl Downloads {
    /// Registers a download of `id`; `None` while one is already running.
    pub fn begin(&self, id: ModelId) -> Option<Running> {
        let mut running = match self.running.lock() {
            Ok(running) => running,
            Err(poisoned) => poisoned.into_inner(),
        };
        running.insert(id).then(|| Running {
            running: Arc::clone(&self.running),
            id,
        })
    }

    pub fn is_running(&self, id: ModelId) -> bool {
        match self.running.lock() {
            Ok(running) => running.contains(&id),
            Err(poisoned) => poisoned.into_inner().contains(&id),
        }
    }
}

/// Lets a progress event through when the interval has passed since the last one, and always
/// lets the final one through.
#[derive(Debug)]
pub struct ProgressThrottle {
    last_sent_ms: Option<u64>,
}

impl ProgressThrottle {
    pub const fn new() -> Self {
        Self { last_sent_ms: None }
    }

    pub fn admit(&mut self, now_ms: u64, last: bool) -> bool {
        let due = self
            .last_sent_ms
            .is_none_or(|sent| now_ms.saturating_sub(sent) >= SETUP_PROGRESS_INTERVAL_MS);
        if due || last {
            self.last_sent_ms = Some(now_ms);
        }
        due || last
    }
}

/// Downloads (or resumes) `id`, sending throttled progress, then `Done` or `Failed`.
pub async fn download(
    store: &ModelStore,
    id: ModelId,
    send: &(dyn Fn(DownloadEvent) + Send + Sync),
) -> Result<(), DownloadProblem> {
    let started = Instant::now();
    let mut throttle = ProgressThrottle::new();
    let mut report = |progress: DownloadProgress| {
        let now_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let last = progress.received_bytes >= progress.total_bytes;
        if throttle.admit(now_ms, last) {
            send(DownloadEvent::Progress {
                received_bytes: progress.received_bytes,
                total_bytes: progress.total_bytes,
            });
        }
    };
    match store.fetch(&id.file(), &mut report).await {
        Ok(_) => {
            send(DownloadEvent::Done);
            Ok(())
        }
        Err(error) => {
            tracing::warn!(model = id.name(), %error, "model download failed");
            let problem = problem(&error);
            send(DownloadEvent::Failed { problem });
            Err(problem)
        }
    }
}

const fn problem(error: &ModelError) -> DownloadProblem {
    match error {
        ModelError::Network { .. } => DownloadProblem::Network,
        ModelError::Http { status } => DownloadProblem::Server { status: *status },
        ModelError::BadResponse { .. } => DownloadProblem::Unexpected,
        ModelError::Corrupt { .. } => DownloadProblem::Corrupt,
        ModelError::Io { .. } => DownloadProblem::Disk,
    }
}

#[cfg(test)]
mod tests {
    use boardai_runtime::models::ModelId;

    use super::{Downloads, ProgressThrottle, SETUP_PROGRESS_INTERVAL_MS};

    #[test]
    fn a_file_downloads_once_at_a_time() {
        let downloads = Downloads::default();
        let first = downloads.begin(ModelId::SileroVad);
        assert!(first.is_some());
        assert!(downloads.is_running(ModelId::SileroVad));
        assert!(downloads.begin(ModelId::SileroVad).is_none());
        assert!(
            downloads
                .begin(ModelId::WhisperBaseEn)
                .is_some_and(|_| true)
        );
        drop(first);
        assert!(!downloads.is_running(ModelId::SileroVad));
        assert!(downloads.begin(ModelId::SileroVad).is_some());
    }

    #[test]
    fn progress_is_throttled_and_the_last_event_always_goes() {
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.admit(0, false));
        assert!(!throttle.admit(SETUP_PROGRESS_INTERVAL_MS - 1, false));
        assert!(throttle.admit(SETUP_PROGRESS_INTERVAL_MS, false));
        assert!(throttle.admit(SETUP_PROGRESS_INTERVAL_MS + 1, true));
        assert!(!throttle.admit(SETUP_PROGRESS_INTERVAL_MS + 2, false));
        let sent = (0..1_000_u64)
            .filter(|tick| throttle.admit(1_000 + tick, false))
            .count();
        assert_eq!(sent, 10);
    }
}
