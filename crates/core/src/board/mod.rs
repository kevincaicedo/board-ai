//! The board (LLD §2.5): a stack of layers, each a list of elements with stable ids.
//!
//! An element enters the board when its op becomes due: layout places it (`Pending`), it starts
//! drawing at its slot (`Drawing`), and it is `Drawn` once its budget has passed. An interruption
//! freezes the stroke in flight (`Frozen`) and removes anything still `Pending`, which the user
//! never saw. A repeated id updates the element (a strike, or finishing a frozen stroke) and
//! never duplicates it. Layout places only new ids, so nothing on the board moves (I6); the
//! exceptions only grow: a group's box, to keep its children inside, and a lifeline, as messages
//! go down a sequence diagram.
//!
//! One op can make several elements (a row of cells and each cell, a tree node and the line from
//! its parent). They share the op, and move through the states together.

mod cells;
pub mod excalidraw;
mod free;
pub mod layout;
mod lifecycle;
pub mod metrics;
mod place;
mod route;
mod sequence;
pub mod summary;
mod tone;
mod tree;

use std::collections::BTreeMap;

use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{ELEMENTS_PER_LAYER_MAX, LAYERS_PER_SESSION_MAX};
use crate::narration::ops::Placement;
use crate::protocol::{DrawOp, EdgePath, Fill, LineStyle, Mark, NodeShape};
use crate::time::SessionTime;
use layout::{CellSpan, Grid};
use place::{Spot, edge_between, repeated, spot, strike};
use sequence::{RowCommit, Sequence};
use tone::Tones;

/// A highlight's tone when the talker names none.
const HIGHLIGHT_FILL: Fill = Fill::Blush;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: ElementId,
    /// The op that drew it; a group's growth redraws under the same op.
    pub op: OpId,
    pub kind: ElementKind,
    pub label: String,
    pub geometry: Geometry,
    pub state: ElementState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementKind {
    Node {
        shape: NodeShape,
        stack: u8,
        group: Option<ElementId>,
        /// Its grid cells; `None` for a node placed off the grid (a tree node, a node beside a
        /// cell), whose neighbours are placed off the grid too.
        span: Option<CellSpan>,
        fill: Option<Fill>,
        /// Its parent, for a tree node.
        parent: Option<ElementId>,
    },
    Edge {
        from: ElementId,
        to: ElementId,
        style: LineStyle,
    },
    Annotation {
        target: ElementId,
        mark: Mark,
        /// A highlight's tone.
        fill: Option<Fill>,
    },
    Trace,
    /// A sequence diagram participant: the box is its head; its lifeline runs from the head's
    /// bottom centre down to `lifeline_bottom`, which only grows.
    Participant {
        span: CellSpan,
        lifeline_bottom: i32,
    },
    /// A message between lifelines; `commit` is what starting it does to the sequence.
    Message {
        from: ElementId,
        to: ElementId,
        style: LineStyle,
        commit: RowCommit,
    },
    /// A row or column of cells; `extent` adds its label and index labels to the box.
    Cells {
        vertical: bool,
        span: Option<CellSpan>,
        extent: Rect,
    },
    /// One cell of a row, at `index` from 0; its label is its value, `index_label` what is
    /// written beside it.
    Cell {
        strip: ElementId,
        index: u8,
        index_label: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Geometry {
    Box(Rect),
    Path(EdgePath),
    Points(Vec<Point>),
}

impl Geometry {
    #[must_use]
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Box(rect) => *rect,
            Self::Path(path) => layout::path_bounds(*path),
            Self::Points(points) => points
                .iter()
                .map(|point| Rect {
                    x: point.x,
                    y: point.y,
                    w: 1,
                    h: 1,
                })
                .reduce(Rect::union)
                .unwrap_or_default(),
        }
    }
}

