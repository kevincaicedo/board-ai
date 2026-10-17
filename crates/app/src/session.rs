//! Starting and closing the conversation session on Tauri's async runtime.
//!
//! It starts once setup is done (`setup::launch`, or "Start talking" on the first-run screen):
//! `settings.toml` from the app's config folder (defaults when it is absent), the
//! store over the vault (`~/BoardAI` by default) with its index in the app's data folder, the
//! talker and the scribe from those settings with their keys from the Keychain, the repo tools
//! and harness for the configured code roots, and real audio (`crate::audio`: the voice, and
//! the microphone once macOS allows it; the virtual voice when audio cannot start). Audio
//! outlives the conversation sessions: every session after an idle rest speaks and listens
//! through the same voice and microphone. A talker that can't be built still starts the
//! session: every question then gets the spoken "No talker model is set up yet." (LLD §6). A
//! vault that can't be used still starts it too, with memory kept for this run only.
//!
//! Quitting closes the session (its digest within `SESSION_QUIT_DIGEST_MS`), then audio (speech
//! in's threads, then the unit), then the store (within `STORE_SHUTDOWN_TIMEOUT_MS`), so
//! quitting never hangs. Changing settings on the settings screen closes everything the same
//! way and starts again (`restart`).
//!
//! Starting, restarting and closing are one task's work, served in the order asked
//! (`crate::lifecycle`).

use std::future::Future;
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
use boardai_runtime::models::ModelStore;
use boardai_runtime::session::{
    MemoryStore, SessionChannels, SessionHandle, SessionParts, SpeechOut, StoreSink,
    UnconfiguredTalker, VaultSink, scribe_from_settings, talker_from_settings, talker_name,
};
use boardai_runtime::settings::{self, HarnessChoice, Settings};
use boardai_runtime::store::{Store, StoreConfig, StoreTask, SystemWallClock};
use boardai_runtime::tools::{RepoRoots, RepoTools, too_broad};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, oneshot};

use crate::audio::RunningAudio;
use crate::lifecycle::{LIFECYCLE_REQUESTS_MAX, Lifecycle, SessionLife};
use crate::secrets::KeychainSecrets;
use crate::sink::TauriSink;

/// The index file in the app's data folder; the vault is the source of truth (D-012).
const INDEX_FILE: &str = "index.sqlite";

/// How long quitting waits for the lifecycle: a start still in progress finishes first, then
/// the session closes (`SESSION_CLOSE_TIMEOUT_MS`), audio stops and the store closes
/// (`STORE_SHUTDOWN_TIMEOUT_MS`). Past it quitting goes on; the next launch gives an unfinished
/// session its minimal digest (D-080).
pub const QUIT_WAIT_MS: u64 = 30_000;

/// What the commands reach: the UI sink, the session once started, and the store.
pub struct AppState {
    pub sink: Arc<TauriSink>,
    // Held only to read or swap the handle, never across an await.
    pub session: Mutex<Option<SessionHandle>>,
    /// The store while it is open. Replays read session logs through it; quitting stops it.
    // Held only to clone or take the handle, never across an await.
    pub store: Mutex<Option<OpenStore>>,
    /// The audio unit and speech in, stopped after the session closes.
    pub audio: Mutex<Option<RunningAudio>>,
    /// Asks the lifecycle task to start, restart or close.
    lifecycle: mpsc::Sender<Lifecycle>,
    /// The task's end, taken once when it is spawned at setup.
    // Held only to take the receiver.
    lifecycle_inbox: Mutex<Option<mpsc::Receiver<Lifecycle>>>,
}

/// The store's handle and its actor task.
pub struct OpenStore {
    pub store: Store,
    task: StoreTask,
}

/// Whether the running session hears the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hearing {
    /// Speech in runs on the microphone.
    Listening,
    /// The audio unit runs, but speech in didn't start (no permission, models not downloaded):
    /// fixable on the settings page, after which a new session hears.
    Deaf,
    /// No session, or no audio unit (the virtual voice): settings can't fix it.
    NoAudio,
}

