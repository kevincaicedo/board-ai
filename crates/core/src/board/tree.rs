//! Trees, top-down (D-150). A child goes one level under its parent, joined to it by a line
//! from the parent's bottom centre to its own top centre, drawn with it. A binary tree's left
//! and right children spread by half as much at each level (160, 80, 40 units, then no
//! narrower), so a tree of four levels stays clear of itself without moving anything already
//! drawn; children with no side line up left to right under their parent. A child that would
//! land on something is nudged outward (I6: nothing drawn moves to make room).

use crate::board::{
    Board, Element, ElementKind, ElementState, Geometry, Layer, NodeRequest, Placed, Rejected,
    free, generated_id, layout, node_draw,
};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::ELEMENTS_PER_LAYER_MAX;
use crate::narration::ops::{Direction, Side};
use crate::protocol::{EdgePath, LineStyle, NodeShape};

/// Vertical space between a parent's bottom and its children's tops.
const LEVEL_GAP: i32 = 56;

/// How far a root's left and right children sit from it, centre to centre; each level halves it.
const SPREAD_ROOT: i32 = 160;

/// The narrowest spread, below which siblings would touch.
const SPREAD_MIN: i32 = 40;

/// Space between children lined up under their parent.
const SIBLING_GAP: i32 = 24;

impl Board {
    /// Places `node` as a child of `parent`. `None` when the parent isn't on the layer, so the
    /// node continues the flow instead (a hint naming nothing never loses the drawing).
    pub(super) fn place_child(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        node: NodeRequest<'_>,
        parent: &ElementId,
        side: Option<Side>,
    ) -> Result<Option<Placed>, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let Some((parent_rect, parent_shape)) = parent_node(layer, parent) else {
            return Ok(None);
        };
        let shape = node.shape.unwrap_or(match parent_shape {
            NodeShape::Group | NodeShape::Actor | NodeShape::Note => NodeShape::Service,
            NodeShape::Service | NodeShape::Db | NodeShape::Queue | NodeShape::Circle => {
                parent_shape
            }
        });
        let (width, height) = layout::node_size(shape, node.label);
        let rect = child_rect(layer, (parent, parent_rect), side, (width, height));
        if layer.elements().len() + 2 > ELEMENTS_PER_LAYER_MAX {
            return Err(Rejected::LayerFull);
        }
        let link = EdgePath {
            start: Point {
                x: parent_rect.center().x,
                y: parent_rect.bottom(),
            },
            control: None,
            end: Point {
                x: rect.center().x,
                y: rect.y,
            },
        };
        let fill = self.tones.pick(layer_id, shape, node.label, node.fill);
        let layer = self.layer_mut(layer_id)?;
        let element = Element {
            id: node.id.clone(),
            op,
            kind: ElementKind::Node {
                shape,
                stack: node.stack,
                group: None,
                span: None,
                fill,
                parent: Some(parent.clone()),
            },
            label: node.label.to_owned(),
            geometry: Geometry::Box(rect),
            state: ElementState::Pending,
        };
        let draw = node_draw(&element, rect, Some(link));
        let line_id = ElementId::parse(&format!("{}>{}", parent.as_str(), node.id.as_str()))
            .unwrap_or_else(|| generated_id("e", op));
        layer.insert(element)?;
        layer.grid.occupy_rect(rect, node.id);
        layer.insert(Element {
            id: line_id,
            op,
            kind: ElementKind::Edge {
                from: parent.clone(),
                to: node.id.clone(),
                style: LineStyle::Solid,
            },
            label: String::new(),
            geometry: Geometry::Path(link),
            state: ElementState::Pending,
        })?;
        Ok(Some(Placed::New {
            id: node.id.clone(),
            draw,
        }))
    }
}

/// The parent's box and kind, once it is drawn.
fn parent_node(layer: &Layer, parent: &ElementId) -> Option<(Rect, NodeShape)> {
    let element = layer
        .element(parent)
        .filter(|element| element.state != ElementState::Pending)?;
    if let (ElementKind::Node { shape, .. }, Geometry::Box(rect)) =
        (&element.kind, &element.geometry)
    {
        Some((*rect, *shape))
    } else {
        None
    }
}

/// Where a `width` × `height` child of `parent` goes: one level under it, on its side (or after
/// its last child), nudged outward past anything in the way.
fn child_rect(
    layer: &Layer,
    (parent, parent_rect): (&ElementId, Rect),
    side: Option<Side>,
    (width, height): (u32, u32),
) -> Rect {
    let half = i32::try_from(width / 2).unwrap_or(0);
    let depth = depth_of(layer.elements(), parent);
    let center_x = match side {
        Some(Side::Left) => parent_rect.center().x - spread(depth),
        Some(Side::Right) => parent_rect.center().x + spread(depth),
        None => last_child(layer.elements(), parent).map_or(parent_rect.center().x, |last| {
            last.right() + SIBLING_GAP + half
        }),
    };
    let candidate = Rect {
        x: center_x - half,
        y: parent_rect.bottom() + LEVEL_GAP,
        w: width,
        h: height,
    };
    let outward = match side {
        Some(Side::Left) => Direction::Left,
        Some(Side::Right) | None => Direction::Right,
    };
    free::clear(layer, candidate, outward)
}

/// How far a child of a node at `depth` (the root is 0) sits from it.
fn spread(depth: u32) -> i32 {
    (SPREAD_ROOT >> depth.min(8)).max(SPREAD_MIN)
}

/// How many parents up from `id` the root is. Each step goes to an element laid out earlier, so
/// the walk ends within the layer's element count.
fn depth_of(elements: &[Element], id: &ElementId) -> u32 {
    let mut depth = 0;
    let mut current = id.clone();
    for _ in 0..elements.len() {
        let parent = elements.iter().find_map(|element| {
            if let ElementKind::Node {
                parent: Some(parent),
                ..
            } = &element.kind
                && element.id == current
            {
                Some(parent.clone())
            } else {
                None
            }
        });
        let Some(parent) = parent else {
            break;
        };
        depth += 1;
        current = parent;
    }
    depth
}

/// The box of the last child laid out under `parent`.
fn last_child(elements: &[Element], parent: &ElementId) -> Option<Rect> {
    elements.iter().rev().find_map(|element| {
        if let ElementKind::Node {
            parent: Some(of), ..
        } = &element.kind
            && of == parent
        {
            Some(element.geometry.bounds())
        } else {
            None
        }
    })
}
