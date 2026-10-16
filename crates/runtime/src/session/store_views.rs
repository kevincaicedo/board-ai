//! The store's answers to the talker's store tools, as tool outputs (LLD §3.3, §3.7): recall as
//! text, and the views (library, Now list, memory, a past board, a replay) as the protocol's
//! [`View`], which the core shows and describes to the talker. Also the context a session
//! starts with.

use boardai_core::ids::LayerId;
use boardai_core::protocol::UiEvent;
use boardai_core::tools::{ToolFailure, ToolOutput};
use boardai_core::views::{PastLayer, SessionCard, SessionRef, TaskRow, View};

use super::ports::StoreRead;
use boardai_core::limits::SUGGESTION_MAX_BYTES;
use boardai_core::narration::tags::bounded;

use crate::limits::{PAST_BOARD_OPS_MAX, SESSION_CONTEXT_MAX_BYTES};
use crate::store::UNTITLED_SESSION;
use crate::store::{LibraryEntry, SessionView, Store, StoreError, TaskView};

/// Answers one store read. `today` (`YYYY-MM-DD`, UTC) goes with views.
pub(crate) async fn read_view(store: &Store, read: StoreRead, today: String) -> ToolOutput {
    let view = match read {
        StoreRead::Recall { query } => return store.recall(&query).await,
        StoreRead::Library => store.library().await.map(|library| View::Library {
            more_sessions: count(library.more_sessions),
            sessions: library.sessions.into_iter().map(card).collect(),
        }),
        StoreRead::Tasks { project } => tasks(store, project).await,
        StoreRead::Memory => store.memory_view().await.map(|memory| View::Memory {
            about_you: memory.about_you,
            systems: memory.systems,
            notes: memory.notes,
            latest: memory.latest,
        }),
        StoreRead::PastSession { query } => store.open_session(&query).await.map(past_session),
        StoreRead::Replay { query } => store.find_session(&query).await.map(|entry| View::Replay {
            session: card(entry),
        }),
    };
    match view {
        Ok(view) => ToolOutput::View {
            view: Box::new(view),
            today,
        },
        Err(error) => failed(&error),
    }
}

async fn tasks(store: &Store, project: Option<String>) -> Result<View, StoreError> {
    let now = store.now_list(project.clone()).await?;
    let notes = store.recent_notes().await?;
    Ok(View::Tasks {
        project,
        more_tasks: count(now.more_tasks),
        tasks: now.tasks.into_iter().map(task).collect(),
        notes,
    })
}

fn failed(error: &StoreError) -> ToolOutput {
    let kind = match error {
        StoreError::NotFound { .. } => ToolFailure::NotFound,
        StoreError::Stopped | StoreError::NoSession | StoreError::SessionForgotten => {
            ToolFailure::Unavailable
        }
        StoreError::Vault(_)
        | StoreError::Io { .. }
        | StoreError::Index { .. }
        | StoreError::Refused { .. }
        | StoreError::SessionActive
        | StoreError::Limit { .. } => ToolFailure::Io,
    };
    ToolOutput::Failed {
        kind,
        message: error.to_string(),
    }
}

fn count(more: u64) -> u32 {
    u32::try_from(more).unwrap_or(u32::MAX)
}

fn card(entry: LibraryEntry) -> SessionCard {
    SessionCard {
        id: entry.id.to_string(),
        title: entry.title,
        date: entry.date,
        duration_ms: u32::try_from(entry.duration_ms).unwrap_or(u32::MAX),
        cover: entry.shapes,
    }
}

fn task(task: TaskView) -> TaskRow {
    let from = task.session.map(|id| SessionRef {
        id,
        title: task.session_title.unwrap_or_default(),
        date: task.session_date.unwrap_or_default(),
    });
    TaskRow {
        id: task.id,
        title: task.title,
        project: task.project,
        from,
    }
}

/// A past board: the layers open at the end, main first, at most [`PAST_BOARD_OPS_MAX`] ops.
fn past_session(session: SessionView) -> View {
    let mut layers: Vec<(LayerId, PastLayer)> = vec![(
        LayerId::MAIN,
        PastLayer {
            title: None,
            ops: Vec::new(),
        },
    )];
    let mut kept = 0_usize;
    let mut left_out = session.board_events_skipped;
    for event in session.board {
        if let UiEvent::LayerPush { layer, title, .. } = event {
            let layer_view = PastLayer {
                title: Some(title),
                ops: Vec::new(),
            };
            layers.push((layer, layer_view));
        } else if let UiEvent::OpStart { layer, op, .. } = event {
            let Some((_, open)) = layers.iter_mut().rev().find(|(id, _)| *id == layer) else {
                continue;
            };
            if kept >= PAST_BOARD_OPS_MAX {
                left_out += 1;
                continue;
            }
            kept += 1;
            open.ops.push(op);
        }
    }
    View::PastSession {
        session: SessionCard {
            id: session.id.to_string(),
            title: session.title,
            date: session.started.get(..10).unwrap_or_default().to_owned(),
            duration_ms: u32::try_from(session.duration_ms).unwrap_or(u32::MAX),
            cover: Vec::new(),
        },
        recap: session.recap,
        layers: layers.into_iter().map(|(_, layer)| layer).collect(),
        ops_left_out: count(left_out),
    }
}

