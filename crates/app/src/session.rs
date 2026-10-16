//! Starting and closing the conversation session on Tauri's async runtime.
//!
//! At startup: `settings.toml` from the app's config folder (defaults when it is absent), the
//! talker from those settings with its key from the Keychain, the repo tools and harness for
//! the configured code roots, the virtual voice until real speech lands, and no mic yet. A
//! talker that can't be built still starts the session: every question then gets the spoken
//! "No talker model is set up yet." (LLD §6).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use boardai_core::{CoreConfig, MicMode};
use boardai_runtime::clock::{MonotonicClock, TokioClock};
use boardai_runtime::harness::{
    ClaudeCodeConfig, ClaudeCodeHarness, CodingHarness, HarnessPool, HarnessTimeouts, PiConfig,
    PiHarness, discover_from_environment,
};
use boardai_runtime::llm::LlmProvider;
use boardai_runtime::session::{
    MemoryStore, SessionChannels, SessionHandle, SessionParts, StoreSink, UnconfiguredTalker,
    VirtualVoice, VirtualVoiceConfig, talker_from_settings, talker_name,
};
use boardai_runtime::settings::{self, HarnessChoice, Settings};
use boardai_runtime::tools::{RepoRoots, RepoTools};
use tauri::{AppHandle, Manager};

use crate::secrets::KeychainSecrets;
use crate::sink::TauriSink;

/// What the commands reach: the UI sink and, once started, the session.
pub struct AppState {
    pub sink: Arc<TauriSink>,
    // Held only to read or swap the handle, never across an await.
    pub session: Mutex<Option<SessionHandle>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            sink: Arc::new(TauriSink::new()),
            session: Mutex::new(None),
        }
    }
}

/// Starts the session in the background; the window opens meanwhile.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let channels = SessionChannels::new();
        let (store, progress) = crate::dev::observe(MemoryStore::new());
        let parts = parts(&app, &channels, store, Arc::clone(&state.sink)).await;
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

/// Closes the session (talker, tasks, timers, voice) and flushes the store.
pub fn close(app: &AppHandle) {
    let state = app.state::<AppState>();
    let session = state.session.lock().ok().and_then(|mut slot| slot.take());
    if let Some(session) = session {
        let stats = tauri::async_runtime::block_on(session.close());
        tracing::info!(steps = stats.steps, "session closed");
    }
}

async fn parts(
    app: &AppHandle,
    channels: &SessionChannels,
    store: Arc<dyn StoreSink>,
    ui: Arc<TauriSink>,
) -> SessionParts {
    let settings = load_settings(app);
    let talker = talker(&settings).await;
    let opened = repo_tools(&settings.code_roots);
    let harness = harness(&settings, opened.folders.first());
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
