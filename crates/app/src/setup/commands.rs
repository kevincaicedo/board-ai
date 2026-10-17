//! The first-run screen's commands (D-100, `ui/src/setup/tauriSetup.ts`). Each returns the
//! fresh [`SetupStatus`] (or nothing) and errors in plain words for the card that asked. File,
//! Keychain and permission work runs on the blocking pool, never on the main thread.

use std::sync::Arc;

use boardai_core::setup::{
    DownloadEvent, HarnessesFound, KeyName, OllamaStatus, SetupEdit, SetupStatus,
};
use boardai_runtime::audio::input_device_names;
use boardai_runtime::harness::discover_from_environment;
use boardai_runtime::llm::ollama::ollama_models;
use boardai_runtime::llm::openai::OLLAMA_BASE_URL;
use boardai_runtime::models::ModelStore;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use super::status::{self, Probes};
use super::{SetupState, downloads, input, mark_complete, paths, save_edit, save_key};
use crate::permission;
use crate::secrets::KeychainSecrets;

const NO_FOLDERS: &str = "BoardAI can't find its settings folder.";

/// The screen's status, with a fresh look at Ollama.
#[tauri::command]
pub async fn setup_status(
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let ollama = probe_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// Saves one choice to `settings.toml`. Picking a talker looks at Ollama again first.
#[tauri::command]
pub async fn setup_save(
    edit: SetupEdit,
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    let ollama = match edit {
        SetupEdit::Talker { .. } => probe_ollama(&state).await,
        SetupEdit::Voice { .. } | SetupEdit::Mic { .. } | SetupEdit::Input { .. } => {
            last_ollama(&state).await
        }
    };
    let lock = Arc::clone(&state.settings);
    let seen = ollama.clone();
    blocking(move || {
        let devices = input_device_names();
        save_edit(&paths, &lock, &edit, &KeychainSecrets, &seen, &devices)
    })
    .await?;
    state.mark_changed();
    current_status(&app, &state, ollama).await
}

/// Saves a pasted key to the Keychain. The UI gets back only that a key with this name exists.
#[tauri::command]
pub async fn setup_key(
    name: KeyName,
    value: String,
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    blocking(move || save_key(&KeychainSecrets, name, value)).await?;
    state.mark_changed();
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// Downloads (or resumes) one model file, with progress on `progress`. Resolves when the file
/// is verified, or fails. A file already downloading (for a screen before a reload) resolves at
/// once with no event: the status shows it as `Downloading` until it is done.
#[tauri::command]
pub async fn setup_download(
    model: String,
    progress: Channel<DownloadEvent>,
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<(), String> {
    let id = input::parse_model(&model)?;
    let Some(_running) = state.downloads.begin(id) else {
        return Ok(());
    };
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    let store = ModelStore::new(paths.models).map_err(|error| error.to_string())?;
    let send = move |event: DownloadEvent| {
        // A closed channel means the screen went away; the download still finishes.
        let _ = progress.send(event);
    };
    downloads::download(&store, id, &send)
        .await
        .map_err(|_| "The download stopped.".to_owned())
}

/// Asks macOS for the microphone (its prompt shows only the first time).
#[tauri::command]
pub async fn setup_microphone(
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let answer = permission::request().await;
    tracing::info!(?answer, "microphone permission asked");
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// "Open Microphone settings": System Settings at Privacy & Security › Microphone, where the
/// app macOS asks about (BoardAI, or the one it was started from) is turned on (D-149).
#[tauri::command]
pub async fn setup_open_microphone_settings() -> Result<(), String> {
    blocking(permission::open_microphone_settings).await
}

/// "Start talking": refused while `core::setup::blockers` names anything; otherwise sets the
/// setup-complete flag and starts the session (once). Opened as settings over a running
/// session, it closes the screen, and when something was saved it restarts the session the
/// way quitting ends one (D-107).
#[tauri::command]
pub async fn setup_finish(app: AppHandle, state: State<'_, SetupState>) -> Result<(), String> {
    if state.started() && !state.take_changed() {
        return Ok(());
    }
    let ollama = probe_ollama(&state).await;
    let checked = current_status(&app, &state, ollama).await?;
    if !checked.blockers.is_empty() {
        state.mark_changed();
        return Err("Not ready to start yet.".to_owned());
    }
    if state.started() {
        crate::session::restart(&app).await;
        return Ok(());
    }
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    let lock = Arc::clone(&state.settings);
    blocking(move || mark_complete(&paths, &lock)).await?;
    if state.claim_start() {
        tracing::info!("setup finished; the session starts");
        crate::session::start(&app);
    }
    Ok(())
}

async fn current_status(
    app: &AppHandle,
    state: &SetupState,
    ollama: OllamaStatus,
) -> Result<SetupStatus, String> {
    let paths = paths(app).ok_or(NO_FOLDERS)?;
    let downloads = state.downloads.clone();
    let started = state.started();
    blocking(move || {
        let found = discover_from_environment();
        let probes = Probes {
            ollama,
            microphone: permission::status(),
            harnesses: HarnessesFound {
                claude_code: found.claude_code.is_some(),
                pi: found.pi.is_some(),
            },
            input_devices: input_device_names(),
            started,
            mic_attributed_to: permission::attributed_to(),
        };
        let models = ModelStore::new(paths.models.clone()).map_err(|error| error.to_string())?;
        Ok(status::gather(
            &paths,
            &KeychainSecrets,
            &models,
            &downloads,
            probes,
        ))
    })
    .await
}

/// Asks Ollama for its models (bounded by `OLLAMA_PROBE_TIMEOUT_MS`) and remembers the answer.
async fn probe_ollama(state: &SetupState) -> OllamaStatus {
    let found = match ollama_models(OLLAMA_BASE_URL).await {
        Ok(models) => OllamaStatus::Reachable { models },
        Err(error) => {
            tracing::debug!(%error, "Ollama did not answer");
            OllamaStatus::Unreachable
        }
    };
    match state.ollama.lock() {
        Ok(mut last) => *last = Some(found.clone()),
        Err(poisoned) => *poisoned.into_inner() = Some(found.clone()),
    }
    found
}

/// The last Ollama answer, or a fresh one when there is none.
async fn last_ollama(state: &SetupState) -> OllamaStatus {
    let last = match state.ollama.lock() {
        Ok(last) => last.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    match last {
        Some(found) => found,
        None => probe_ollama(state).await,
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