impl AppState {
    pub fn new() -> Self {
        let (lifecycle, inbox) = mpsc::channel(LIFECYCLE_REQUESTS_MAX);
        Self {
            sink: Arc::new(TauriSink::new()),
            session: Mutex::new(None),
            store: Mutex::new(None),
            audio: Mutex::new(None),
            lifecycle,
            lifecycle_inbox: Mutex::new(Some(inbox)),
        }
    }

    /// Whether the running session hears (`Hearing`).
    pub fn hearing(&self) -> Hearing {
        let Ok(audio) = self.audio.lock() else {
            return Hearing::NoAudio;
        };
        match audio.as_ref() {
            Some(running) if running.hears() => Hearing::Listening,
            Some(_) => Hearing::Deaf,
            None => Hearing::NoAudio,
        }
    }
}

/// Spawns the lifecycle task (once, at setup). It serves requests in order until the app's
/// state, and with it every sender, is dropped at exit.
pub fn serve_lifecycle(app: &AppHandle) {
    let inbox = app
        .state::<AppState>()
        .lifecycle_inbox
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let Some(inbox) = inbox else {
        tracing::error!("the session lifecycle was already served");
        return;
    };
    let life = AppLife { app: app.clone() };
    tauri::async_runtime::spawn(crate::lifecycle::serve(inbox, life));
}

/// The app's session as the lifecycle drives it.
struct AppLife {
    app: AppHandle,
}

impl SessionLife for AppLife {
    fn is_open(&self) -> bool {
        self.app
            .state::<AppState>()
            .session
            .lock()
            .is_ok_and(|slot| slot.is_some())
    }

    fn open(&self) -> impl Future<Output = ()> + Send {
        open_session(self.app.clone())
    }

    fn shut_down(&self) -> impl Future<Output = ()> + Send {
        shut_down(self.app.clone())
    }
}

/// Asks for the session at launch; the window opens meanwhile.
pub fn start_soon(app: &AppHandle) {
    let (opened, _answer) = oneshot::channel();
    if let Err(error) = app
        .state::<AppState>()
        .lifecycle
        .try_send(Lifecycle::Start { opened })
    {
        tracing::error!(%error, "the session could not be asked to start");
    }
}

/// Starts the session (once) and returns when it has read its settings and runs.
pub async fn start(app: &AppHandle) {
    let (opened, answer) = oneshot::channel();
    ask(app, Lifecycle::Start { opened }, answer).await;
}

/// Settings changed: ends the session as quitting does (its digest within the quit budget),
/// then starts a new one from the new `settings.toml`, and returns once it has read them. The
/// UI stays connected; the new session's `SessionStart` resets its stage.
pub async fn restart(app: &AppHandle) {
    let (opened, answer) = oneshot::channel();
    ask(app, Lifecycle::Restart { opened }, answer).await;
}

/// Quitting: closes the session, then audio, then the store, on the calling thread, within
/// [`QUIT_WAIT_MS`].
pub fn close(app: &AppHandle) {
    let (closed, answer) = oneshot::channel();
    let waited = tauri::async_runtime::block_on(tokio::time::timeout(
        Duration::from_millis(QUIT_WAIT_MS),
        ask(app, Lifecycle::Close { closed }, answer),
    ));
    if waited.is_err() {
        tracing::warn!("the session did not close within {QUIT_WAIT_MS} ms; quitting goes on");
    }
}

async fn ask(app: &AppHandle, request: Lifecycle, answer: oneshot::Receiver<()>) {
    let lifecycle = app.state::<AppState>().lifecycle.clone();
    if lifecycle.send(request).await.is_err() {
        tracing::error!("the session lifecycle has stopped");
        return;
    }
    // A dropped answer means the task stopped; either way the request is over.
    let _ = answer.await;
}

