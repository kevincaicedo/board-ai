//! Laying out the incremental ops of a quick sketch: nodes, edges, pen marks, strikes and traces
//! (LLD §2.5). Each places only new ids, so nothing drawn moves (I6).

use crate::board::place::{Spot, edge_between, repeated, spot, strike};
use crate::board::{
    Board, Element, ElementKind, ElementState, Geometry, Placed, Rejected, UserMark, layout, route,
    user,
};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::narration::ops::Placement;
use crate::protocol::{DrawOp, EdgePath, Fill, LineStyle, Mark, NodeShape};
use layout::CellSpan;

/// A highlight's tone when the talker names none.
const HIGHLIGHT_FILL: Fill = Fill::Blush;

impl Board {
    pub fn place_node(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        node: NodeRequest<'_>,
    ) -> Result<Placed, Rejected> {
        if let Some(existing) = self.layer_mut(layer_id)?.element(node.id) {
            user::not_yours(existing)?;
            return Ok(repeated(existing, op, node.label));
        }
        if let Placement::Child { parent, side } = node.placement
            && let Some(placed) = self.place_child(layer_id, op, node, parent, *side)?
        {
            return Ok(placed);
        }
        if let Some(placed) = self.place_below_circle(layer_id, op, node)? {
            return Ok(placed);
        }
        let shape = node.shape.unwrap_or(NodeShape::Service);
        let fill = self.tones.pick(layer_id, shape, node.label, node.fill);
        let layer = self.layer_mut(layer_id)?;
        let (width, mut height) = layout::node_size(shape, node.label);
        // A box beside a cell (a hash chain's entry, a list's next node) is no taller than what it
        // hangs from, so entries for neighbouring buckets line up row by row.
        if let Placement::Near { target, .. } = node.placement
            && let Ok((beside, None)) = layer.node_rect(target)
            && matches!(shape, NodeShape::Service | NodeShape::Queue)
        {
            height = height.min(beside.h);
        }
        let (rect, span, group) = match spot(layer, node.placement, width, height) {
            Spot::Grid { cell, cols, group } => {
                let span = CellSpan { cell, cols };
                if shape == NodeShape::Group {
                    // Every cell its frame covers waits for its members, so nothing else is
                    // placed on the frame.
                    for col in cell.col..cell.col + cols {
                        layer.grid.reserve(layout::Cell { col, ..cell }, node.id);
                    }
                } else {
                    layer.grid.occupy(span, node.id);
                }
                (layout::rect_in(span, width, height), Some(span), group)
            }
            Spot::Free { rect } => {
                layer.grid.occupy_rect(rect, node.id);
                (rect, None, None)
            }
        };
        let element = Element {
            id: node.id.clone(),
            op: Some(op),
            kind: ElementKind::Node {
                shape,
                stack: node.stack,
                group,
                span,
                fill,
                parent: None,
            },
            label: node.label.to_owned(),
            geometry: Geometry::Box(rect),
            state: ElementState::Pending,
        };
        let draw = node_draw(&element, op, rect, None);
        layer.insert(element)?;
        Ok(Placed::New {
            id: node.id.clone(),
            draw,
        })
    }

    pub fn place_edge(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        edge: EdgeRequest<'_>,
    ) -> Result<Placed, Rejected> {
        // Between two lifelines an edge is a message: models draw them with `edge` too.
        let layer = self.layer_mut(layer_id)?;
        if layer.is_participant(edge.from) && layer.is_participant(edge.to) {
            return self.place_message(layer_id, op, edge);
        }
        let id = ElementId::parse(&format!("{}>{}", edge.from.as_str(), edge.to.as_str()))
            .or_else(|| ElementId::parse(&format!("e:{}", op.0)))
            .ok_or_else(|| Rejected::UnknownElement(edge.from.clone()))?;
        if let Some(existing) = layer.element(&id) {
            return Ok(match edge.label {
                Some(label) => repeated(existing, op, label),
                None => Placed::Unchanged,
            });
        }
        let (from_rect, _) = layer.node_rect(edge.from)?;
        let (to_rect, _) = layer.node_rect(edge.to)?;
        let obstacles: Vec<Rect> = layer
            .elements
            .iter()
            .filter(|element| element.id != *edge.from && element.id != *edge.to)
            .filter_map(|element| match (&element.kind, &element.geometry) {
                (ElementKind::Node { shape, .. }, Geometry::Box(rect))
                    if *shape != NodeShape::Group =>
                {
                    Some(*rect)
                }
                // An edge bows around your shapes as it does around nodes.
                (
                    ElementKind::User {
                        mark: UserMark::Box | UserMark::Ink { .. },
                        ..
                    },
                    Geometry::Box(rect),
                ) => Some(*rect),
                (
                    ElementKind::Node { .. }
                    | ElementKind::Edge { .. }
                    | ElementKind::Annotation { .. }
                    | ElementKind::Trace
                    | ElementKind::Participant { .. }
                    | ElementKind::Message { .. }
                    | ElementKind::Cells { .. }
                    | ElementKind::Cell { .. }
                    | ElementKind::User { .. },
                    _,
                ) => None,
            })
            .collect();
        let path = route::edge_path(from_rect, to_rect, &obstacles);
        let draw = DrawOp::Edge {
            id: id.clone(),
            op,
            from: edge.from.clone(),
            to: edge.to.clone(),
            path,
            label: edge.label.map(str::to_owned),
            style: edge.style,
        };
        layer.insert(Element {
            id: id.clone(),
            op: Some(op),
            kind: ElementKind::Edge {
                from: edge.from.clone(),
                to: edge.to.clone(),
                style: edge.style,
            },
            label: edge.label.unwrap_or_default().to_owned(),
            geometry: Geometry::Path(path),
            state: ElementState::Pending,
        })?;
        Ok(Placed::New { id, draw })
    }

