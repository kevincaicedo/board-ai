//! Dev hooks, compiled into debug builds only: a typed utterance without a mic, so the steel
//! thread can be exercised before speech in lands.
//!
//! - `dev_say(text)`: a command the UI's dev overlay can call; granted at runtime in debug
//!   builds, absent (no handler, no permission) in release.
//! - `BOARDAI_DEV_SCRIPT=<file>`: a script in the `converse` format, said once the UI connects.
//!   The session starts at launch without the first-run screen.
//!   With `BOARDAI_DEV_EXIT=1` the app quits when the script's last answer ends, through the
//!   normal exit path (the session closes and logs what the bridge delivered).
//! - `BOARDAI_SETTINGS=<file>`: read settings from this file instead of the config folder.

use std::path::PathBuf;
use std::sync::Arc;

use boardai_runtime::session::{Progress, SpeechInLink, StoreSink};
use tokio::sync::watch;

use crate::sink::TauriSink;

/// How long after the UI connects a dev script starts, so the first frames and fonts are in.
#[cfg(debug_assertions)]
const DEV_SCRIPT_START_DELAY_MS: u64 = 1_500;

/// The store, counting progress for a dev script in debug builds.
#[cfg(debug_assertions)]
pub fn observe<S: StoreSink + 'static>(
    store: S,
) -> (Arc<dyn StoreSink>, Option<watch::Receiver<Progress>>) {
    let (observed, progress) = boardai_runtime::session::Observed::new(store);
    (Arc::new(observed), Some(progress))
}

#[cfg(not(debug_assertions))]
pub fn observe<S: StoreSink + 'static>(
    store: S,
) -> (Arc<dyn StoreSink>, Option<watch::Receiver<Progress>>) {
    (Arc::new(store), None)
}

/// A dev script needs a session without anyone clicking through the first-run screen.
#[cfg(debug_assertions)]
pub fn skips_setup() -> bool {
    std::env::var_os("BOARDAI_DEV_SCRIPT").is_some()
}

#[cfg(not(debug_assertions))]
pub fn skips_setup() -> bool {
    false
}

#[cfg(debug_assertions)]
pub fn settings_override() -> Option<PathBuf> {
    std::env::var_os("BOARDAI_SETTINGS").map(PathBuf::from)
}

#[cfg(not(debug_assertions))]
pub fn settings_override() -> Option<PathBuf> {
    None
}

/// Says `BOARDAI_DEV_SCRIPT`'s lines once the UI is connected.
#[cfg(debug_assertions)]
pub async fn run_script(
    app: &tauri::AppHandle,
    sink: &TauriSink,
    link: SpeechInLink,
    progress: Option<watch::Receiver<Progress>>,
) {
    let (Some(path), Some(mut progress)) = (std::env::var_os("BOARDAI_DEV_SCRIPT"), progress)
    else {
        return;
    };
    let script = match std::fs::read_to_string(&path).map(|text| {
        boardai_runtime::session::Script::parse(&text).map_err(|error| error.to_string())
    }) {
        Ok(Ok(script)) => script,
        Ok(Err(error)) => return tracing::error!(%error, "BOARDAI_DEV_SCRIPT is invalid"),
        Err(error) => return tracing::error!(%error, "BOARDAI_DEV_SCRIPT can't be read"),
    };
    sink.wait_connected().await;
    tokio::time::sleep(std::time::Duration::from_millis(DEV_SCRIPT_START_DELAY_MS)).await;
    tracing::info!(lines = script.lines.len(), "dev script starts");
    if boardai_runtime::session::run_script(script, &link, &mut progress)
        .await
        .is_err()
    {
        tracing::warn!("the session ended during the dev script");
    }
    tracing::info!("dev script finished");
    if std::env::var_os("BOARDAI_DEV_EXIT").is_some() {
        app.exit(0);
    }
}

#[cfg(not(debug_assertions))]
pub async fn run_script(
    _app: &tauri::AppHandle,
    _sink: &TauriSink,
    _link: SpeechInLink,
    _progress: Option<watch::Receiver<Progress>>,
) {
}

/// Grants the main window `dev_say` (debug builds only).
#[cfg(debug_assertions)]
pub fn grant(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let capability = tauri::ipc::CapabilityBuilder::new("dev")
        .window("main")
        .permission("allow-dev-say");
    if let Err(error) = app.add_capability(capability) {
        tracing::warn!(%error, "dev_say could not be granted");
    }
}

#[cfg(not(debug_assertions))]
pub fn grant(_app: &tauri::AppHandle) {}

/// Says `text` as if spoken (debug builds only).
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_say(
    text: String,
    state: tauri::State<'_, crate::session::AppState>,
) -> Result<(), String> {
    let link = state
        .session
        .lock()
        .map_err(|_| "the session is unavailable".to_owned())?
        .as_ref()
        .map(boardai_runtime::session::SessionHandle::speech_in)
        .ok_or("the session has not started yet")?;
    boardai_runtime::session::say_typed(&link, &text)
        .await
        .map_err(|error| error.to_string())
}