impl Element {
    /// Everything the element covers on the board: its geometry, plus a participant's lifeline
    /// and a row of cells' labels.
    #[must_use]
    pub fn area(&self) -> Rect {
        match (&self.kind, &self.geometry) {
            (
                ElementKind::Participant {
                    lifeline_bottom, ..
                },
                Geometry::Box(head),
            ) => head.union(layout::lifeline_box(
                head.center().x,
                head.bottom(),
                *lifeline_bottom,
            )),
            (ElementKind::Cells { extent, .. }, _) => *extent,
            (
                ElementKind::Node { .. }
                | ElementKind::Edge { .. }
                | ElementKind::Annotation { .. }
                | ElementKind::Trace
                | ElementKind::Participant { .. }
                | ElementKind::Message { .. }
                | ElementKind::Cell { .. },
                _,
            ) => self.geometry.bounds(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementState {
    /// Laid out; its op waits for its drawing slot.
    Pending,
    Drawing {
        started_at: SessionTime,
        budget_ms: u32,
    },
    Drawn,
    /// Stopped mid-stroke by an interruption; `remaining_ms` is the budget left to finish it.
    Frozen {
        progress_percent: u8,
        remaining_ms: u32,
    },
    /// Its label was struck through and rewritten.
    Struck,
}

/// What laying out a drawing op produced.
#[derive(Clone, Debug, PartialEq)]
pub enum Placed {
    /// A new element, now `Pending`; start it with [`Board::start`].
    New { id: ElementId, draw: DrawOp },
    /// A repeated id with a new label: strike the old one ([`Board::apply_strike`] at start).
    Strike { target: ElementId, draw: DrawOp },
    /// A label for an element that has none (an empty cell): write it in
    /// ([`Board::apply_write`] at start).
    Write { target: ElementId, draw: DrawOp },
    /// Something the op needs drawn first (a participant a message names that nobody drew):
    /// start it like a new element, then lay the op out again at its next slot.
    Before { id: ElementId, draw: DrawOp },
    /// A repeated id of a frozen element: finish the frozen stroke instead of redrawing it.
    ResumeFrozen,
    /// A repeated id with nothing new.
    Unchanged,
}

/// Why a drawing op could not be laid out; the op is dropped and counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejected {
    LayerMissing,
    LayerFull,
    UnknownElement(ElementId),
    NoEdge {
        from: ElementId,
        to: ElementId,
    },
    /// An id a new element needs (a cell's `arr.3`) already names something else.
    IdTaken(ElementId),
    /// A message needs two participants, or one for a message to itself.
    NotParticipant(ElementId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    id: LayerId,
    title: String,
    open: bool,
    elements: Vec<Element>,
    index: BTreeMap<ElementId, usize>,
    grid: Grid,
    sequence: Sequence,
}

impl Layer {
    fn new(id: LayerId, title: String) -> Self {
        Self {
            id,
            title,
            open: true,
            elements: Vec::new(),
            index: BTreeMap::new(),
            grid: Grid::default(),
            sequence: Sequence::default(),
        }
    }

    #[must_use]
    pub fn id(&self) -> LayerId {
        self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Elements in the order they were laid out.
    #[must_use]
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    #[must_use]
    pub fn element(&self, id: &ElementId) -> Option<&Element> {
        self.index
            .get(id)
            .and_then(|&index| self.elements.get(index))
    }

    fn element_mut(&mut self, id: &ElementId) -> Option<&mut Element> {
        let index = *self.index.get(id)?;
        self.elements.get_mut(index)
    }

    fn insert(&mut self, element: Element) -> Result<(), Rejected> {
        if self.elements.len() >= ELEMENTS_PER_LAYER_MAX {
            return Err(Rejected::LayerFull);
        }
        if self.index.contains_key(&element.id) {
            return Err(Rejected::IdTaken(element.id));
        }
        self.index.insert(element.id.clone(), self.elements.len());
        self.elements.push(element);
        Ok(())
    }

    /// The box of something drawn that others can be placed beside or joined to, and its grid
    /// cells when it sits on the grid (`None` places its neighbours off the grid).
    fn node_rect(&self, id: &ElementId) -> Result<(Rect, Option<CellSpan>), Rejected> {
        let element = self
            .element(id)
            .filter(|element| element.state != ElementState::Pending)
            .ok_or_else(|| Rejected::UnknownElement(id.clone()))?;
        match (&element.kind, &element.geometry) {
            (
                ElementKind::Node { span, .. } | ElementKind::Cells { span, .. },
                Geometry::Box(rect),
            ) => Ok((*rect, *span)),
            (ElementKind::Cell { .. } | ElementKind::Participant { .. }, Geometry::Box(rect)) => {
                Ok((*rect, None))
            }
            (
                ElementKind::Node { .. }
                | ElementKind::Cells { .. }
                | ElementKind::Cell { .. }
                | ElementKind::Participant { .. },
                Geometry::Path(_) | Geometry::Points(_),
            )
            | (
                ElementKind::Edge { .. }
                | ElementKind::Annotation { .. }
                | ElementKind::Trace
                | ElementKind::Message { .. },
                _,
            ) => Err(Rejected::UnknownElement(id.clone())),
        }
    }

    /// The last node or row of cells laid out, where the flow continues for one with no
    /// placement.
    fn last_node(&self) -> Option<&Element> {
        self.elements.iter().rev().find(|element| {
            matches!(
                element.kind,
                ElementKind::Node { group: None, .. } | ElementKind::Cells { .. }
            )
        })
    }

    /// Bounds of everything the user can see on this layer.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.elements
            .iter()
            .filter(|element| element.state != ElementState::Pending)
            .map(Element::area)
            .reduce(Rect::union)
    }

    /// The elements one op made, in the order it made them.
    fn made_by(&self, op: OpId) -> impl Iterator<Item = &Element> {
        self.elements.iter().filter(move |element| element.op == op)
    }

    /// The frozen element, if a stroke on this layer was stopped mid-way.
    #[must_use]
    pub fn frozen(&self) -> Option<&Element> {
        self.elements
            .iter()
            .find(|element| matches!(element.state, ElementState::Frozen { .. }))
    }
}

/// A frozen stroke, as the interruption reports it to the talker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenOp {
    pub layer: LayerId,
    pub id: ElementId,
    pub label: String,
    pub progress_percent: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    layers: Vec<Layer>,
    tones: Tones,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            layers: vec![Layer::new(LayerId::MAIN, "main".to_owned())],
            tones: Tones::default(),
        }
    }

    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    #[must_use]
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(usize::from(id.0))
    }

