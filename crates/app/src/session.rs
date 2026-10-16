//! Starting and closing the conversation session on Tauri's async runtime.
//!
//! At startup: `settings.toml` from the app's config folder (defaults when it is absent), the
//! store over the vault (`~/BoardAI` by default) with its index in the app's data folder, the
//! talker and the scribe from those settings with their keys from the Keychain, the repo tools
//! and harness for the configured code roots, the virtual voice until real speech lands, and no
//! mic yet. A talker that can't be built still starts the session: every question then gets
//! the spoken "No talker model is set up yet." (LLD §6). A vault that can't be used still
//! starts it too, with memory kept for this run only.
//!
//! Quitting closes the session (its digest within `SESSION_QUIT_DIGEST_MS`), then the store
//! (within `STORE_SHUTDOWN_TIMEOUT_MS`), so quitting never hangs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::{CoreConfig, MicMode};
use boardai_runtime::clock::{MonotonicClock, TokioClock};
use boardai_runtime::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, CodingHarness, HarnessPool, HarnessTimeouts, PiConfig,
    PiHarness, discover_from_environment,
};
use boardai_runtime::limits::STORE_SHUTDOWN_TIMEOUT_MS;
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::session::{
    MemoryStore, SessionChannels, SessionHandle, SessionParts, StoreSink, UnconfiguredTalker,
    VaultSink, VirtualVoice, VirtualVoiceConfig, scribe_from_settings, talker_from_settings,
    talker_name,
};
use boardai_runtime::settings::{self, HarnessChoice, Settings};
use boardai_runtime::store::{Store, StoreConfig, StoreTask, SystemWallClock};
use boardai_runtime::tools::{RepoRoots, RepoTools};
use tauri::{AppHandle, Manager};

use crate::secrets::KeychainSecrets;
use crate::sink::TauriSink;

/// The index file in the app's data folder; the vault is the source of truth (D-012).
const INDEX_FILE: &str = "index.sqlite";

/// What the commands reach: the UI sink, the session once started, and the store.
pub struct AppState {
    pub sink: Arc<TauriSink>,
    // Held only to read or swap the handle, never across an await.
    pub session: Mutex<Option<SessionHandle>>,
    /// The store while it is open. Replays read session logs through it; quitting stops it.
    // Held only to clone or take the handle, never across an await.
    pub store: Mutex<Option<OpenStore>>,
}

/// The store's handle and its actor task.
pub struct OpenStore {
    pub store: Store,
    task: StoreTask,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            sink: Arc::new(TauriSink::new()),
            session: Mutex::new(None),
            store: Mutex::new(None),
        }
    }
}

/// Starts the session in the background; the window opens meanwhile.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let settings = load_settings(&app);
        let opened = open_store(&app, &settings).await;
        let store: Arc<dyn StoreSink> = match &opened {
            Some(open) => {
                let scribe = scribe(&settings).await;
                Arc::new(VaultSink::new(
                    open.store.clone(),
                    scribe,
                    Arc::new(SystemWallClock),
                ))
            }
            None => Arc::new(MemoryStore::new()),
        };
        match state.store.lock() {
            Ok(mut slot) => *slot = opened,
            Err(_) => tracing::error!("the store slot is poisoned; replays are unavailable"),
        }
        let channels = SessionChannels::new();
        let (store, progress) = crate::dev::observe(store);
        let parts = parts(&settings, &channels, store, Arc::clone(&state.sink)).await;
        let session = boardai_runtime::session::spawn(channels, parts);
        let link = session.speech_in();
        match state.session.lock() {
            Ok(mut slot) => *slot = Some(session),
            Err(_) => tracing::error!("the session slot is poisoned; commands will be refused"),
        }
        tracing::info!("session started");
        crate::dev::run_script(&app, &state.sink, link, progress).await;
    });
}

/// Closes the session (talker, tasks, timers, voice; its digest within the quit budget), then
/// the store.
pub fn close(app: &AppHandle) {
    let state = app.state::<AppState>();
    let session = state.session.lock().ok().and_then(|mut slot| slot.take());
    if let Some(session) = session {
        let stats = tauri::async_runtime::block_on(session.close());
        tracing::info!(
            steps = stats.steps,
            sessions = stats.sessions,
            "session closed"
        );
    }
    let store = state.store.lock().ok().and_then(|mut slot| slot.take());
    if let Some(OpenStore { store, task }) = store {
        let bound = Duration::from_millis(STORE_SHUTDOWN_TIMEOUT_MS);
        let stopped = tauri::async_runtime::block_on(async move {
            tokio::time::timeout(bound, async move {
                let closed = store.shutdown().await;
                let joined = task.join().await;
                closed.and(joined)
            })
            .await
        });
        match stopped {
            Ok(Ok(())) => tracing::info!("store closed"),
            Ok(Err(error)) => tracing::warn!(%error, "the store did not close cleanly"),
            Err(_) => tracing::warn!("the store did not close in time"),
        }
    }
}

