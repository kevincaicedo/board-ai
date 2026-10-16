//! The last board state of a session, rebuilt from its log for `open_session`: the layers still
//! open at the end, each with its ops in order and its last camera frame (the core frames each
//! layer on its own). Detours closed before the end are gone from the board, so their ops are
//! left out.

use boardai_core::ids::LayerId;
use boardai_core::protocol::UiEvent;

use crate::limits::SESSION_BOARD_EVENTS_MAX;
use crate::store::log::LogLine;

struct Layer {
    id: LayerId,
    /// `None` for the main layer, which has no `LayerPush`.
    push: Option<UiEvent>,
    ops: Vec<UiEvent>,
    camera: Option<UiEvent>,
}

/// Board events rebuilt from log lines, and how many were left out at the bound.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BoardState {
    pub events: Vec<UiEvent>,
    pub skipped: u64,
}

/// Folds a log into its final board. Memory is bounded: ops past `SESSION_BOARD_EVENTS_MAX`
/// are counted, not kept.
pub(crate) fn final_board(lines: impl Iterator<Item = LogLine>) -> BoardState {
    let mut layers = vec![Layer {
        id: LayerId::MAIN,
        push: None,
        ops: Vec::new(),
        camera: None,
    }];
    let mut kept = 0_usize;
    let mut skipped = 0_u64;
    for line in lines {
        let LogLine::Ui { event, .. } = line else {
            continue;
        };
        match event {
            UiEvent::LayerPush { layer, .. } => layers.push(Layer {
                id: layer,
                push: Some(event),
                ops: Vec::new(),
                camera: None,
            }),
            UiEvent::LayerPop { layer } => {
                if layer != LayerId::MAIN
                    && let Some(position) = layers.iter().rposition(|open| open.id == layer)
                {
                    let popped = layers.remove(position);
                    kept = kept.saturating_sub(popped.ops.len());
                }
            }
            UiEvent::OpStart { layer, .. } => {
                let Some(open) = layers.iter_mut().rev().find(|open| open.id == layer) else {
                    continue;
                };
                if kept >= SESSION_BOARD_EVENTS_MAX {
                    skipped += 1;
                    continue;
                }
                kept += 1;
                open.ops.push(event);
            }
            UiEvent::Camera { layer, .. } => {
                if let Some(open) = layers.iter_mut().rev().find(|open| open.id == layer) {
                    open.camera = Some(event);
                }
            }
            UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Caption { .. }
            | UiEvent::Thread { .. }
            | UiEvent::Freeze { .. }
            | UiEvent::Resume { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. }
            | UiEvent::Suggestion { .. }
            | UiEvent::SessionStart { .. }
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings => {}
        }
    }
    let mut events = Vec::new();
    for layer in layers {
        events.extend(layer.push);
        events.extend(layer.ops);
        events.extend(layer.camera);
    }
    BoardState { events, skipped }
}

#[cfg(test)]
mod tests {
    use boardai_core::geometry::Rect;
    use boardai_core::ids::{ElementId, LayerId, OpId};
    use boardai_core::protocol::{DrawOp, NodeShape, UiEvent};
    use boardai_core::time::SessionTime;

    use super::final_board;
    use crate::store::log::LogLine;

    fn node(layer: u8, id: &str) -> LogLine {
        let rect = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        LogLine::Ui {
            t: SessionTime::ZERO,
            event: UiEvent::OpStart {
                layer: LayerId(layer),
                op: DrawOp::Node {
                    id: ElementId::parse(id).unwrap_or_else(|| unreachable!()),
                    op: OpId(0),
                    shape: NodeShape::Service,
                    label: id.to_owned(),
                    rect,
                    stack: 1,
                    fill: None,
                },
                budget_ms: 500,
            },
        }
    }

    fn ui(event: UiEvent) -> LogLine {
        LogLine::Ui {
            t: SessionTime::ZERO,
            event,
        }
    }

    #[test]
    fn closed_detours_leave_the_board_and_open_ones_stay() {
        let lines = vec![
            node(0, "lb"),
            ui(UiEvent::LayerPush {
                layer: LayerId(1),
                title: "cache".into(),
                question: None,
            }),
            node(1, "redis"),
            ui(UiEvent::LayerPop { layer: LayerId(1) }),
            node(0, "api"),
            ui(UiEvent::LayerPush {
                layer: LayerId(2),
                title: "open".into(),
                question: None,
            }),
            node(2, "queue"),
        ];
        let board = final_board(lines.into_iter());
        let labels: Vec<String> = board
            .events
            .iter()
            .map(|event| {
                if let UiEvent::OpStart {
                    op: DrawOp::Node { label, .. },
                    ..
                } = event
                {
                    label.clone()
                } else if let UiEvent::LayerPush { title, .. } = event {
                    format!("push {title}")
                } else {
                    format!("{event:?}")
                }
            })
            .collect();
        assert_eq!(labels, vec!["lb", "api", "push open", "queue"]);
        assert_eq!(board.skipped, 0);
    }

    /// The core frames each layer on its own (camera follow): every open layer keeps its last
    /// frame, and a closed detour's frame goes with it.
    #[test]
    fn each_open_layer_keeps_its_last_camera() {
        let camera = |layer: u8, x: i32| {
            ui(UiEvent::Camera {
                layer: LayerId(layer),
                frame: Rect {
                    x,
                    y: 0,
                    w: 100,
                    h: 100,
                },
                focus: None,
            })
        };
        let lines = vec![
            node(0, "lb"),
            camera(0, 1),
            camera(0, 2),
            ui(UiEvent::LayerPush {
                layer: LayerId(1),
                title: "closed".into(),
                question: None,
            }),
            camera(1, 3),
            ui(UiEvent::LayerPop { layer: LayerId(1) }),
            ui(UiEvent::LayerPush {
                layer: LayerId(2),
                title: "open".into(),
                question: Some("why?".into()),
            }),
            camera(2, 4),
        ];
        let frames: Vec<(u8, i32)> = final_board(lines.into_iter())
            .events
            .iter()
            .filter_map(|event| {
                if let UiEvent::Camera { layer, frame, .. } = event {
                    Some((layer.0, frame.x))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(frames, vec![(0, 2), (2, 4)]);
    }
}