    /// A pen mark around or under the target; a highlight hachures the target itself, in
    /// `fill` or the highlight tone.
    pub fn place_annotation(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        mark: AnnotationRequest<'_>,
    ) -> Result<Placed, Rejected> {
        let AnnotationRequest {
            target,
            mark,
            text,
            fill,
        } = mark;
        let layer = self.layer_mut(layer_id)?;
        let target_bounds = layer
            .element(target)
            .filter(|element| element.state.is_shown())
            .map(|element| element.geometry.bounds())
            .ok_or_else(|| Rejected::UnknownElement(target.clone()))?;
        let (rect, fill) = match mark {
            Mark::Highlight => (target_bounds, Some(fill.unwrap_or(HIGHLIGHT_FILL))),
            Mark::Circle | Mark::Underline | Mark::Arrow | Mark::Bracket => (
                layout::inflate(target_bounds, layout::ANNOTATION_MARGIN, 0),
                None,
            ),
        };
        let id = generated_id("a", op);
        let draw = DrawOp::Annotate {
            id: id.clone(),
            op,
            target: target.clone(),
            mark,
            rect,
            text: text.map(str::to_owned),
            fill,
        };
        layer.insert(Element {
            id: id.clone(),
            op: Some(op),
            kind: ElementKind::Annotation {
                target: target.clone(),
                mark,
                fill,
            },
            label: text.unwrap_or_default().to_owned(),
            geometry: Geometry::Box(rect),
            state: ElementState::Pending,
        })?;
        Ok(Placed::New { id, draw })
    }

    /// `strike target text`: strike through the target's label and write the new one.
    pub fn place_strike(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        target: &ElementId,
        text: &str,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let existing = layer
            .element(target)
            .filter(|element| element.state.is_shown())
            .ok_or_else(|| Rejected::UnknownElement(target.clone()))?;
        user::not_yours(existing)?;
        Ok(strike(existing, op, text))
    }

    pub fn place_trace(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        hops: &[ElementId],
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let mut points: Vec<Point> = Vec::new();
        for pair in hops.windows(2) {
            let (from, to) = (&pair[0], &pair[1]);
            let path = edge_between(layer, from, to).ok_or_else(|| Rejected::NoEdge {
                from: from.clone(),
                to: to.clone(),
            })?;
            for point in [Some(path.start), path.control, Some(path.end)]
                .into_iter()
                .flatten()
            {
                if points.last() != Some(&point) {
                    points.push(point);
                }
            }
        }
        let id = generated_id("t", op);
        let draw = DrawOp::Trace {
            id: id.clone(),
            op,
            points: points.clone(),
        };
        layer.insert(Element {
            id: id.clone(),
            op: Some(op),
            kind: ElementKind::Trace,
            label: String::new(),
            geometry: Geometry::Points(points),
            state: ElementState::Pending,
        })?;
        Ok(Placed::New { id, draw })
    }
}

/// A node to lay out.
#[derive(Clone, Copy, Debug)]
pub struct NodeRequest<'a> {
    pub id: &'a ElementId,
    pub label: &'a str,
    /// The talker's kind: a tree child without one takes its parent's, anything else is a
    /// service.
    pub shape: Option<NodeShape>,
    pub placement: &'a Placement,
    pub stack: u8,
    /// The talker's tone; the board picks one when it is `None` (and for shapes with no fill
    /// ignores it).
    pub fill: Option<Fill>,
}

/// An edge to lay out (or a message, between two participants).
#[derive(Clone, Copy, Debug)]
pub struct EdgeRequest<'a> {
    pub from: &'a ElementId,
    pub to: &'a ElementId,
    pub label: Option<&'a str>,
    pub style: LineStyle,
}

/// A mark to make on an element.
#[derive(Clone, Copy, Debug)]
pub struct AnnotationRequest<'a> {
    pub target: &'a ElementId,
    pub mark: Mark,
    pub text: Option<&'a str>,
    /// A highlight's tone; ignored for a pen mark.
    pub fill: Option<Fill>,
}

#[allow(
    clippy::expect_used,
    reason = "\"<letter>:<digits>\" is short and uses only letters, digits and ':'"
)]
pub(super) fn generated_id(prefix: &str, op: OpId) -> ElementId {
    ElementId::parse(&format!("{prefix}:{}", op.0)).expect("generated ids are valid")
}

/// The `Node` op that draws `element` (a node) under `op` at `rect`.
pub(super) fn node_draw(element: &Element, op: OpId, rect: Rect, link: Option<EdgePath>) -> DrawOp {
    let (shape, stack, fill) = match element.kind {
        ElementKind::Node {
            shape, stack, fill, ..
        } => (shape, stack, fill),
        ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Participant { .. }
        | ElementKind::Message { .. }
        | ElementKind::Cells { .. }
        | ElementKind::Cell { .. }
        | ElementKind::User { .. } => (NodeShape::Note, 1, None),
    };
    DrawOp::Node {
        id: element.id.clone(),
        op,
        shape,
        label: element.label.clone(),
        rect,
        stack,
        fill,
        link,
    }
}
