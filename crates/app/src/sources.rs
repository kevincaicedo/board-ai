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
    // Held only to read or change the drag and send what it says (a `try_send`), never across
    // an await: sending under it keeps the session's drag events in the order the drag's own.
    current: Mutex<DragTrack>,
}

/// One drag at a time, from its enter to its drop or leave. Each drag has a generation, so a
/// check that answers after its drag ended, or after a newer one began, is dropped: the overlay
/// never sticks once the pointer has left (finding 24).
#[derive(Debug)]
struct DragTrack {
    generation: u64,
    /// Between an enter and its drop or leave.
    live: bool,
    /// The half under the pointer, kept while the enter's check runs.
    half: DropHalf,
    /// What the session was last told: the drag once its check answered.
    shown: Option<DragOver>,
}

impl Default for DragTrack {
    fn default() -> Self {
        Self {
            generation: 0,
            live: false,
            half: DropHalf::Session,
            shown: None,
        }
    }
}

impl DragTrack {
    /// A drag entered: a new generation, shown once its check answers.
    fn enter(&mut self, half: DropHalf) -> u64 {
        self.generation += 1;
        self.live = true;
        self.half = half;
        self.shown = None;
        self.generation
    }

    /// The enter's check answered: the drag to show, when it is still the drag in progress,
    /// over the half the pointer is on now.
    fn checked(
        &mut self,
        generation: u64,
        title: String,
        refused: Option<String>,
    ) -> Option<DragOver> {
        if !self.live || self.generation != generation {
            return None;
        }
        let drag = DragOver {
            title,
            refused,
            half: self.half,
        };
        self.shown = Some(drag.clone());
        Some(drag)
    }

    /// The pointer moved: the drag to show again when its half changed.
    fn moved(&mut self, half: DropHalf) -> Option<DragOver> {
        if !self.live || self.half == half {
            return None;
        }
        self.half = half;
        let drag = self.shown.as_mut()?;
        drag.half = half;
        Some(drag.clone())
    }

    /// Dropped or left: the drag ends; its generation, for a drop's check to end the overlay.
    fn end(&mut self) -> u64 {
        self.generation += 1;
        self.live = false;
        self.shown = None;
        self.generation
    }

    /// Whether the overlay is still the ended drag's: no drag began since `generation` ended.
    fn still_ended(&self, generation: u64) -> bool {
        !self.live && self.generation == generation
    }
}

impl DragState {
    /// Runs `change` on the drag, then sends what it says, under the drag's lock.
    fn update(
        &self,
        app: &AppHandle,
        change: impl FnOnce(&mut DragTrack) -> Option<Option<DragOver>>,
    ) {
        let Ok(mut track) = self.current.lock() else {
            return;
        };
        if let Some(drag) = change(&mut track) {
            send_drag(app, drag);
        }
    }
}

/// One drag-and-drop event on the window.
pub fn on_drag_drop(window: &Window, event: &DragDropEvent) {
    let app = window.app_handle().clone();
    let state = app.state::<DragState>();
    match event {
        DragDropEvent::Enter { paths, position } => {
            let half = half_of(window, *position);
            let mut generation = 0;
            state.update(&app, |track| {
                generation = track.enter(half);
                None
            });
            let paths = bounded(paths);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let checked = check_all(paths).await;
                let (title, refused) = (title_of(&checked), refusal_of(&checked));
                app.state::<DragState>().update(&app, |track| {
                    track.checked(generation, title, refused).map(Some)
                });
            });
        }
        DragDropEvent::Over { position } => {
            let half = half_of(window, *position);
            state.update(&app, |track| track.moved(half).map(Some));
        }
        DragDropEvent::Drop { paths, position } => {
            let keep = half_of(window, *position) == DropHalf::Project;
            let paths = bounded(paths);
            let mut generation = 0;
            state.update(&app, |track| {
                generation = track.end();
                None
            });
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let checked = check_all(paths).await;
                // The overlay stays while the drop is checked, unless a new drag took it over.
                app.state::<DragState>()
                    .update(&app, |track| track.still_ended(generation).then_some(None));
                share_checked(&app, checked, keep);
            });
        }
        DragDropEvent::Leave => state.update(&app, |track| {
            track.end();
            Some(None)
        }),
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

#[cfg(test)]
mod tests {
    use boardai_core::sources::DropHalf;

    use super::DragTrack;

    fn title() -> String {
        "hekaton.pdf".to_owned()
    }

    /// Finding 24: a check that answers after the pointer left is dropped, so the overlay it
    /// would show never sticks.
    #[test]
    fn a_check_that_answers_after_the_drag_left_shows_nothing() {
        let mut track = DragTrack::default();
        let entered = track.enter(DropHalf::Session);
        let _ = track.end();
        assert_eq!(track.checked(entered, title(), None), None);
        assert_eq!(track.moved(DropHalf::Project), None);
    }

    /// A drop's check ends the overlay only while no new drag has begun; the new drag's own
    /// check shows it, over the half the pointer moved to meanwhile.
    #[test]
    fn a_new_drag_owns_the_overlay() {
        let mut track = DragTrack::default();
        let first = track.enter(DropHalf::Session);
        let dropped = track.end();
        assert_eq!(track.checked(first, title(), None), None);
        let second = track.enter(DropHalf::Session);
        assert!(
            !track.still_ended(dropped),
            "the drop's end can't hide the new drag"
        );
        assert_eq!(track.moved(DropHalf::Project), None, "nothing shown yet");
        let shown = track.checked(second, title(), None).map(|drag| drag.half);
        assert_eq!(shown, Some(DropHalf::Project));
        assert_eq!(
            track.moved(DropHalf::Session).map(|drag| drag.half),
            Some(DropHalf::Session)
        );
        let left = track.end();
        assert!(track.still_ended(left));
    }
}