/// The idle line under the orb (frame 1): "Pick up <last session's title> where we left off?",
/// within the core's `SUGGESTION_MAX_BYTES` (a long title is cut, the question kept). `None`
/// for a session that never got a real title.
pub(crate) fn suggestion_for(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() || title == UNTITLED_SESSION {
        return None;
    }
    let room = SUGGESTION_MAX_BYTES.saturating_sub(SUGGESTION_FRAME_BYTES);
    Some(format!(
        "{SUGGESTION_START}{}{SUGGESTION_END}",
        bounded(title, room)
    ))
}

const SUGGESTION_START: &str = "Pick up ";
const SUGGESTION_END: &str = " where we left off?";
const SUGGESTION_FRAME_BYTES: usize = SUGGESTION_START.len() + SUGGESTION_END.len();

/// What the talker starts a session with: where the last session left off, and what BoardAI
/// keeps about the user and their systems, within [`SESSION_CONTEXT_MAX_BYTES`]; and the idle
/// suggestion from the last session's title.
pub(crate) async fn session_context(store: &Store) -> (Option<String>, Option<String>) {
    let mut lines = Vec::new();
    match store.recap().await {
        Ok(Some(recap)) => lines.push(format!("Where you left off with the user. {recap}")),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "no recap for the new session"),
    }
    let mut suggestion = None;
    match store.memory_view().await {
        Ok(memory) => {
            suggestion = memory
                .latest
                .as_ref()
                .and_then(|latest| suggestion_for(&latest.session.title));
            if !memory.about_you.is_empty() {
                lines.push("What you keep about the user (use it, don't recite it):".to_owned());
                lines.extend(memory.about_you.iter().map(|row| format!("- {}", row.text)));
            }
            if !memory.systems.is_empty() {
                lines.push("What you keep about their systems:".to_owned());
                lines.extend(memory.systems.iter().map(|row| format!("- {}", row.text)));
            }
        }
        Err(error) => tracing::warn!(%error, "no memories for the new session"),
    }
    let mut context = String::new();
    for line in lines {
        if context.len() + line.len() + 1 > SESSION_CONTEXT_MAX_BYTES {
            break;
        }
        context.push_str(&line);
        context.push('\n');
    }
    ((!context.is_empty()).then_some(context), suggestion)
}

#[cfg(test)]
mod tests {
    use boardai_core::geometry::Rect;
    use boardai_core::ids::{ElementId, LayerId, OpId};
    use boardai_core::protocol::{DrawOp, NodeShape, UiEvent};
    use boardai_core::views::View;

    use super::{past_session, suggestion_for};
    use crate::limits::PAST_BOARD_OPS_MAX;
    use crate::store::{SessionId, SessionView, SkipCounts};

    fn op(layer: u8, index: usize) -> UiEvent {
        UiEvent::OpStart {
            layer: LayerId(layer),
            op: DrawOp::Node {
                id: ElementId::parse(&format!("n{index}")).unwrap_or_else(|| unreachable!()),
                op: OpId(0),
                shape: NodeShape::Service,
                label: format!("n{index}"),
                rect: Rect::default(),
                stack: 1,
                fill: None,
                link: None,
            },
            budget_ms: 400,
        }
    }

    /// The idle line fits the core's limit with its question intact.
    #[test]
    fn the_suggestion_names_the_last_session_and_fits() {
        assert_eq!(
            suggestion_for("Raft log compaction").as_deref(),
            Some("Pick up Raft log compaction where we left off?")
        );
        let long = suggestion_for(&"word ".repeat(80)).unwrap_or_default();
        assert!(
            long.len() <= boardai_core::limits::SUGGESTION_MAX_BYTES,
            "{long}"
        );
        assert!(long.starts_with("Pick up ") && long.ends_with(" where we left off?"));
        assert_eq!(suggestion_for("Untitled session"), None);
        assert_eq!(suggestion_for("  "), None);
    }

    #[test]
    fn a_past_board_keeps_its_layers_and_its_bound() {
        let mut board = vec![op(0, 0)];
        board.push(UiEvent::LayerPush {
            layer: LayerId(1),
            title: "cache-aside".into(),
            question: None,
        });
        board.extend((1..=PAST_BOARD_OPS_MAX + 2).map(|index| op(1, index)));
        let view = SessionView {
            id: SessionId::parse("2026-10-12-093000").unwrap_or_else(|| unreachable!()),
            title: "Raft".into(),
            started: "2026-10-12T09:30:00Z".into(),
            duration_ms: 60_000,
            events_path: std::path::PathBuf::new(),
            recap: "Snapshots.".into(),
            board,
            board_events_skipped: 4,
            log_skipped: SkipCounts::default(),
        };
        let View::PastSession {
            session,
            layers,
            ops_left_out,
            ..
        } = past_session(view)
        else {
            panic!("a past session");
        };
        assert_eq!(session.date, "2026-10-12");
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].ops.len(), 1);
        assert_eq!(layers[1].title.as_deref(), Some("cache-aside"));
        assert_eq!(
            layers[0].ops.len() + layers[1].ops.len(),
            PAST_BOARD_OPS_MAX
        );
        assert_eq!(
            ops_left_out,
            3 + 4,
            "three past the bound, four the store skipped"
        );
    }
}
