//! The commands of the UI contract (D-027, `ui/src/transport/tauri.ts`): the event channel, UI
//! commands, and a past session's log for a replay.

use boardai_core::protocol::{UiBatch, UiCommand};
use boardai_runtime::session::InboxRefused;
use boardai_runtime::store::SessionId;
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

/// A past session's `events.jsonl`, for a replay in the UI. Only by a session id the store
/// knows, parsed here at the boundary: never a path, and bounded by
/// `SESSION_LOG_READ_MAX_BYTES`.
#[tauri::command]
pub async fn session_log(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let id = SessionId::parse(&id).ok_or("that is not a session id")?;
    let store = state
        .store
        .lock()
        .map_err(|_| "the store is unavailable".to_owned())?
        .as_ref()
        .map(|open| open.store.clone())
        .ok_or("memory is off, so there are no past sessions")?;
    store
        .session_log(&id)
        .await
        .map_err(|error| error.to_string())
}
