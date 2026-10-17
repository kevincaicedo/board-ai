//! The invariants of drawing together (artboard 13), checked on every simulator step:
//!
//! - **I14**: no talker stream is in flight while the user draws: none was left running when
//!   drawing began, and no `Talk` starts until it ends. No view opens over the drawing either
//!   (LLD §3.7: drawing closes one, and a read made before drawing began answers nothing).
//! - **I6 for the user's shapes**: a shape's geometry never changes once echoed (a pen shape only
//!   gains or loses its last strokes; an arrow's loose end may connect, and an end comes loose
//!   only when the shape it joined was undone), BoardAI never reuses one's id, and a box, row of
//!   cells or participant head BoardAI starts never overlaps a box or pen shape of the user's on
//!   its layer. Connectors, marks, group frames and lifelines may cross them.

use std::collections::BTreeMap;

use crate::board::staged::metrics::part_area;
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId};
use crate::protocol::{DrawOp, NodeShape, StagedPart, UiEvent, UserShapeKind};
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Yours {
    drawing: bool,
    /// Each shape of the user's as last echoed.
    shapes: BTreeMap<(LayerId, ElementId), UserShapeKind>,
}

impl Yours {
    /// A talker request starts.
    pub fn on_talk(&self, now: SessionTime, violations: &mut Vec<String>) {
        if self.drawing {
            violations.push(format!(
                "{} ms: I14: a talker request started while the user draws",
                now.as_ms()
            ));
        }
    }

