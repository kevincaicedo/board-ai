//! The first-run screen's and the settings page's side of the shell (LLD §3.8, artboards 11 ·
//! First run and 12 · Settings): on a first launch the session waits, the screen reads a
//! [`SetupStatus`], saves choices to `settings.toml`, keys to the Keychain, downloads the local
//! models, asks for the microphone, and "Start talking" starts the session. Over a running
//! session the settings page does the same, and also removes keys, adds code folders through
//! the system's chooser, opens the vault and rebuilds the index.
//!
//! Keys go from the command straight into the `SecretStore`; they are never written to a file,
//! logged, or sent back to the UI, which only learns which names have a key (law 10).

pub mod commands;
mod downloads;
mod folders;
mod harnesses;
mod input;
mod keys;
mod status;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use boardai_core::setup::{KeyName, KeyState, OllamaStatus, SetupEdit, TalkerProvider};
use boardai_runtime::llm::catalog::{ModelCatalog, OpenRouterModelId};
use boardai_runtime::settings::{SecretStore, Settings, SettingsEdit, TtsKind};
use tauri::{AppHandle, Manager};

pub use downloads::Downloads;
pub use status::SetupPaths;

use harnesses::HarnessVersions;
use keys::KeyLedger;

/// What the setup commands share.
#[derive(Default)]
pub struct SetupState {
    /// Serializes read-modify-write of `settings.toml`. Locked only inside a blocking task,
    /// never across an await.
    settings: Arc<Mutex<()>>,
    downloads: Downloads,
    /// The last Ollama answer, so a save doesn't wait on the probe again.
    // Held only to read or replace the value.
    ollama: Mutex<Option<OllamaStatus>>,
    /// The model picker's lists, fetched on demand and kept a while (D-153).
    catalog: Arc<ModelCatalog>,
    /// Which keys the Keychain held when this run last looked (names only, D-102).
    keys: Arc<KeyLedger>,
    /// What each harness's `--version` answered this run.
    harness_versions: Arc<HarnessVersions>,
    /// The session was started (at launch, or by finishing setup); it starts once.
    started: AtomicBool,
    /// A choice or a key was saved while the session runs (the screen opened as settings):
    /// finishing restarts the session so it takes effect.
    changed: AtomicBool,
}

impl SetupState {
    pub fn new() -> Self {
        Self::default()
    }

    fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Marks the session started; `true` only for the first caller.
    fn claim_start(&self) -> bool {
        !self.started.swap(true, Ordering::SeqCst)
    }

    /// Something was saved; it matters only once the session runs.
    fn mark_changed(&self) {
        if self.started() {
            self.changed.store(true, Ordering::SeqCst);
        }
    }

    /// Whether something was saved since the last restart, clearing it.
    fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }
}

/// At launch: start the session when setup was finished, else leave the window to the
/// first-run screen. Debug builds running a dev script skip setup (`dev::skips_setup`).
pub fn launch(app: &AppHandle) {
    let complete = paths(app).is_some_and(|paths| status::load(&paths).0.setup_complete);
    let state = app.state::<SetupState>();
    if complete || crate::dev::skips_setup() {
        if state.claim_start() {
            crate::session::start(app);
        }
    } else {
        tracing::info!("first run: the session starts after setup");
    }
}

/// Where setup reads and writes, from the app's folders.
pub fn paths(app: &AppHandle) -> Option<SetupPaths> {
    let home = app.path().home_dir().ok()?;
    let settings = crate::session::settings_path(app)?;
    Some(SetupPaths {
        models: status::models_root(&home),
        settings,
        home,
    })
}

/// What the app has seen that a save is checked against.
#[derive(Clone, Copy)]
pub struct Known<'a> {
    pub ollama: &'a OllamaStatus,
    /// The model picker's lists: a picked OpenRouter model's thinking comes from here.
    pub catalog: &'a ModelCatalog,
    /// The input devices macOS lists now.
    pub devices: &'a [String],
}

