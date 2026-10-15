//! The bridge's timing, on tokio's paused clock.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::ids::LayerId;
use boardai_core::protocol::{OrbState, UiBatch, UiEvent};
use boardai_core::time::SessionTime;
use tokio::time::Instant;

use super::{BridgeHandle, UiSink, UiSinkClosed};

#[derive(Default)]
struct RecordingSink {
    batches: Mutex<Vec<(Instant, UiBatch)>>,
    closed: Mutex<bool>,
}

impl RecordingSink {
    fn batches(&self) -> Vec<(Instant, UiBatch)> {
        self.batches
            .lock()
            .map(|batches| batches.clone())
            .unwrap_or_default()
    }

    fn close(&self) {
        if let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
    }
}

impl UiSink for RecordingSink {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        if self.closed.lock().map(|closed| *closed).unwrap_or(true) {
            return Err(UiSinkClosed);
        }
        if let Ok(mut batches) = self.batches.lock() {
            batches.push((Instant::now(), batch));
        }
        Ok(())
    }
}

fn at(ms: u64) -> SessionTime {
    SessionTime::from_ms(ms)
}

fn freeze(layer: u8) -> UiEvent {
    UiEvent::Freeze {
        layer: LayerId(layer),
    }
}

fn orb(level_percent: u8) -> UiEvent {
    UiEvent::Orb {
        state: OrbState::Speaking,
        level_percent,
    }
}

async fn wait_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[tokio::test(start_paused = true)]
async fn the_first_step_goes_at_once_and_the_rest_of_the_frame_waits() {
    let sink = Arc::new(RecordingSink::default());
    let bridge = BridgeHandle::spawn(Arc::clone(&sink) as Arc<dyn UiSink>);
    let start = Instant::now();
    assert!(bridge.send(at(0), vec![freeze(0)]).await.is_ok());
    wait_ms(1).await;
    assert_eq!(sink.batches().len(), 1, "a lone step is not delayed");
    assert!(bridge.send(at(2), vec![freeze(1), orb(10)]).await.is_ok());
    assert!(bridge.send(at(5), vec![orb(30), freeze(2)]).await.is_ok());
    wait_ms(9).await;
    assert_eq!(sink.batches().len(), 1, "the frame is still open");
    wait_ms(10).await;
    let batches = sink.batches();
    assert_eq!(batches.len(), 2);
    let (sent_at, second) = &batches[1];
    assert_eq!(sent_at.duration_since(start), Duration::from_millis(16));
    assert_eq!(second.at, at(5));
    assert_eq!(second.events, vec![freeze(1), orb(30), freeze(2)]);
    let stats = bridge.close().await;
    assert_eq!(stats.batches, 2);
    assert_eq!(stats.events, 4);
    assert_eq!(stats.frame.coalesced, 1);
}

#[tokio::test(start_paused = true)]
async fn closing_sends_what_is_left_and_a_closed_sink_only_counts() {
    let sink = Arc::new(RecordingSink::default());
    let bridge = BridgeHandle::spawn(Arc::clone(&sink) as Arc<dyn UiSink>);
    assert!(bridge.send(at(0), vec![freeze(0)]).await.is_ok());
    assert!(bridge.send(at(1), vec![freeze(1)]).await.is_ok());
    assert!(bridge.send(at(1), Vec::new()).await.is_ok());
    let stats = bridge.close().await;
    let events: Vec<UiEvent> = sink
        .batches()
        .into_iter()
        .flat_map(|(_, batch)| batch.events)
        .collect();
    assert_eq!(events, vec![freeze(0), freeze(1)], "in order, nothing lost");
    assert_eq!(stats.lost_batches, 0);

    let gone = Arc::new(RecordingSink::default());
    gone.close();
    let bridge = BridgeHandle::spawn(Arc::clone(&gone) as Arc<dyn UiSink>);
    assert!(bridge.send(at(0), vec![freeze(0)]).await.is_ok());
    let stats = bridge.close().await;
    assert_eq!(stats.lost_batches, 1);
    assert_eq!(stats.batches, 0);
}
