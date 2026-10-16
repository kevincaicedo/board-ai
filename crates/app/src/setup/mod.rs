//! The first-run screen's side of the shell (LLD §3.8, artboard 11 · First run): on a first
//! launch the session waits, the screen reads a [`SetupStatus`], saves choices to
//! `settings.toml`, keys to the Keychain, downloads the local models, asks for the
//! microphone, and "Start talking" starts the session.
//!
//! Keys go from the command straight into the `SecretStore`; they are never written to a file,
//! logged, or sent back to the UI, which only learns which names have a key (law 10).

pub mod commands;
mod downloads;
mod input;
mod status;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use boardai_core::setup::{KeyName, OllamaStatus, SetupEdit};
use boardai_runtime::settings::{SecretStore, Settings, SettingsEdit};
use tauri::{AppHandle, Manager};

pub use downloads::Downloads;
pub use status::SetupPaths;

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

/// Saves one edit to `settings.toml`. `devices` are the input devices macOS lists now. Blocks
/// (file I/O, maybe the Keychain).
pub fn save_edit(
    paths: &SetupPaths,
    lock: &Mutex<()>,
    edit: &SetupEdit,
    secrets: &dyn SecretStore,
    ollama: &OllamaStatus,
    devices: &[String],
) -> Result<(), String> {
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    let current =
        Settings::load(&paths.settings, &paths.home).map_err(|error| error.to_string())?;
    let elevenlabs_key = input::secret_name(KeyName::ElevenLabs)
        .is_some_and(|name| secrets.contains(&name).unwrap_or(false));
    let known = input::EditContext {
        current: &current,
        ollama,
        elevenlabs_key,
        devices,
    };
    let change = input::settings_edit(edit, known)?;
    Settings::edit(&paths.settings, &paths.home, &[change]).map_err(|error| error.to_string())?;
    Ok(())
}

/// Saves a pasted key to the store under its name. The value is parsed here and goes nowhere
/// else. Blocks (the Keychain).
pub fn save_key(secrets: &dyn SecretStore, key: KeyName, value: String) -> Result<(), String> {
    let secret = input::parse_key(value).map_err(|refused| refused.words().to_owned())?;
    let name = input::secret_name(key).ok_or("that key has no name")?;
    secrets.set(&name, &secret).map_err(|error| {
        tracing::warn!(%error, "a key could not be saved");
        "The Keychain didn't save the key. Unlock it and try again.".to_owned()
    })
}

/// Sets the setup-complete flag. Blocks (file I/O).
pub fn mark_complete(paths: &SetupPaths, lock: &Mutex<()>) -> Result<(), String> {
    let _held = lock.lock().map_err(|_| "settings are busy".to_owned())?;
    Settings::edit(&paths.settings, &paths.home, &[SettingsEdit::SetupComplete])
        .map(|_| ())
        .map_err(|error| error.to_string())
}
