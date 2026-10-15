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

/// Where a new node goes, and the group it joins.
pub(super) fn node_cell(
    layer: &Layer,
    placement: &Placement,
    cols: i32,
) -> Result<(layout::Cell, Option<ElementId>), Rejected> {
    match placement {
        Placement::Auto => {
            let cell = match layer.last_node() {
                Some(last) => {
                    let (_, span) = layer.node_rect(&last.id)?;
                    let start = layout::start_next_to(span, cols, Direction::Right);
                    layer.grid.find(start, cols, Direction::Right, None)
                }
                None => {
                    let origin = layout::Cell { row: 0, col: 0 };
                    layer.grid.find(origin, cols, Direction::Right, None)
                }
            };
            Ok((cell, None))
        }
        Placement::Near { target, direction } => {
            let (_, span) = layer.node_rect(target)?;
            let start = layout::start_next_to(span, cols, *direction);
            Ok((layer.grid.find(start, cols, *direction, None), None))
        }
        Placement::Inside { group, near } => {
            let (_, group_span) = layer.node_rect(group)?;
            let is_group = matches!(
                layer.element(group).map(|element| &element.kind),
                Some(ElementKind::Node {
                    shape: NodeShape::Group,
                    ..
                })
            );
            if !is_group {
                return Err(Rejected::NotAGroup(group.clone()));
            }
            let (start, direction) = match near {
                Some((target, direction)) => {
                    let (_, span) = layer.node_rect(target)?;
                    (layout::start_next_to(span, cols, *direction), *direction)
                }
                None => (group_span.cell, Direction::Right),
            };
            let cell = layer.grid.find(start, cols, direction, Some(group));
            Ok((cell, Some(group.clone())))
        }
    }
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
