//! Placement helpers for the board: where a new node goes, what a repeated id means, and the
//! path between two connected nodes.

use crate::board::{
    Element, ElementKind, ElementState, Geometry, Layer, Placed, UserMark, free, layout,
};
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

/// Where a new drawing goes: grid cells (and the group it joins), or a box off the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Spot {
    Grid {
        cell: layout::Cell,
        cols: i32,
        group: Option<ElementId>,
    },
    Free {
        rect: Rect,
    },
}

/// Where a new drawing `width` × `height` goes. A placement hint that names nothing on this
/// layer (the main board's id used inside a detour, a typo) never loses the drawing: it
/// continues the flow as if it had no hint. `in` naming a node that isn't a group places it
/// next to that node instead. Next to something placed off the grid (a cell, a tree node), it
/// is placed off the grid too, lined up with it.
pub(super) fn spot(layer: &Layer, placement: &Placement, width: u32, height: u32) -> Spot {
    match placement {
        Placement::Auto | Placement::Child { .. } => auto_spot(layer, width, height),
        Placement::Near { target, direction } => {
            near_spot(layer, target, *direction, width, height)
        }
        Placement::Inside { group, near } => {
            let Ok((_, Some(group_span))) = layer.node_rect(group) else {
                return auto_spot(layer, width, height);
            };
            let is_group = matches!(
                layer.element(group).map(|element| &element.kind),
                Some(ElementKind::Node {
                    shape: NodeShape::Group,
                    ..
                })
            );
            if !is_group {
                return near_spot(layer, group, Direction::Right, width, height);
            }
            let cols = layout::columns_for(width);
            let beside = near.as_ref().and_then(|(target, direction)| {
                Some((layer.node_rect(target).ok()?.1?, *direction))
            });
            let (start, direction) = match beside {
                Some((span, direction)) => {
                    (layout::start_next_to(span, cols, direction), direction)
                }
                None => (group_span.cell, Direction::Right),
            };
            Spot::Grid {
                cell: layer.grid.find(start, cols, direction, Some(group)),
                cols,
                group: Some(group.clone()),
            }
        }
    }
}

/// Continue the flow: right of the last node placed on the layer, or the origin.
fn auto_spot(layer: &Layer, width: u32, height: u32) -> Spot {
    let last = layer
        .last_node()
        .and_then(|last| layer.node_rect(&last.id).ok());
    match last {
        Some((_, Some(span))) => {
            let start = layout::start_next_to(span, layout::columns_for(width), Direction::Right);
            grid_spot(layer, start, width, height, Direction::Right)
        }
        Some((rect, None)) => Spot::Free {
            rect: free::beside(layer, rect, width, height, Direction::Right),
        },
        None => grid_spot(
            layer,
            layout::Cell { row: 0, col: 0 },
            width,
            height,
            Direction::Right,
        ),
    }
}

/// Next to `target` in `direction`: on the grid when the target is on it, lined up with it
/// off the grid when it isn't, and the flow when the target isn't on the layer.
fn near_spot(
    layer: &Layer,
    target: &ElementId,
    direction: Direction,
    width: u32,
    height: u32,
) -> Spot {
    match layer.node_rect(target) {
        Ok((_, Some(span))) => {
            let start = layout::start_next_to(span, layout::columns_for(width), direction);
            grid_spot(layer, start, width, height, direction)
        }
        Ok((rect, None)) => Spot::Free {
            rect: free::beside(layer, rect, width, height, direction),
        },
        Err(_) => auto_spot(layer, width, height),
    }
}

/// The first free grid block at or beyond `start` along `direction`.
fn grid_spot(
    layer: &Layer,
    start: layout::Cell,
    width: u32,
    height: u32,
    direction: Direction,
) -> Spot {
    let cols = layout::columns_for(width);
    let rows = layout::rows_for(height);
    Spot::Grid {
        cell: layer.grid.find_block(start, cols, rows, direction),
        cols,
        group: None,
    }
}

/// The path of the edge `from>to`, or `to>from` walked backwards; an arrow you drew between the
/// two counts too.
pub(super) fn edge_between(layer: &Layer, from: &ElementId, to: &ElementId) -> Option<EdgePath> {
    layer.elements.iter().find_map(|element| {
        let (
            ElementKind::Edge { from: a, to: b, .. }
            | ElementKind::User {
                mark:
                    UserMark::Arrow {
                        from: Some(a),
                        to: Some(b),
                    },
                ..
            },
            Geometry::Path(path),
        ) = (&element.kind, &element.geometry)
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
