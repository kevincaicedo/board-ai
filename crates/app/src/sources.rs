//! Sources from the window (D-184): files and folders dropped on it, through Tauri's native
//! drag-and-drop events (never a path from the webview), and the macOS chooser the Sources tray
//! asks for (the D-173 pattern: the app shows the system's chooser itself).
//!
//! Every path is checked here, where it enters (`runtime::sources::validate`), before the
//! session sees it. While something is dragged over the window, the session gets which half the
//! pointer is on (the left shares for this session, the right keeps in the project), only when
//! that changes; dropping shares what passed, and BoardAI says why when nothing did.

use std::path::PathBuf;
use std::sync::Mutex;

use boardai_core::sources::tray::ChooseWhat;
use boardai_core::sources::{DragOver, DropHalf};
use boardai_runtime::limits::DROP_PATHS_MAX;
use boardai_runtime::sources::{ValidSource, validate};
use tauri::{AppHandle, DragDropEvent, Manager, PhysicalPosition, Window};

use crate::session::AppState;
use crate::setup::folders::{CHOOSE_SOURCE_FILE, CHOOSE_SOURCE_FOLDER, Chosen, choose};

/// The drag in progress, kept between its events.
#[derive(Debug, Default)]
pub struct DragState {
    // Held only to read or swap, never across an await.
    current: Mutex<Option<DragOver>>,
}

/// One drag-and-drop event on the window.
pub fn on_drag_drop(window: &Window, event: &DragDropEvent) {
    let app = window.app_handle().clone();
    match event {
        DragDropEvent::Enter { paths, position } => {
            let half = half_of(window, *position);
            let paths = bounded(paths);
            tauri::async_runtime::spawn(async move {
                let checked = check_all(paths).await;
                let drag = DragOver {
                    title: title_of(&checked),
                    refused: refusal_of(&checked),
                    half,
                };
                remember(&app, Some(drag.clone()));
                send_drag(&app, Some(drag));
            });
        }
        DragDropEvent::Over { position } => {
            let half = half_of(window, *position);
            let state = app.state::<DragState>();
            let changed = state.current.lock().ok().and_then(|mut current| {
                let drag = current.as_mut()?;
                (drag.half != half).then(|| {
                    drag.half = half;
                    drag.clone()
                })
            });
            if let Some(drag) = changed {
                send_drag(&app, Some(drag));
            }
        }
        DragDropEvent::Drop { paths, position } => {
            let keep = half_of(window, *position) == DropHalf::Project;
            let paths = bounded(paths);
            remember(&app, None);
            tauri::async_runtime::spawn(async move {
                let checked = check_all(paths).await;
                send_drag(&app, None);
                share_checked(&app, checked, keep);
            });
        }
        DragDropEvent::Leave => {
            remember(&app, None);
            send_drag(&app, None);
        }
        // Tauri may add kinds of drag event; none of them shares anything.
        _ => {}
    }
}

/// The tray's "choose a folder" / "a file": the system's chooser, then the same checks as a
/// drop. Resolves once the chooser closed and what it picked went to the session.
#[tauri::command]
pub async fn sources_choose(what: ChooseWhat, keep: bool, app: AppHandle) -> Result<(), String> {
    let script = match what {
        ChooseWhat::Folder => CHOOSE_SOURCE_FOLDER,
        ChooseWhat::File => CHOOSE_SOURCE_FILE,
    };
    let chosen = tauri::async_runtime::spawn_blocking(move || choose(script))
        .await
        .map_err(|_| "The chooser stopped unexpectedly.".to_owned())??;
    let Chosen::Folder(path) = chosen else {
        return Ok(());
    };
    let checked = check_all(vec![path]).await;
    share_checked(&app, checked, keep);
    Ok(())
}

fn bounded(paths: &[PathBuf]) -> Vec<PathBuf> {
    if paths.len() > DROP_PATHS_MAX {
        tracing::warn!(
            dropped = paths.len() - DROP_PATHS_MAX,
            "a drop carried more paths than BoardAI looks at"
        );
    }
    paths.iter().take(DROP_PATHS_MAX).cloned().collect()
}

/// The left half shares for this session, the right keeps in the project (14b).
fn half_of(window: &Window, position: PhysicalPosition<f64>) -> DropHalf {
    let width = window
        .inner_size()
        .map_or(0.0, |size| f64::from(size.width));
    if position.x >= width / 2.0 {
        DropHalf::Project
    } else {
        DropHalf::Session
    }
}

/// Each path, checked on the blocking pool (a folder's files are counted).
async fn check_all(paths: Vec<PathBuf>) -> Vec<(PathBuf, Result<ValidSource, &'static str>)> {
    let home = home_folder();
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                let checked = validate(&path, &home).map_err(|refusal| refusal.words());
                (path, checked)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn home_folder() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn title_of(checked: &[(PathBuf, Result<ValidSource, &'static str>)]) -> String {
    match checked {
        [(path, _)] => path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        many => format!("{} items", many.len()),
    }
}

/// Why nothing dragged can be shared; `None` when something can.
fn refusal_of(checked: &[(PathBuf, Result<ValidSource, &'static str>)]) -> Option<String> {
    if checked.iter().any(|(_, valid)| valid.is_ok()) {
        return None;
    }
    let words = checked
        .iter()
        .find_map(|(_, valid)| valid.as_ref().err().copied())
        .unwrap_or("nothing was dragged.");
    Some(format!("Can't share that: {words}"))
}

fn remember(app: &AppHandle, drag: Option<DragOver>) {
    if let Ok(mut current) = app.state::<DragState>().current.lock() {
        *current = drag;
    }
}

fn send_drag(app: &AppHandle, drag: Option<DragOver>) {
    with_session(app, |session| session.drag(drag).is_ok());
}

/// Shares what passed; when nothing did, BoardAI says why.
fn share_checked(
    app: &AppHandle,
    checked: Vec<(PathBuf, Result<ValidSource, &'static str>)>,
    keep: bool,
) {
    let mut refused = None;
    let mut shared = 0_usize;
    for (_, valid) in checked {
        match valid {
            Ok(valid) => {
                if with_session(app, |session| session.share(valid, keep).is_ok()) {
                    shared += 1;
                }
            }
            Err(words) => refused = refused.or(Some(words)),
        }
    }
    if shared == 0
        && let Some(words) = refused
    {
        with_session(app, |session| session.refuse(words.to_owned()).is_ok());
    }
}

/// Runs `send` with the session, briefly holding its slot; false without a session.
fn with_session(
    app: &AppHandle,
    send: impl FnOnce(&boardai_runtime::session::SessionHandle) -> bool,
) -> bool {
    let state = app.state::<AppState>();
    let Ok(slot) = state.session.lock() else {
        return false;
    };
    let Some(session) = slot.as_ref() else {
        return false;
    };
    let sent = send(session);
    if !sent {
        tracing::warn!("the session didn't take a source event: it is busy or closed");
    }
    sent
}