/// Opens the session: settings, the store, audio, the talker, the orchestrator. Only the
/// lifecycle task calls it.
async fn open_session(app: AppHandle) {
    let app = &app;
    let state = app.state::<AppState>();
    let settings = load_settings(app);
    let opened = open_store(app, &settings).await;
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
    let (speech, audio) = crate::audio::start(&settings, &channels, models_root(app)).await;
    if let Ok(mut slot) = state.audio.lock() {
        *slot = audio;
    }
    let parts = parts(&settings, speech, store, Arc::clone(&state.sink)).await;
    let session = boardai_runtime::session::spawn(channels, parts);
    let link = session.speech_in();
    match state.session.lock() {
        Ok(mut slot) => *slot = Some(session),
        Err(_) => tracing::error!("the session slot is poisoned; commands will be refused"),
    }
    tracing::info!("session started");
    let (app, sink) = (app.clone(), Arc::clone(&state.sink));
    tauri::async_runtime::spawn(async move {
        crate::dev::run_script(&app, &sink, link, progress).await;
    });
}

/// Closes the session (talker, tasks, timers, voice; its digest within the quit budget), then
/// audio, then the store. Every slot is emptied first, so nothing reaches a closing part.
async fn shut_down(app: AppHandle) {
    let state = app.state::<AppState>();
    let session = state.session.lock().ok().and_then(|mut slot| slot.take());
    if let Some(session) = session {
        let stats = session.close().await;
        tracing::info!(
            steps = stats.steps,
            sessions = stats.sessions,
            "session closed"
        );
    }
    let audio = state.audio.lock().ok().and_then(|mut slot| slot.take());
    if let Some(audio) = audio {
        audio.stop().await;
    }
    let store = state.store.lock().ok().and_then(|mut slot| slot.take());
    if let Some(OpenStore { store, task }) = store {
        let bound = Duration::from_millis(STORE_SHUTDOWN_TIMEOUT_MS);
        let stopped = tokio::time::timeout(bound, async move {
            let closed = store.shutdown().await;
            let joined = task.join().await;
            closed.and(joined)
        })
        .await;
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

/// `~/Library/Application Support/ai.boardai.desktop/models`, where `just models` puts them.
fn models_root(app: &AppHandle) -> PathBuf {
    let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("/"));
    ModelStore::default_root(&home)
}

async fn parts(
    settings: &Settings,
    speech: Box<dyn SpeechOut>,
    store: Arc<dyn StoreSink>,
    ui: Arc<TauriSink>,
) -> SessionParts {
    let talker = talker(settings).await;
    let opened = repo_tools(&settings.code_roots);
    let harness = harness(settings, opened.folders.first());
    SessionParts {
        core: CoreConfig {
            talker_name: talker_name(&settings.talker),
            mic: match settings.mic {
                settings::MicMode::Open => MicMode::OpenMic,
                settings::MicMode::PushToTalk => MicMode::PushToTalk,
            },
            session_context: None,
            suggestion: None,
            repo_reads: opened.tools.repo_reads(),
        },
        clock: TokioClock::start(),
        talker,
        tools: opened.tools,
        harness,
        speech,
        store,
        ui,
    }
}

/// `settings.toml` in the app's config folder (or the dev override); the first-run screen
/// writes the same file.
pub fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    crate::dev::settings_override().or_else(|| {
        app.path()
            .app_config_dir()
            .ok()
            .map(|folder| folder.join("settings.toml"))
    })
}

fn load_settings(app: &AppHandle) -> Settings {
    let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let Some(path) = settings_path(app) else {
        tracing::error!("no app config folder; using default settings");
        return Settings::defaults(&home);
    };
    let mut settings = match Settings::load(&path, &home) {
        Ok(settings) => settings,
        Err(error) => {
            // The error names the field and never quotes a value (D-025).
            tracing::error!(%error, "settings.toml is invalid; using the defaults");
            Settings::defaults(&home)
        }
    };
    // A hand-edited `code_roots` is held to the settings page's rule: the repo tools and the
    // vocabulary scan never open the disk, the home folder or `~/Library`.
    settings
        .code_roots
        .retain(|root| match too_broad(root, &home) {
            None => true,
            Some(why) => {
                tracing::warn!(?why, "a code folder is too broad to open; it is skipped");
                false
            }
        });
    settings
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
