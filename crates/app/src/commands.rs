//! The two commands of the UI contract (D-027, `ui/src/transport/tauri.ts`).

use boardai_core::protocol::{UiBatch, UiCommand};
use boardai_runtime::session::InboxRefused;
use tauri::State;
use tauri::ipc::Channel;

use crate::session::AppState;

/// Once, at startup: the UI's channel for every `UiBatch`, in order.
#[tauri::command]
pub fn connect(events: Channel<UiBatch>, state: State<'_, AppState>) {
    state.sink.connect(events);
}

/// One `UiCommand`, fire and forget. Pointer and mic commands go to the core as
/// `Input::Command`. A rejection is counted by the UI and never retried.
#[tauri::command]
pub fn command(command: UiCommand, state: State<'_, AppState>) -> Result<(), String> {
    let slot = state
        .session
        .lock()
        .map_err(|_| "the session is unavailable".to_owned())?;
    let session = slot.as_ref().ok_or("the session has not started yet")?;
    session.command(command).map_err(|refused| match refused {
        InboxRefused::Full => "the session is busy; the command was dropped".to_owned(),
        InboxRefused::Closed => "the session has ended".to_owned(),
    })
}