    pub fn on_ui(
        &mut self,
        now: SessionTime,
        event: &UiEvent,
        talk_in_flight: bool,
        violations: &mut Vec<String>,
    ) {
        let mut violate = |what: String| violations.push(format!("{} ms: {what}", now.as_ms()));
        match event {
            UiEvent::DrawMode { drawing } => {
                if *drawing && talk_in_flight {
                    violate("I14: drawing began with a talker stream in flight".into());
                }
                self.drawing = *drawing;
            }
            UiEvent::UserShape { layer, shape } => {
                let key = (*layer, shape.id.clone());
                let exists = |id: &ElementId| self.shapes.contains_key(&(*layer, id.clone()));
                if let Some(old) = self.shapes.get(&key)
                    && !kept(old, &shape.kind, exists)
                {
                    violate(format!("I6: the user's {} moved", shape.id.as_str()));
                }
                self.shapes.insert(key, shape.kind.clone());
            }
            UiEvent::UserShapeRemove { layer, id } => {
                self.shapes.remove(&(*layer, id.clone()));
            }
            // The page turned (D-177): the user's shapes on it went with it, kept in the record.
            UiEvent::PageTurn { .. } => {
                self.shapes.retain(|(layer, _), _| *layer != LayerId::MAIN);
            }
            UiEvent::OpStart { layer, op, .. } => {
                if let Some(problem) = self.over_yours(*layer, op) {
                    violate(problem);
                }
            }
            UiEvent::ViewShow { .. } => {
                if self.drawing {
                    violate("a view opened over the user's drawing".into());
                }
            }
            UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Caption { .. }
            | UiEvent::Thread { .. }
            | UiEvent::Freeze { .. }
            | UiEvent::Resume { .. }
            | UiEvent::LayerPush { .. }
            | UiEvent::LayerPop { .. }
            | UiEvent::Camera { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. }
            | UiEvent::Suggestion { .. }
            | UiEvent::SessionStart { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings
            | UiEvent::SessionEnd
            | UiEvent::LabelHint { .. }
            | UiEvent::Diagram { .. } => {}
        }
    }

    /// What is wrong with BoardAI starting `op` on `layer` among the user's shapes.
    fn over_yours(&self, layer: LayerId, op: &DrawOp) -> Option<String> {
        let (id, area) = match op {
            DrawOp::Node {
                id, shape, rect, ..
            } => (id, (*shape != NodeShape::Group).then_some(*rect)),
            DrawOp::Participant { id, rect, .. } => (id, Some(*rect)),
            DrawOp::Cells { id, .. } => (id, Some(crate::board::layout::op_bounds(op))),
            DrawOp::Edge { id, .. }
            | DrawOp::Annotate { id, .. }
            | DrawOp::Trace { id, .. }
            | DrawOp::Message { id, .. } => (id, None),
            DrawOp::Strike { target, .. } | DrawOp::Write { target, .. } => {
                return self
                    .shapes
                    .contains_key(&(layer, target.clone()))
                    .then(|| format!("I6: BoardAI rewrote the user's {}", target.as_str()));
            }
            // A staged diagram's parts each count; its frames and connectors may cross.
            DrawOp::Reveal { parts, .. } => {
                return parts
                    .iter()
                    .find_map(|part| self.part_over_yours(layer, part));
            }
        };
        self.solid_over_yours(layer, id, area)
    }

    fn part_over_yours(&self, layer: LayerId, part: &StagedPart) -> Option<String> {
        let area = match part {
            StagedPart::Frame { .. } | StagedPart::Edge { .. } => None,
            StagedPart::Node { .. }
            | StagedPart::Note { .. }
            | StagedPart::Record { .. }
            | StagedPart::Cells { .. }
            | StagedPart::Legend { .. } => Some(part_area(part)),
        };
        self.solid_over_yours(layer, part.id(), area)
    }

    fn solid_over_yours(
        &self,
        layer: LayerId,
        id: &ElementId,
        area: Option<Rect>,
    ) -> Option<String> {
        if self.shapes.contains_key(&(layer, id.clone())) {
            return Some(format!("I6: BoardAI redrew the user's {}", id.as_str()));
        }
        let area = area?;
        self.shapes
            .iter()
            .filter(|((on, _), _)| *on == layer)
            .find_map(|((_, yours), kind)| {
                let bounds = solid_bounds(kind)?;
                bounds.intersects(area).then(|| {
                    format!(
                        "I6: BoardAI drew {} over the user's {}",
                        id.as_str(),
                        yours.as_str()
                    )
                })
            })
    }
}

/// A box's or pen shape's box; an arrow is a connector, which BoardAI may draw across.
fn solid_bounds(kind: &UserShapeKind) -> Option<Rect> {
    match kind {
        UserShapeKind::Box { rect } => Some(*rect),
        UserShapeKind::Ink { bounds, .. } => Some(*bounds),
        UserShapeKind::Arrow { .. } => None,
    }
}

/// A re-sent shape keeps its geometry: the same box, the same arrow (a loose end may connect to
/// a shape drawn after it, an end comes loose only from a shape that no longer `exists`, and a
/// connection never moves to another shape), or a pen shape that gained or lost strokes at its
/// end.
fn kept(old: &UserShapeKind, new: &UserShapeKind, exists: impl Fn(&ElementId) -> bool) -> bool {
    let connects = |old: &Option<ElementId>, new: &Option<ElementId>| match (old, new) {
        (None, _) => true,
        (Some(old), Some(new)) => old == new,
        (Some(old), None) => !exists(old),
    };
    match (old, new) {
        (UserShapeKind::Ink { strokes: old, .. }, UserShapeKind::Ink { strokes: new, .. }) => {
            new.starts_with(old) || old.starts_with(new)
        }
        (
            UserShapeKind::Arrow {
                start,
                end,
                from,
                to,
            },
            UserShapeKind::Arrow {
                start: new_start,
                end: new_end,
                from: new_from,
                to: new_to,
            },
        ) => {
            start == new_start && end == new_end && connects(from, new_from) && connects(to, new_to)
        }
        (
            UserShapeKind::Box { .. },
            UserShapeKind::Box { .. } | UserShapeKind::Arrow { .. } | UserShapeKind::Ink { .. },
        )
        | (UserShapeKind::Arrow { .. }, UserShapeKind::Box { .. } | UserShapeKind::Ink { .. })
        | (UserShapeKind::Ink { .. }, UserShapeKind::Box { .. } | UserShapeKind::Arrow { .. }) => {
            old == new
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Yours;
    use crate::geometry::{Point, Rect};
    use crate::ids::{ElementId, LayerId, OpId};
    use crate::protocol::{DrawOp, NodeShape, UiEvent, UserShape, UserShapeKind};
    use crate::time::SessionTime;
    use crate::views::View;

    fn id(text: &str) -> ElementId {
        ElementId::parse(text).unwrap_or_else(|| unreachable!("a valid id"))
    }

    fn boxed(name: &str, x: i32) -> UiEvent {
        UiEvent::UserShape {
            layer: LayerId::MAIN,
            shape: UserShape {
                id: id(name),
                kind: UserShapeKind::Box {
                    rect: Rect {
                        x,
                        y: 0,
                        w: 100,
                        h: 60,
                    },
                },
                label: None,
                note: None,
            },
        }
    }

    /// An arrow from (0, 0) to (200, 0) named `name`, joined at its end to `to`.
    fn arrow(name: &str, to: Option<&str>) -> UiEvent {
        UiEvent::UserShape {
            layer: LayerId::MAIN,
            shape: UserShape {
                id: id(name),
                kind: UserShapeKind::Arrow {
                    start: Point { x: 0, y: 0 },
                    end: Point { x: 200, y: 0 },
                    from: None,
                    to: to.map(id),
                },
                label: None,
                note: None,
            },
        }
    }

    fn node(name: &str, x: i32) -> UiEvent {
        UiEvent::OpStart {
            layer: LayerId::MAIN,
            op: DrawOp::Node {
                id: id(name),
                op: OpId(1),
                shape: NodeShape::Service,
                label: name.into(),
                rect: Rect {
                    x,
                    y: 10,
                    w: 120,
                    h: 64,
                },
                stack: 1,
                fill: None,
                link: None,
            },
            budget_ms: 500,
        }
    }

    /// A staged diagram's reveal (D-187): a solid part over the user's box is caught; a frame
    /// around it and a connector across it are not.
    #[test]
    fn a_revealed_part_over_your_box_is_caught_and_frames_may_cross() {
        use crate::protocol::{StagedPart, TextAt};
        let now = SessionTime::from_ms(1);
        let mut violations = Vec::new();
        let mut yours = Yours::default();
        yours.on_ui(now, &boxed("u1", 0), false, &mut violations);
        let rect = |x: i32| Rect {
            x,
            y: 10,
            w: 60,
            h: 30,
        };
        let frame = StagedPart::Frame {
            id: id("f"),
            rect: Rect {
                x: -20,
                y: -20,
                w: 400,
                h: 200,
            },
            label: TextAt {
                text: "F".into(),
                at: Point { x: 0, y: 0 },
            },
            note: None,
            category: None,
            dashed: true,
        };
        let part = |x: i32| StagedPart::Node {
            id: id("p"),
            shape: NodeShape::Pill,
            rect: rect(x),
            label: "p".into(),
            note: None,
            step: None,
            category: None,
            dashed: false,
        };
        let reveal = |parts: Vec<StagedPart>| UiEvent::OpStart {
            layer: LayerId::MAIN,
            op: DrawOp::Reveal {
                id: id("r:1"),
                op: OpId(1),
                parts,
            },
            budget_ms: 500,
        };
        yours.on_ui(
            now,
            &reveal(vec![frame.clone(), part(300)]),
            false,
            &mut violations,
        );
        assert!(violations.is_empty(), "{violations:?}");
        yours.on_ui(now, &reveal(vec![frame, part(20)]), false, &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
    }

    /// The planted violations: each turns the check red.
    #[test]
    fn talk_while_drawing_moved_shapes_and_boxes_over_them_are_caught() {
        let now = SessionTime::from_ms(1);
        let mut violations = Vec::new();
        let mut yours = Yours::default();
        yours.on_ui(
            now,
            &UiEvent::DrawMode { drawing: true },
            true,
            &mut violations,
        );
        yours.on_talk(now, &mut violations);
        assert_eq!(violations.len(), 2, "{violations:?}");
        violations.clear();
        yours.on_ui(now, &boxed("u1", 0), false, &mut violations);
        yours.on_ui(now, &boxed("u1", 5), false, &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
        violations.clear();
        yours.on_ui(now, &node("api", 50), false, &mut violations);
        yours.on_ui(now, &node("u1", 900), false, &mut violations);
        assert_eq!(violations.len(), 2, "{violations:?}");
        violations.clear();
        // An arrow's end comes loose only from a shape that is gone.
        let to_u1 = arrow("u2", Some("u1"));
        yours.on_ui(now, &to_u1, false, &mut violations);
        yours.on_ui(now, &arrow("u2", None), false, &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
        violations.clear();
        yours.on_ui(now, &to_u1, false, &mut violations);
        let undone = UiEvent::UserShapeRemove {
            layer: LayerId::MAIN,
            id: id("u1"),
        };
        yours.on_ui(now, &undone, false, &mut violations);
        yours.on_ui(now, &arrow("u2", None), false, &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
        yours.on_ui(now, &boxed("u1", 0), false, &mut violations);
        // A view over the drawing.
        let view = UiEvent::ViewShow {
            view: View::Memory {
                about_you: Vec::new(),
                systems: Vec::new(),
                notes: Vec::new(),
                latest: None,
            },
            today: "2026-10-16".to_owned(),
        };
        yours.on_ui(now, &view, false, &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
        violations.clear();
        // Beside it, after drawing ended: fine.
        yours.on_ui(
            now,
            &UiEvent::DrawMode { drawing: false },
            false,
            &mut violations,
        );
        yours.on_ui(now, &node("db", 400), false, &mut violations);
        yours.on_ui(now, &view, false, &mut violations);
        yours.on_talk(now, &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
