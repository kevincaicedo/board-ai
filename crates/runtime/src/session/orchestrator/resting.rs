//! The resting stage (D-145): once a session rests, its digest is written in the background,
//! then the library is read for the stage and shown, the session just ended first. Neither wait
//! ever holds the orchestrator's loop: the next words open a new session at once, and the
//! phase they replace aborts what was still running.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::protocol::UiEvent;
use boardai_core::tools::ToolOutput;
use tokio::task::JoinHandle;

use super::{Orchestrator, Phase};
use crate::clock::Clock;
use crate::limits::SESSION_TOOL_TIMEOUT_MS;
use crate::session::ports::StoreRead;

/// The library on the resting stage (D-145).
pub(super) enum RestingLibrary {
    /// Not on the stage: before the digest is written, after Esc, or when the read failed.
    Hidden,
    /// Being read beside the loop, so a slow store never holds the next session's words.
    Reading(LibraryRead),
    /// On the stage, so Esc can close it.
    Shown,
}

/// The library read for the resting stage, bounded by `SESSION_TOOL_TIMEOUT_MS` (`None` past
/// it). Its owner is the resting phase: replacing or dropping it aborts the read.
pub(super) struct LibraryRead(JoinHandle<Option<ToolOutput>>);

impl Drop for LibraryRead {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What finished while resting.
pub(super) enum RestingWork {
    /// The ended session's digest is written.
    DigestWritten,
    /// The library read answered (`None`: it failed or ran past its bound).
    Library(Option<ToolOutput>),
}

impl Orchestrator {
    /// Something runs for the resting stage: the digest, then the library read.
    pub(super) fn resting_work(&self) -> bool {
        matches!(
            self.phase,
            Phase::Resting {
                finishing: Some(_),
                ..
            } | Phase::Resting {
                library: RestingLibrary::Reading(_),
                ..
            }
        )
    }

    pub(super) async fn on_resting_work(&mut self, done: RestingWork) {
        match done {
            RestingWork::DigestWritten => self.read_resting_library(),
            RestingWork::Library(read) => self.show_resting_library(read).await,
        }
    }

    /// The ended session's digest is written: the library is read for the resting stage, the
    /// session just ended first (D-145), beside the loop and within `SESSION_TOOL_TIMEOUT_MS`.
    fn read_resting_library(&mut self) {
        let Phase::Resting {
            finishing, library, ..
        } = &mut self.phase
        else {
            return;
        };
        *finishing = None;
        let store = Arc::clone(&self.store);
        let bound = Duration::from_millis(SESSION_TOOL_TIMEOUT_MS);
        let read = tokio::spawn(async move {
            tokio::time::timeout(bound, store.read(StoreRead::Library))
                .await
                .ok()
        });
        *library = RestingLibrary::Reading(LibraryRead(read));
    }

    /// The library read answered: it shows on the resting stage. A store that didn't answer in
    /// time, or failed, leaves the stage clear.
    async fn show_resting_library(&mut self, read: Option<ToolOutput>) {
        let Phase::Resting { library, .. } = &mut self.phase else {
            return;
        };
        let Some(ToolOutput::View { view, today }) = read else {
            *library = RestingLibrary::Hidden;
            tracing::warn!("the library could not be read for the resting stage");
            return;
        };
        *library = RestingLibrary::Shown;
        let view = *view;
        self.send_resting(vec![UiEvent::ViewShow { view, today }])
            .await;
    }

    /// Esc on the resting library closes it; Esc before it arrived means it never shows.
    pub(super) async fn close_resting_library(&mut self) {
        let Phase::Resting { library, .. } = &mut self.phase else {
            return;
        };
        let shown = matches!(library, RestingLibrary::Shown);
        *library = RestingLibrary::Hidden;
        if shown {
            self.send_resting(vec![UiEvent::ViewDismiss]).await;
        }
    }

    /// Events for the resting stage: straight to the UI, never to the ended session's log.
    pub(super) async fn send_resting(&mut self, events: Vec<UiEvent>) {
        let now = self.clock.now();
        if self.bridge.send(now, events).await.is_err() {
            tracing::error!("the UI bridge has stopped; UI events are lost");
        }
    }
}

/// Resolves when the resting stage's current wait is done: the digest being written, then the
/// library read; never otherwise. The phase changes right after, so a finished task is never
/// awaited again.
pub(super) async fn work_done(phase: &mut Phase) -> RestingWork {
    match phase {
        Phase::Resting {
            finishing: Some(finishing),
            ..
        } => {
            if let Err(error) = (&mut finishing.task).await {
                tracing::warn!(%error, "the session's digest task ended abnormally");
            }
            RestingWork::DigestWritten
        }
        Phase::Resting {
            library: RestingLibrary::Reading(read),
            ..
        } => match (&mut read.0).await {
            Ok(answer) => RestingWork::Library(answer),
            Err(error) => {
                tracing::warn!(%error, "the resting library's read ended abnormally");
                RestingWork::Library(None)
            }
        },
        Phase::Resting { .. } | Phase::Active { .. } => std::future::pending().await,
    }
}
