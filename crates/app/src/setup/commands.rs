//! The first-run screen's and the settings page's commands (D-100, D-153,
//! `ui/src/setup/tauriSetup.ts`). Each returns the fresh [`SetupStatus`] (or nothing) and errors
//! in plain words for the card that asked. File, Keychain, process and permission work runs on
//! the blocking pool, never on the main thread.

use std::sync::Arc;
use std::time::Duration;

use boardai_core::setup::{
    DownloadEvent, IndexRebuilt, KeyName, ModelListing, ModelQuery, OllamaStatus, SetupEdit,
    SetupStatus,
};
use boardai_runtime::audio::device_snapshot;
use boardai_runtime::harness::discover_from_environment;
use boardai_runtime::llm::catalog::CatalogError;
use boardai_runtime::llm::ollama::ollama_models;
use boardai_runtime::llm::openai::OLLAMA_BASE_URL;
use boardai_runtime::models::ModelStore;
use boardai_runtime::store::{ScanReport, StoreError};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use super::folders::{self, Chosen};
use super::status::{self, Inputs, Probes, Seen};
use super::{
    Known, SetupState, add_code_folder, downloads, harnesses, input, keys, mark_complete, paths,
    remove_key, save_edit, save_key,
};
use crate::permission;
use crate::secrets::KeychainSecrets;
use crate::session::AppState;

