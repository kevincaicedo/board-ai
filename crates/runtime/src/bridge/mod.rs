//! The bridge (LLD §3.1, §3.7): UI events out of the orchestrator, in batches per 16 ms frame,
//! to whatever carries them to the UI (a Tauri channel now, a WebSocket later).
//!
//! The first step after a quiet frame goes out at once, so a lone stroke is never delayed;
//! steps that follow within the frame wait for its end and go out together. Each batch is
//! stamped with the latest step time it carries, so the UI's clock never shows an event before
//! the core produced it (a stroke never starts before its word, I1).

mod frame;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::protocol::{UiBatch, UiEvent};
use boardai_core::time::SessionTime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use frame::Frame;
pub use frame::FrameCounts;

use crate::limits::{BRIDGE_BATCH_EVENTS_MAX, BRIDGE_FRAME_MS, BRIDGE_QUEUE_STEPS};

/// Where batches go. The bridge calls it from its own task, in order, one batch at a time; it
/// must not block (a transport queues the batch).
pub trait UiSink: Send + Sync {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed>;
}

/// The UI is gone (the window closed or reloaded). The batch is lost; the bridge counts it and
/// keeps going, since a new connection may come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiSinkClosed;

impl fmt::Display for UiSinkClosed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the UI is not connected")
    }
}

/// The bridge's task has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BridgeClosed;

/// What the bridge did over its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BridgeStats {
    pub batches: u64,
    pub events: u64,
    pub frame: FrameCounts,
    /// Batches the sink refused.
    pub lost_batches: u64,
}

/// One step's events, stamped with the step's session time.
#[derive(Debug)]
struct Step {
    at: SessionTime,
    events: Vec<UiEvent>,
}

/// The orchestrator's end of the bridge. Dropping it, or [`BridgeHandle::close`], lets the
/// bridge send what it holds and end.
#[derive(Debug)]
pub struct BridgeHandle {
    sender: mpsc::Sender<Step>,
    task: JoinHandle<BridgeStats>,
}

impl BridgeHandle {
    /// Starts the bridge's task. It ends when the handle is closed or dropped.
    pub fn spawn(sink: Arc<dyn UiSink>) -> Self {
        let (sender, receiver) = mpsc::channel(BRIDGE_QUEUE_STEPS);
        let task = tokio::spawn(run(receiver, sink));
        Self { sender, task }
    }

    /// Hands over one step's events. Waits when the bridge is [`BRIDGE_QUEUE_STEPS`] behind:
    /// drawing events are never dropped.
    pub async fn send(&self, at: SessionTime, events: Vec<UiEvent>) -> Result<(), BridgeClosed> {
        if events.is_empty() {
            return Ok(());
        }
        self.sender
            .send(Step { at, events })
            .await
            .map_err(|_| BridgeClosed)
    }

    /// Sends what is left and waits for the task to end.
    pub async fn close(self) -> BridgeStats {
        drop(self.sender);
        self.task.await.unwrap_or_default()
    }
}

async fn run(mut receiver: mpsc::Receiver<Step>, sink: Arc<dyn UiSink>) -> BridgeStats {
    let frame_length = Duration::from_millis(BRIDGE_FRAME_MS);
    let mut frame = Frame::default();
    let mut stats = BridgeStats::default();
    let mut full = Vec::new();
    let mut flush_at: Option<Instant> = None;
    let mut last_flush: Option<Instant> = None;
    loop {
        // The frame's end comes first, so a steady stream of steps can't hold a batch back.
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(flush_at.unwrap_or_else(Instant::now)), if flush_at.is_some() => {
                // Steps already waiting belong to this frame.
                while let Ok(step) = receiver.try_recv() {
                    frame.push(step.at, step.events, &mut full);
                }
                deliver(&*sink, full.drain(..).chain(frame.take()), &mut stats);
                flush_at = None;
                last_flush = Some(Instant::now());
            }
            step = receiver.recv() => {
                let Some(step) = step else { break };
                frame.push(step.at, step.events, &mut full);
                if !full.is_empty() {
                    tracing::warn!(
                        events_max = BRIDGE_BATCH_EVENTS_MAX,
                        "a UI frame reached its event limit; the batch went out early"
                    );
                }
                deliver(&*sink, full.drain(..), &mut stats);
                if flush_at.is_none() && !frame.is_empty() {
                    let next_frame = last_flush.map_or_else(Instant::now, |at| at + frame_length);
                    flush_at = Some(next_frame.max(Instant::now()));
                }
            }
        }
    }
    deliver(&*sink, frame.take(), &mut stats);
    stats.frame = frame.counts;
    stats
}

fn deliver(sink: &dyn UiSink, batches: impl IntoIterator<Item = UiBatch>, stats: &mut BridgeStats) {
    for batch in batches {
        let events = u64::try_from(batch.events.len()).unwrap_or(u64::MAX);
        match sink.send(batch) {
            Ok(()) => {
                stats.batches += 1;
                stats.events += events;
            }
            Err(UiSinkClosed) => {
                stats.lost_batches += 1;
                tracing::debug!(events, "UI batch lost: the UI is not connected");
            }
        }
    }
}

#[cfg(test)]
mod tests;