/// Saves one edit to `settings.toml`. Blocks (file I/O, maybe the Keychain).
pub fn save_edit(
    paths: &SetupPaths,
    lock: &Mutex<()>,
    edit: &SetupEdit,
    secrets: &dyn SecretStore,
    known: Known<'_>,
) -> Result<(), String> {
    let Known {
        ollama,
        catalog,
        devices,
    } = known;
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    let current =
        Settings::load(&paths.settings, &paths.home).map_err(|error| error.to_string())?;
    let elevenlabs_key = input::secret_name(KeyName::ElevenLabs)
        .is_some_and(|name| secrets.contains(&name).unwrap_or(false));
    let open_router_model = match edit {
        SetupEdit::Talker {
            provider: TalkerProvider::OpenRouter,
            model: Some(model),
        }
        | SetupEdit::Scribe { model } => {
            OpenRouterModelId::parse(model).and_then(|id| catalog.listed(&id))
        }
        SetupEdit::Talker { .. }
        | SetupEdit::Voice { .. }
        | SetupEdit::Mic { .. }
        | SetupEdit::Input { .. }
        | SetupEdit::RemoveCodeFolder { .. } => None,
    };
    let known = input::EditContext {
        current: &current,
        ollama,
        open_router_model: open_router_model.as_ref(),
        elevenlabs_key,
        devices,
    };
    let change = input::settings_edit(edit, known)?;
    Settings::edit(&paths.settings, &paths.home, &[change]).map_err(|error| error.to_string())?;
    Ok(())
}

/// Saves a pasted key to the store under its name. The value is parsed here and goes nowhere
/// else. Blocks (the Keychain).
pub fn save_key(
    secrets: &dyn SecretStore,
    ledger: &KeyLedger,
    key: KeyName,
    value: String,
) -> Result<(), String> {
    let secret = input::parse_key(value).map_err(|refused| refused.words().to_owned())?;
    let name = input::secret_name(key).ok_or("that key has no name")?;
    secrets.set(&name, &secret).map_err(|error| {
        tracing::warn!(%error, "a key could not be saved");
        "The Keychain didn't save the key. Unlock it and try again.".to_owned()
    })?;
    ledger.record(key, KeyState::Saved);
    Ok(())
}

/// Removes a key from the store. When it is ElevenLabs' and ElevenLabs is the voice, the voice
/// goes back to Kokoro, so the next session still speaks. Blocks (the Keychain, file I/O).
pub fn remove_key(
    paths: &SetupPaths,
    lock: &Mutex<()>,
    secrets: &dyn SecretStore,
    ledger: &KeyLedger,
    key: KeyName,
) -> Result<(), String> {
    let name = input::secret_name(key).ok_or("that key has no name")?;
    secrets.remove(&name).map_err(|error| {
        tracing::warn!(%error, "a key could not be removed");
        "The Keychain didn't remove the key. Unlock it and try again.".to_owned()
    })?;
    ledger.record(key, KeyState::Missing);
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    let voice_needs_it = key == KeyName::ElevenLabs
        && Settings::load(&paths.settings, &paths.home)
            .is_ok_and(|settings| settings.tts.kind == TtsKind::ElevenLabs);
    if voice_needs_it {
        Settings::edit(
            &paths.settings,
            &paths.home,
            &[SettingsEdit::Tts(TtsKind::Kokoro)],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Adds a folder picked in the system's chooser to `code_roots`. Blocks (file I/O).
pub fn add_code_folder(paths: &SetupPaths, lock: &Mutex<()>, folder: &Path) -> Result<(), String> {
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    let current =
        Settings::load(&paths.settings, &paths.home).map_err(|error| error.to_string())?;
    let change = folders::add_folder(&current, folder, &paths.home)?;
    Settings::edit(&paths.settings, &paths.home, &[change]).map_err(|error| error.to_string())?;
    Ok(())
}

/// Sets the setup-complete flag. Blocks (file I/O).
pub fn mark_complete(paths: &SetupPaths, lock: &Mutex<()>) -> Result<(), String> {
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    Settings::edit(&paths.settings, &paths.home, &[SettingsEdit::SetupComplete])
        .map(|_| ())
        .map_err(|error| error.to_string())
}