/// The store over the settings' vault, with its index in the app's data folder. `None` (memory
/// for this run only) when either can't be used; the reason is logged without quoting paths.
async fn open_store(app: &AppHandle, settings: &Settings) -> Option<OpenStore> {
    let index = match app.path().app_data_dir() {
        Ok(folder) => folder.join(INDEX_FILE),
        Err(error) => {
            tracing::error!(%error, "no app data folder for the index; memory is off");
            return None;
        }
    };
    match Store::open(StoreConfig::new(settings.vault.clone(), index)).await {
        Ok((store, task, opened)) => {
            tracing::info!(index = ?opened.index, scan = ?opened.scan, "store opened");
            Some(OpenStore { store, task })
        }
        Err(error) => {
            tracing::error!(%error, "the vault can't be used; memory is kept for this run only");
            None
        }
    }
}

async fn parts(
    settings: &Settings,
    channels: &SessionChannels,
    store: Arc<dyn StoreSink>,
    ui: Arc<TauriSink>,
) -> SessionParts {
    let talker = talker(settings).await;
    let opened = repo_tools(&settings.code_roots);
    let harness = harness(settings, opened.folders.first());
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::default());
    SessionParts {
        core: CoreConfig {
            talker_name: talker_name(&settings.talker),
            mic: match settings.mic {
                settings::MicMode::Open => MicMode::OpenMic,
                settings::MicMode::PushToTalk => MicMode::PushToTalk,
            },
            session_context: None,
            suggestion: None,
        },
        clock: TokioClock::start(),
        talker,
        tools: opened.tools,
        harness,
        speech: Box::new(voice),
        store,
        ui,
    }
}

fn load_settings(app: &AppHandle) -> Settings {
    let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let path = crate::dev::settings_override().or_else(|| {
        app.path()
            .app_config_dir()
            .ok()
            .map(|folder| folder.join("settings.toml"))
    });
    let Some(path) = path else {
        tracing::error!("no app config folder; using default settings");
        return Settings::defaults(&home);
    };
    match Settings::load(&path, &home) {
        Ok(settings) => settings,
        Err(error) => {
            // The error names the field and never quotes a value (D-025).
            tracing::error!(%error, "settings.toml is invalid; using the defaults");
            Settings::defaults(&home)
        }
    }
}

/// The configured talker, or one that says why there is none. The Keychain read may block.
async fn talker(settings: &Settings) -> Arc<dyn LlmProvider> {
    let llm = settings.talker.clone();
    let built = tauri::async_runtime::spawn_blocking(move || {
        let clock = Arc::new(MonotonicClock::start());
        talker_from_settings(&llm, &KeychainSecrets, clock)
    })
    .await;
    match built {
        Ok(Ok(talker)) => talker,
        Ok(Err(error)) => {
            tracing::warn!(%error, "no talker; questions get a spoken setup line");
            Arc::new(UnconfiguredTalker::new(error.to_string()))
        }
        Err(error) => {
            tracing::error!(%error, "building the talker failed");
            Arc::new(UnconfiguredTalker::new(
                "the talker could not be built".to_owned(),
            ))
        }
    }
}

/// The configured scribe; `None` gives every session the minimal digest. The Keychain read may
/// block.
async fn scribe(settings: &Settings) -> Option<Arc<dyn LlmProvider>> {
    let llm = settings.scribe.clone();
    let built = tauri::async_runtime::spawn_blocking(move || {
        let clock = Arc::new(MonotonicClock::start());
        scribe_from_settings(&llm, &KeychainSecrets, clock)
    })
    .await;
    match built {
        Ok(Ok(scribe)) => Some(scribe),
        Ok(Err(error)) => {
            tracing::warn!(%error, "no scribe; sessions get the minimal digest");
            None
        }
        Err(error) => {
            tracing::error!(%error, "building the scribe failed");
            None
        }
    }
}

/// The repo tools over the code roots that exist, and those folders.
struct Opened {
    tools: RepoTools,
    folders: Vec<PathBuf>,
}

fn repo_tools(code_roots: &[PathBuf]) -> Opened {
    let folders: Vec<PathBuf> = code_roots
        .iter()
        .filter(|root| root.is_dir())
        .cloned()
        .collect();
    match RepoRoots::open(&folders) {
        Ok(roots) => Opened {
            tools: RepoTools::new(roots),
            folders,
        },
        Err(error) => {
            tracing::warn!(detail = %error.message, "code roots could not be opened");
            Opened {
                tools: RepoTools::default(),
                folders: Vec::new(),
            }
        }
    }
}

/// A harness pool for the first code root, when a harness is installed.
fn harness(settings: &Settings, repo: Option<&PathBuf>) -> Option<HarnessPool> {
    let repo = repo?.clone();
    let found = discover_from_environment();
    let (claude, pi) = match settings.harness.kind {
        HarnessChoice::Auto => (found.claude_code, found.pi),
        HarnessChoice::ClaudeCode => (found.claude_code, None),
        HarnessChoice::Pi => (None, found.pi),
    };
    let environment = Vec::new();
    let harness: Arc<dyn CodingHarness> = match (claude, pi) {
        (Some(program), _) => Arc::new(ClaudeCodeHarness::new(ClaudeCodeConfig {
            program,
            repo,
            environment,
        })),
        (None, Some(program)) => Arc::new(PiHarness::new(PiConfig {
            program,
            repo,
            environment,
        })),
        (None, None) => return None,
    };
    Some(HarnessPool::new(harness, HarnessTimeouts::default()))
}