/// How long "Rebuild the index" waits for the store. Past it the screen says the rebuild goes
/// on; the store finishes it (one transaction, D-116) whatever the screen does.
pub const INDEX_REBUILD_WAIT_MS: u64 = 60_000;

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
        SetupEdit::Voice { .. }
        | SetupEdit::Mic { .. }
        | SetupEdit::Input { .. }
        | SetupEdit::Scribe { .. }
        | SetupEdit::RemoveCodeFolder { .. } => last_ollama(&state).await,
    };
    let lock = Arc::clone(&state.settings);
    let seen = ollama.clone();
    let catalog = Arc::clone(&state.catalog);
    blocking(move || {
        let devices = Inputs::from_snapshot(&device_snapshot()).names;
        let known = Known {
            ollama: &seen,
            catalog: &catalog,
            devices: &devices,
        };
        save_edit(&paths, &lock, &edit, &KeychainSecrets, known)
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
    let ledger = Arc::clone(&state.keys);
    blocking(move || save_key(&KeychainSecrets, &ledger, name, value)).await?;
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

/// The model picker's lists (D-153): OpenRouter's tool-capable models, one OpenRouter model's
/// providers, or the local Ollama's models. Bounded by `MODEL_LIST_TIMEOUT_MS` (Ollama's by
/// `OLLAMA_PROBE_TIMEOUT_MS`) and kept a while, so asking again is cheap. Needs no key.
#[tauri::command]
pub async fn setup_models(
    query: ModelQuery,
    state: State<'_, SetupState>,
) -> Result<ModelListing, String> {
    state.catalog.answer(&query).await.map_err(|error| {
        tracing::info!(?error, ?query, "a model list was not answered");
        model_list_problem(&query, &error).to_owned()
    })
}

/// Why a model list wasn't answered, in plain words for the thinking section.
pub fn model_list_problem(query: &ModelQuery, error: &CatalogError) -> &'static str {
    match (query, error) {
        (_, CatalogError::NotAModelId) => "That is not an OpenRouter model id.",
        (ModelQuery::Ollama, CatalogError::Provider(_)) => {
            "Ollama isn't answering on this Mac. Start it and try again."
        }
        (ModelQuery::OpenRouter | ModelQuery::Providers { .. }, CatalogError::Provider(_)) => {
            "OpenRouter's list didn't arrive. Check the connection and try again."
        }
    }
}

/// "Remove" on the keys section: deletes a key from the Keychain (D-172). Removing ElevenLabs'
/// while it is the voice puts Kokoro back.
#[tauri::command]
pub async fn setup_remove_key(
    name: KeyName,
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    let lock = Arc::clone(&state.settings);
    let ledger = Arc::clone(&state.keys);
    blocking(move || remove_key(&paths, &lock, &KeychainSecrets, &ledger, name)).await?;
    state.mark_changed();
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// "Check keys": reads the Keychain for every key now, on the owner's request, even while a
/// session runs (an unsigned build may prompt once per key, D-102).
#[tauri::command]
pub async fn setup_check_keys(
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let ledger = Arc::clone(&state.keys);
    blocking(move || {
        for name in keys::KEY_NAMES {
            keys::read(&KeychainSecrets, &ledger, name);
        }
        Ok(())
    })
    .await?;
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// The coding section asks each installed harness for its version once, within
/// `HARNESS_VERSION_TIMEOUT_MS`; the answers are kept for the run.
#[tauri::command]
pub async fn setup_harness_versions(
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let versions = Arc::clone(&state.harness_versions);
    blocking(move || {
        harnesses::ask_versions(&discover_from_environment(), &versions);
        Ok(())
    })
    .await?;
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// "Add a folder": the macOS folder chooser, shown by the app, so no path comes from the UI
/// (D-173). Closing it changes nothing.
#[tauri::command]
pub async fn setup_add_code_folder(
    app: AppHandle,
    state: State<'_, SetupState>,
) -> Result<SetupStatus, String> {
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    let lock = Arc::clone(&state.settings);
    let added = blocking(move || match folders::choose_folder()? {
        Chosen::Folder(folder) => add_code_folder(&paths, &lock, &folder).map(|()| true),
        Chosen::Cancelled => Ok(false),
    })
    .await?;
    if added {
        state.mark_changed();
    }
    let ollama = last_ollama(&state).await;
    current_status(&app, &state, ollama).await
}

/// "Open the vault": the configured vault in Finder.
#[tauri::command]
pub async fn setup_open_vault(app: AppHandle) -> Result<(), String> {
    let paths = paths(&app).ok_or(NO_FOLDERS)?;
    blocking(move || {
        let (settings, _) = status::load(&paths);
        folders::open_vault(&settings.vault)
    })
    .await
}

/// "Rebuild the index": the store empties the index and fills it again from the vault (the
/// vault is the source of truth, so nothing is lost), within `INDEX_REBUILD_WAIT_MS`.
#[tauri::command]
pub async fn setup_rebuild_index(app: AppHandle) -> Result<IndexRebuilt, String> {
    let store = app
        .state::<AppState>()
        .store
        .lock()
        .ok()
        .and_then(|open| open.as_ref().map(|open| open.store.clone()))
        .ok_or("Memory is off for this run, so there is no index to rebuild.")?;
    let wait = Duration::from_millis(INDEX_REBUILD_WAIT_MS);
    let rebuilt = tokio::time::timeout(wait, store.rebuild_index()).await;
    index_rebuild_answer(rebuilt.ok())
}

/// The rebuild's answer for the memory section: its counts, or why there are none (`None`: it
/// ran past the wait).
pub fn index_rebuild_answer(
    rebuilt: Option<Result<ScanReport, StoreError>>,
) -> Result<IndexRebuilt, String> {
    match rebuilt {
        Some(Ok(report)) => Ok(IndexRebuilt {
            files_seen: report.files_seen,
            reindexed: report.reindexed,
            skipped: report.skipped,
            truncated: report.truncated,
        }),
        Some(Err(error)) => {
            tracing::warn!(%error, "the index could not be rebuilt");
            Err("The index couldn't be rebuilt; the old one is kept. Try again.".to_owned())
        }
        None => Err("Still rebuilding. It finishes on its own; check again later.".to_owned()),
    }
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
/// way quitting ends one (D-103).
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
    let ledger = Arc::clone(&state.keys);
    let versions = Arc::clone(&state.harness_versions);
    let started = state.started();
    blocking(move || {
        let probes = Probes {
            ollama,
            microphone: permission::status(),
            harnesses: harnesses::installed(&discover_from_environment(), &versions),
            inputs: Inputs::from_snapshot(&device_snapshot()),
            started,
            mic_attributed_to: permission::attributed_to(),
        };
        let models = ModelStore::new(paths.models.clone()).map_err(|error| error.to_string())?;
        let seen = Seen {
            downloads: &downloads,
            keys: &ledger,
        };
        Ok(status::gather(
            &paths,
            &KeychainSecrets,
            &models,
            seen,
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
