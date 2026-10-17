//! How elements move through their states (`Pending` → `Drawing` → `Drawn`, `Frozen` by an
//! interruption and back, `Struck` by a correction). Every element an op made moves with it, so a
//! row of cells or a tree node with its line freezes and resumes as one stroke.

use crate::board::{Board, ElementKind, ElementState, FrozenOp, Geometry, layout, node_draw};
use crate::ids::{ElementId, LayerId};
use crate::protocol::DrawOp;
use crate::time::SessionTime;

impl Board {
    /// The element's op starts now, with every element it made. Returns the redraw of its group
    /// when the group had to grow.
    pub fn start(
        &mut self,
        layer_id: LayerId,
        id: &ElementId,
        now: SessionTime,
        budget_ms: u32,
    ) -> Option<DrawOp> {
        let layer = self.layer_mut(layer_id).ok()?;
        let op = layer.element(id)?.op?;
        for element in &mut layer.elements {
            if element.op == Some(op) && element.state == ElementState::Pending {
                element.state = ElementState::Drawing {
                    started_at: now,
                    budget_ms,
                };
            }
        }
        if let Some(ElementKind::Message { commit, .. }) =
            layer.element(id).map(|element| element.kind.clone())
        {
            layer.commit_row(commit);
        }
        let element = layer.element(id)?;
        let (
            ElementKind::Node {
                group: Some(group), ..
            },
            Geometry::Box(child),
        ) = (&element.kind, &element.geometry)
        else {
            return None;
        };
        let (group, child) = (group.clone(), *child);
        let group_element = layer.element_mut(&group)?;
        let Geometry::Box(old) = group_element.geometry else {
            return None;
        };
        let group_op = group_element.op?;
        let needed = layout::inflate(child, layout::GROUP_PADDING, layout::GROUP_LABEL_SPACE);
        let grown = old.union(needed);
        if grown == old {
            return None;
        }
        group_element.geometry = Geometry::Box(grown);
        Some(node_draw(group_element, group_op, grown, None))
    }

    pub fn apply_strike(&mut self, layer_id: LayerId, target: &ElementId, label: &str) {
        if let Some(element) = self
            .layer_mut(layer_id)
            .ok()
            .and_then(|layer| layer.element_mut(target))
        {
            label.clone_into(&mut element.label);
            element.state = ElementState::Struck;
        }
    }

    /// Removes an element that was laid out but never started, with everything its op made; the
    /// user never saw them.
    pub fn remove_pending(&mut self, layer_id: LayerId, id: &ElementId) {
        let Ok(layer) = self.layer_mut(layer_id) else {
            return;
        };
        let Some(op) = layer
            .element(id)
            .filter(|element| element.state == ElementState::Pending)
            .and_then(|element| element.op)
        else {
            return;
        };
        let removed: Vec<ElementId> = layer
            .made_by(op)
            .filter(|element| element.state == ElementState::Pending)
            .map(|element| element.id.clone())
            .collect();
        layer
            .elements
            .retain(|element| !(element.op == Some(op) && element.state == ElementState::Pending));
        for id in &removed {
            layer.grid.release(id);
        }
        layer.index = layer
            .elements
            .iter()
            .enumerate()
            .map(|(index, element)| (element.id.clone(), index))
            .collect();
    }

    /// Marks strokes whose budget has passed as drawn.
    pub fn settle(&mut self, now: SessionTime) {
        for element in self
            .layers
            .iter_mut()
            .flat_map(|layer| layer.elements.iter_mut())
        {
            if let ElementState::Drawing {
                started_at,
                budget_ms,
            } = element.state
                && now >= started_at.plus_ms(u64::from(budget_ms))
            {
                element.state = ElementState::Drawn;
            }
        }
    }

    /// Freezes the stroke in flight on `layer_id`: the most recently started op still within its
    /// budget, with every element it made. Any older stroke still drawing is treated as
    /// finished.
    pub fn freeze(&mut self, layer_id: LayerId, now: SessionTime) -> Option<FrozenOp> {
        self.settle(now);
        let layer = self.layer_mut(layer_id).ok()?;
        let (_, latest_op) = layer
            .elements
            .iter()
            .filter_map(|element| match element.state {
                ElementState::Drawing { started_at, .. } => Some((started_at, element.op?)),
                ElementState::Pending
                | ElementState::Planned
                | ElementState::Drawn
                | ElementState::Frozen { .. }
                | ElementState::Struck => None,
            })
            .max()?;
        for element in &mut layer.elements {
            let ElementState::Drawing {
                started_at,
                budget_ms,
            } = element.state
            else {
                continue;
            };
            if element.op != Some(latest_op) {
                element.state = ElementState::Drawn;
                continue;
            }
            let elapsed_ms = u32::try_from(now.ms_since(started_at)).unwrap_or(u32::MAX);
            let progress = elapsed_ms.saturating_mul(100) / budget_ms.max(1);
            let progress_percent = u8::try_from(progress.min(99)).unwrap_or(99);
            element.state = ElementState::Frozen {
                progress_percent,
                remaining_ms: budget_ms.saturating_sub(elapsed_ms),
            };
        }
        let element = layer.made_by(latest_op).next()?;
        let ElementState::Frozen {
            progress_percent, ..
        } = element.state
        else {
            return None;
        };
        Some(FrozenOp {
            layer: layer_id,
            id: element.id.clone(),
            label: element.label.clone(),
            progress_percent,
        })
    }

    /// Restarts the frozen stroke on `layer_id`, with every element its op made; returns the
    /// budget it has left.
    pub fn resume_frozen(&mut self, layer_id: LayerId, now: SessionTime) -> Option<u32> {
        let layer = self.layer_mut(layer_id).ok()?;
        let mut left = None;
        for element in &mut layer.elements {
            if let ElementState::Frozen { remaining_ms, .. } = element.state {
                element.state = ElementState::Drawing {
                    started_at: now,
                    budget_ms: remaining_ms,
                };
                left = Some(remaining_ms);
            }
        }
        left
    }
}
