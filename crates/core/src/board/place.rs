//! Placement helpers for the board: where a new node goes, what a repeated id means, and the
//! path between two connected nodes.

use crate::board::{Element, ElementKind, ElementState, Geometry, Layer, Placed, Rejected, layout};
use crate::geometry::Rect;
use crate::ids::{ElementId, OpId};
use crate::narration::ops::{Direction, Placement};
use crate::protocol::{DrawOp, EdgePath, NodeShape};

/// A repeated id: finish it if frozen, strike it if the label changed, else nothing.
pub(super) fn repeated(existing: &Element, op: OpId, label: &str) -> Placed {
    match existing.state {
        ElementState::Frozen { .. } => Placed::ResumeFrozen,
        ElementState::Pending
        | ElementState::Drawing { .. }
        | ElementState::Drawn
        | ElementState::Struck => {
            if existing.label == label {
                Placed::Unchanged
            } else {
                strike(existing, op, label)
            }
        }
    }
}

pub(super) fn strike(existing: &Element, op: OpId, label: &str) -> Placed {
    let rect = match existing.geometry {
        Geometry::Box(rect) => rect,
        Geometry::Path(path) => {
            let middle = layout::path_bounds(path).center();
            Rect {
                x: middle.x - 40,
                y: middle.y - 12,
                w: 80,
                h: 24,
            }
        }
        Geometry::Points(_) => existing.geometry.bounds(),
    };
    Placed::Strike {
        target: existing.id.clone(),
        draw: DrawOp::Strike {
            op,
            target: existing.id.clone(),
            label: label.to_owned(),
            rect,
        },
    }
}

/// Where a new node goes, and the group it joins. A placement hint that names nothing on this
/// layer (the main board's id used inside a detour, a typo) never loses the node: it continues
/// the flow as if it had no hint. `in` naming a node that isn't a group places it next to that
/// node instead.
pub(super) fn node_cell(
    layer: &Layer,
    placement: &Placement,
    cols: i32,
) -> Result<(layout::Cell, Option<ElementId>), Rejected> {
    match placement {
        Placement::Auto => Ok((auto_cell(layer, cols)?, None)),
        Placement::Near { target, direction } => {
            Ok((near_cell(layer, target, *direction, cols)?, None))
        }
        Placement::Inside { group, near } => {
            let Ok((_, group_span)) = layer.node_rect(group) else {
                return Ok((auto_cell(layer, cols)?, None));
            };
            let is_group = matches!(
                layer.element(group).map(|element| &element.kind),
                Some(ElementKind::Node {
                    shape: NodeShape::Group,
                    ..
                })
            );
            if !is_group {
                return Ok((near_cell(layer, group, Direction::Right, cols)?, None));
            }
            let (start, direction) = match near
                .as_ref()
                .and_then(|(target, direction)| Some((layer.node_rect(target).ok()?, *direction)))
            {
                Some(((_, span), direction)) => {
                    (layout::start_next_to(span, cols, direction), direction)
                }
                None => (group_span.cell, Direction::Right),
            };
            let cell = layer.grid.find(start, cols, direction, Some(group));
            Ok((cell, Some(group.clone())))
        }
    }
}

/// Continue the flow: right of the last node placed on the layer, or the origin.
fn auto_cell(layer: &Layer, cols: i32) -> Result<layout::Cell, Rejected> {
    let start = match layer.last_node() {
        Some(last) => {
            let (_, span) = layer.node_rect(&last.id)?;
            layout::start_next_to(span, cols, Direction::Right)
        }
        None => layout::Cell { row: 0, col: 0 },
    };
    Ok(layer.grid.find(start, cols, Direction::Right, None))
}

/// Next to `target` in `direction`, or the flow when `target` isn't on the layer.
fn near_cell(
    layer: &Layer,
    target: &ElementId,
    direction: Direction,
    cols: i32,
) -> Result<layout::Cell, Rejected> {
    let Ok((_, span)) = layer.node_rect(target) else {
        return auto_cell(layer, cols);
    };
    let start = layout::start_next_to(span, cols, direction);
    Ok(layer.grid.find(start, cols, direction, None))
}

/// The path of the edge `from>to`, or `to>from` walked backwards.
pub(super) fn edge_between(layer: &Layer, from: &ElementId, to: &ElementId) -> Option<EdgePath> {
    layer.elements.iter().find_map(|element| {
        let (ElementKind::Edge { from: a, to: b, .. }, Geometry::Path(path)) =
            (&element.kind, &element.geometry)
        else {
            return None;
        };
        if element.state == ElementState::Pending {
            return None;
        }
        if a == from && b == to {
            Some(*path)
        } else if a == to && b == from {
            Some(EdgePath {
                start: path.end,
                control: path.control,
                end: path.start,
            })
        } else {
            None
        }
    })
}
