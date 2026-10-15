//! The Tauri side of the UI transport (D-027): the bridge's [`UiSink`] sends each batch on the
//! channel the UI registered with `connect`, in order.
//!
//! The session starts before the webview has loaded, so batches made before the first
//! `connect` wait here (bounded) and go out first when it comes.

use std::collections::VecDeque;
use std::sync::Mutex;

use boardai_core::protocol::UiBatch;
use boardai_runtime::bridge::{UiSink, UiSinkClosed};
use tauri::ipc::Channel;
use tokio::sync::watch;

/// Batches kept while no UI is connected (startup: the initial orb and thread). Past it the
/// oldest is dropped and counted; the UI starts from the same defaults anyway.
pub const WAITING_BATCHES_MAX: usize = 64;

#[derive(Default)]
struct State {
    channel: Option<Channel<UiBatch>>,
    waiting: VecDeque<UiBatch>,
    dropped_waiting: u64,
}

pub struct TauriSink {
    // Held only for a send or a swap, never across an await.
    state: Mutex<State>,
    connected: watch::Sender<bool>,
}

impl TauriSink {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            connected: watch::Sender::new(false),
        }
    }

    /// The UI registered its channel. Waiting batches go first, so order is kept. A second
    /// connect (a reload) replaces the first; that UI starts with an empty board.
    pub fn connect(&self, channel: Channel<UiBatch>) {
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("the UI channel lock is poisoned; the UI stays disconnected");
            return;
        };
        for batch in state.waiting.drain(..) {
            if channel.send(batch).is_err() {
                tracing::warn!("the UI went away while connecting");
                return;
            }
        }
        if state.channel.replace(channel).is_some() {
            tracing::warn!("the UI reconnected; it starts from an empty board");
        }
        if state.dropped_waiting > 0 {
            tracing::warn!(
                dropped = state.dropped_waiting,
                "UI batches dropped before connect"
            );
        }
        self.connected.send_replace(true);
    }

    /// Resolves once a UI has connected.
    pub async fn wait_connected(&self) {
        let mut connected = self.connected.subscribe();
        // An error means the sender is gone: the app is closing, nothing to wait for.
        let _ = connected.wait_for(|connected| *connected).await;
    }
}

impl UiSink for TauriSink {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        let Ok(mut state) = self.state.lock() else {
            return Err(UiSinkClosed);
        };
        if let Some(channel) = &state.channel {
            return channel.send(batch).map_err(|_| UiSinkClosed);
        }
        if state.waiting.len() >= WAITING_BATCHES_MAX {
            state.waiting.pop_front();
            state.dropped_waiting += 1;
        }
        state.waiting.push_back(batch);
        Ok(())
    }
}
