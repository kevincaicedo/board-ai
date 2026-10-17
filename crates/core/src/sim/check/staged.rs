//! **I16**, staged diagrams (D-187): a staged part is drawn only by a reveal, at most once, and
//! exactly where it was laid out when its diagram was staged, before its first stroke; nothing
//! but a staged part is in a reveal. Together with I6 this is "nothing drawn ever moves": the
//! layout was fixed before anything of it was drawn. Checked on every simulator step.

use std::collections::BTreeMap;

use crate::engine::Core;
use crate::ids::{ElementId, LayerId};
use crate::protocol::{DrawOp, StagedPart, UiEvent};
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Staged {
    /// Every part of every diagram staged, as laid out when it was staged.
    laid_out: BTreeMap<(LayerId, ElementId), StagedPart>,
    /// The parts drawn so far.
    drawn: BTreeMap<(LayerId, ElementId), StagedPart>,
}

impl Staged {
    pub fn on_ui(
        &mut self,
        now: SessionTime,
        event: &UiEvent,
        core: &Core,
        violations: &mut Vec<String>,
    ) {
        let mut violate =
            |what: String| violations.push(format!("{} ms: I16: {what}", now.as_ms()));
        match event {
            // Staged: everything is laid out now, before its first stroke.
            UiEvent::Diagram { layer, part: 0, .. } => {
                for diagram in core.board().staged(*layer) {
                    for entry in &diagram.entries {
                        self.laid_out
                            .entry((*layer, entry.id.clone()))
                            .or_insert_with(|| entry.draw.clone());
                    }
                    for legend in &diagram.legend {
                        let part = StagedPart::Legend {
                            id: legend.id.clone(),
                            rect: legend.rect,
                            category: legend.category,
                            label: legend.label.clone(),
                        };
                        self.laid_out
                            .entry((*layer, legend.id.clone()))
                            .or_insert(part);
                    }
                }
            }
            UiEvent::OpStart { layer, op, .. } => {
                for problem in self.drawn(*layer, op) {
                    violate(problem);
                }
            }
            // The page and its diagrams left the board (D-177): ids are free again on layer 0.
            UiEvent::PageTurn { .. } => {
                self.laid_out
                    .retain(|(layer, _), _| *layer != LayerId::MAIN);
                self.drawn.retain(|(layer, _), _| *layer != LayerId::MAIN);
            }
            UiEvent::Diagram { .. }
            | UiEvent::Orb { .. }
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
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings
            | UiEvent::SessionEnd
            | UiEvent::DrawMode { .. }
            | UiEvent::UserShape { .. }
            | UiEvent::UserShapeRemove { .. }
            | UiEvent::LabelHint { .. } => {}
        }
    }

    /// What is wrong with `op` starting on `layer`.
    fn drawn(&mut self, layer: LayerId, op: &DrawOp) -> Vec<String> {
        let DrawOp::Reveal { parts, .. } = op else {
            // A sketch's op redrawing a staged part draws it outside its reveal.
            let id = match op {
                DrawOp::Node { id, .. }
                | DrawOp::Edge { id, .. }
                | DrawOp::Participant { id, .. }
                | DrawOp::Message { id, .. }
                | DrawOp::Cells { id, .. } => id,
                DrawOp::Annotate { .. }
                | DrawOp::Strike { .. }
                | DrawOp::Trace { .. }
                | DrawOp::Write { .. }
                | DrawOp::Reveal { .. } => return Vec::new(),
            };
            return if self.laid_out.contains_key(&(layer, id.clone())) {
                vec![format!("{} was drawn outside its reveal", id.as_str())]
            } else {
                Vec::new()
            };
        };
        let mut problems = Vec::new();
        for part in parts {
            let key = (layer, part.id().clone());
            match self.laid_out.get(&key) {
                None => problems.push(format!(
                    "{} was revealed but never staged on layer {}",
                    part.id().as_str(),
                    layer.0
                )),
                Some(laid_out) if laid_out != part => problems.push(format!(
                    "{} was drawn somewhere other than where it was laid out",
                    part.id().as_str()
                )),
                Some(_) => {}
            }
            if self.drawn.insert(key, part.clone()).is_some() {
                problems.push(format!("{} was revealed twice", part.id().as_str()));
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::Staged;
    use crate::geometry::Rect;
    use crate::ids::{ElementId, LayerId, OpId};
    use crate::protocol::{DrawOp, NodeShape, StagedPart};

    fn id(text: &str) -> ElementId {
        ElementId::parse(text).expect("valid id")
    }

    fn part(name: &str, x: i32) -> StagedPart {
        StagedPart::Node {
            id: id(name),
            shape: NodeShape::Service,
            rect: Rect {
                x,
                y: 0,
                w: 60,
                h: 32,
            },
            label: name.to_owned(),
            note: None,
            step: None,
            category: None,
            dashed: false,
        }
    }

    fn reveal(parts: Vec<StagedPart>) -> DrawOp {
        DrawOp::Reveal {
            id: id("r:1"),
            op: OpId(1),
            parts,
        }
    }

    fn checker() -> Staged {
        let mut staged = Staged::default();
        staged
            .laid_out
            .insert((LayerId::MAIN, id("arena")), part("arena", 0));
        staged
    }

    /// Planted violations: a part revealed twice, moved after staging, never staged, or drawn
    /// by a sketch's op; a clean reveal passes.
    #[test]
    fn each_way_of_breaking_i16_is_caught() {
        let main = LayerId::MAIN;
        let mut clean = checker();
        assert!(
            clean
                .drawn(main, &reveal(vec![part("arena", 0)]))
                .is_empty()
        );
        assert_eq!(clean.drawn(main, &reveal(vec![part("arena", 0)])).len(), 1);
        assert_eq!(
            checker()
                .drawn(main, &reveal(vec![part("arena", 40)]))
                .len(),
            1
        );
        assert_eq!(
            checker().drawn(main, &reveal(vec![part("pool", 0)])).len(),
            1
        );
        let sketch = DrawOp::Node {
            id: id("arena"),
            op: OpId(2),
            shape: NodeShape::Service,
            label: "Arena".into(),
            rect: Rect::default(),
            stack: 1,
            fill: None,
            link: None,
        };
        assert_eq!(checker().drawn(main, &sketch).len(), 1);
        assert!(checker().drawn(LayerId(1), &sketch).is_empty());
    }
}
