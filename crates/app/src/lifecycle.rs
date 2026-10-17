//! The session's lifecycle as one task (`session::serve_lifecycle`): starting, restarting and
//! closing are served one at a time, in the order asked. A restart asked while a start is still
//! opening the store waits for it, so two orchestrators or two store writers never run (law 3),
//! and a close during a start ends what that start opened.

use std::future::Future;

use tokio::sync::{mpsc, oneshot};

/// Lifecycle requests waiting for the task; a sender waits for room (only setup, the settings
/// page and quitting ask, one at a time).
pub const LIFECYCLE_REQUESTS_MAX: usize = 4;

/// One request to the lifecycle task, answered once it is done.
#[derive(Debug)]
pub enum Lifecycle {
    /// Opens the session unless one is open or the app is quitting; answered once the session
    /// has read its settings and runs.
    Start { opened: oneshot::Sender<()> },
    /// Ends the session as quitting does, then opens one from the settings as they are now.
    Restart { opened: oneshot::Sender<()> },
    /// Quitting: ends the session, then audio, then the store; nothing opens after it.
    Close { closed: oneshot::Sender<()> },
}

/// What the lifecycle drives: the app's session, or a fake in tests.
pub trait SessionLife {
    /// A session is open.
    fn is_open(&self) -> bool;
    /// Opens one: settings, the store, audio, the talker, the orchestrator.
    fn open(&self) -> impl Future<Output = ()> + Send;
    /// Ends the open one, then audio, then the store; nothing when none is open.
    fn shut_down(&self) -> impl Future<Output = ()> + Send;
}

/// Serves requests in order until every sender is gone.
pub async fn serve(mut inbox: mpsc::Receiver<Lifecycle>, life: impl SessionLife) {
    let mut quitting = false;
    while let Some(request) = inbox.recv().await {
        match request {
            Lifecycle::Start { opened } => {
                if !quitting && !life.is_open() {
                    life.open().await;
                }
                // The asker may have stopped waiting (quitting past its bound).
                let _ = opened.send(());
            }
            Lifecycle::Restart { opened } => {
                if !quitting {
                    life.shut_down().await;
                    tracing::info!("settings changed; a new session starts");
                    life.open().await;
                }
                let _ = opened.send(());
            }
            Lifecycle::Close { closed } => {
                quitting = true;
                life.shut_down().await;
                let _ = closed.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};

    use super::{LIFECYCLE_REQUESTS_MAX, Lifecycle, SessionLife, serve};

    /// Sessions that take a while to open and close, counting how many are open at once.
    #[derive(Clone, Default)]
    struct Recorded {
        open: Arc<AtomicUsize>,
        most_open: Arc<AtomicUsize>,
        opening: Arc<AtomicUsize>,
        steps: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Recorded {
        fn note(&self, step: &'static str) {
            if let Ok(mut steps) = self.steps.lock() {
                steps.push(step);
            }
        }

        fn steps(&self) -> Vec<&'static str> {
            self.steps
                .lock()
                .map(|steps| steps.clone())
                .unwrap_or_default()
        }
    }

    impl SessionLife for Recorded {
        fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst) > 0
        }

        fn open(&self) -> impl Future<Output = ()> + Send {
            let this = self.clone();
            async move {
                let opening = this.opening.fetch_add(1, Ordering::SeqCst) + 1;
                this.most_open.fetch_max(opening, Ordering::SeqCst);
                this.note("open");
                // The store opens and the scribe's key is read: long enough for a restart to
                // be asked meanwhile.
                tokio::time::sleep(Duration::from_millis(50)).await;
                let open = this.open.fetch_add(1, Ordering::SeqCst) + 1;
                this.most_open.fetch_max(open, Ordering::SeqCst);
                this.opening.fetch_sub(1, Ordering::SeqCst);
            }
        }

        fn shut_down(&self) -> impl Future<Output = ()> + Send {
            let this = self.clone();
            async move {
                this.note("shut down");
                tokio::time::sleep(Duration::from_millis(10)).await;
                this.open.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Review 2, finding 11: a restart asked while a start is still opening the store made a
    /// second orchestrator and a second store writer.
    #[test]
    fn a_restart_during_a_start_waits_for_it_and_one_session_runs() {
        tauri::async_runtime::block_on(async {
            let life = Recorded::default();
            let (requests, inbox) = mpsc::channel(LIFECYCLE_REQUESTS_MAX);
            let served = tauri::async_runtime::spawn(serve(inbox, life.clone()));
            let (started, start_done) = oneshot::channel();
            let (restarted, restart_done) = oneshot::channel();
            let (again, again_done) = oneshot::channel();
            for request in [
                Lifecycle::Start { opened: started },
                Lifecycle::Restart { opened: restarted },
                Lifecycle::Start { opened: again },
            ] {
                assert!(requests.send(request).await.is_ok());
            }
            assert!(start_done.await.is_ok());
            assert!(restart_done.await.is_ok());
            assert!(again_done.await.is_ok());
            assert_eq!(life.steps(), ["open", "shut down", "open"]);
            assert_eq!(
                life.most_open.load(Ordering::SeqCst),
                1,
                "never two at once"
            );
            assert!(life.is_open());
            let (closed, close_done) = oneshot::channel();
            let (late, late_done) = oneshot::channel();
            assert!(requests.send(Lifecycle::Close { closed }).await.is_ok());
            assert!(
                requests
                    .send(Lifecycle::Start { opened: late })
                    .await
                    .is_ok()
            );
            assert!(close_done.await.is_ok());
            assert!(late_done.await.is_ok());
            assert!(!life.is_open(), "nothing opens after quitting");
            drop(requests);
            assert!(served.await.is_ok(), "the task ends with its senders");
        });
    }
}