    fn layer_mut(&mut self, id: LayerId) -> Result<&mut Layer, Rejected> {
        self.layers
            .get_mut(usize::from(id.0))
            .ok_or(Rejected::LayerMissing)
    }

    /// Opens a detour layer; `None` when the session has used every layer id.
    pub fn push_layer(&mut self, title: &str) -> Option<LayerId> {
        if self.layers.len() >= LAYERS_PER_SESSION_MAX {
            return None;
        }
        let id = LayerId(u8::try_from(self.layers.len()).ok()?);
        self.layers.push(Layer::new(id, title.to_owned()));
        Some(id)
    }

    /// Closes a detour layer. It stays in the board for the session export.
    pub fn close_layer(&mut self, id: LayerId) {
        if let Ok(layer) = self.layer_mut(id) {
            layer.open = false;
        }
    }

    pub fn place_node(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        node: NodeRequest<'_>,
    ) -> Result<Placed, Rejected> {
        if let Some(existing) = self.layer_mut(layer_id)?.element(node.id) {
            return Ok(repeated(existing, op, node.label));
        }
        if let Placement::Child { parent, side } = node.placement
            && let Some(placed) = self.place_child(layer_id, op, node, parent, *side)?
        {
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
            op,
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
        let draw = node_draw(&element, rect, None);
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
                (
                    ElementKind::Node { .. }
                    | ElementKind::Edge { .. }
                    | ElementKind::Annotation { .. }
                    | ElementKind::Trace
                    | ElementKind::Participant { .. }
                    | ElementKind::Message { .. }
                    | ElementKind::Cells { .. }
                    | ElementKind::Cell { .. },
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
            op,
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
            .filter(|element| element.state != ElementState::Pending)
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
            op,
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
            .filter(|element| element.state != ElementState::Pending)
            .ok_or_else(|| Rejected::UnknownElement(target.clone()))?;
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
            op,
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
fn generated_id(prefix: &str, op: OpId) -> ElementId {
    ElementId::parse(&format!("{prefix}:{}", op.0)).expect("generated ids are valid")
}

fn node_draw(element: &Element, rect: Rect, link: Option<EdgePath>) -> DrawOp {
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
        | ElementKind::Cell { .. } => (NodeShape::Note, 1, None),
    };
    DrawOp::Node {
        id: element.id.clone(),
        op: element.op,
        shape,
        label: element.label.clone(),
        rect,
        stack,
        fill,
        link,
    }
}
