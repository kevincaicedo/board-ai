//! The board (LLD §2.5): a stack of layers, each a list of elements with stable ids.
//!
//! An element enters the board when its op becomes due: layout places it (`Pending`), it starts
//! drawing at its slot (`Drawing`), and it is `Drawn` once its budget has passed. An interruption
//! freezes the stroke in flight (`Frozen`) and removes anything still `Pending`, which the user
//! never saw. A repeated id updates the element (a strike, or finishing a frozen stroke) and
//! never duplicates it. Layout places only new ids, so nothing on the board moves (I6); the one
//! exception is a group, whose box only grows to keep its children inside.

pub mod excalidraw;
pub mod layout;
pub mod metrics;
mod place;
pub mod summary;
mod tone;

use std::collections::BTreeMap;

use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{ELEMENTS_PER_LAYER_MAX, LAYERS_PER_SESSION_MAX};
use crate::narration::ops::Placement;
use crate::protocol::{DrawOp, EdgePath, Fill, LineStyle, Mark, NodeShape};
use crate::time::SessionTime;
use layout::{CellSpan, Grid};
use place::{edge_between, node_cell, repeated, strike};
use tone::Tones;

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
        span: CellSpan,
        fill: Option<Fill>,
    },
    Edge {
        from: ElementId,
        to: ElementId,
        style: LineStyle,
    },
    Annotation {
        target: ElementId,
        mark: Mark,
    },
    Trace,
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
    NoEdge { from: ElementId, to: ElementId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    id: LayerId,
    title: String,
    open: bool,
    elements: Vec<Element>,
    index: BTreeMap<ElementId, usize>,
    grid: Grid,
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
        self.index.insert(element.id.clone(), self.elements.len());
        self.elements.push(element);
        Ok(())
    }

    fn node_rect(&self, id: &ElementId) -> Result<(Rect, CellSpan), Rejected> {
        let element = self
            .element(id)
            .filter(|element| element.state != ElementState::Pending)
            .ok_or_else(|| Rejected::UnknownElement(id.clone()))?;
        match (&element.kind, &element.geometry) {
            (ElementKind::Node { span, .. }, Geometry::Box(rect)) => Ok((*rect, *span)),
            (ElementKind::Node { .. }, Geometry::Path(_) | Geometry::Points(_))
            | (ElementKind::Edge { .. } | ElementKind::Annotation { .. } | ElementKind::Trace, _) => {
                Err(Rejected::UnknownElement(id.clone()))
            }
        }
    }

    /// The last node laid out, where the flow continues for a node with no placement.
    fn last_node(&self) -> Option<&Element> {
        self.elements
            .iter()
            .rev()
            .find(|element| matches!(element.kind, ElementKind::Node { group: None, .. }))
    }

    /// Bounds of everything the user can see on this layer.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.elements
            .iter()
            .filter(|element| element.state != ElementState::Pending)
            .map(|element| element.geometry.bounds())
            .reduce(Rect::union)
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
        let fill = self.tones.pick(layer_id, node.shape, node.label, node.fill);
        let layer = self.layer_mut(layer_id)?;
        let (width, height) = layout::node_size(node.shape, node.label);
        let cols = layout::columns_for(width);
        let (cell, group) = node_cell(layer, node.placement, cols)?;
        let span = CellSpan { cell, cols };
        let rect = layout::rect_in(span, width, height);
        if node.shape == NodeShape::Group {
            layer.grid.reserve(cell, node.id);
        } else {
            layer.grid.occupy(span, node.id);
        }
        let element = Element {
            id: node.id.clone(),
            op,
            kind: ElementKind::Node {
                shape: node.shape,
                stack: node.stack,
                group,
                span,
                fill,
            },
            label: node.label.to_owned(),
            geometry: Geometry::Box(rect),
            state: ElementState::Pending,
        };
        let draw = node_draw(&element, rect);
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
        let layer = self.layer_mut(layer_id)?;
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
                    | ElementKind::Trace,
                    _,
                ) => None,
            })
            .collect();
        let path = layout::edge_path(from_rect, to_rect, &obstacles);
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

    pub fn place_annotation(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        target: &ElementId,
        mark: Mark,
        text: Option<&str>,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let target_bounds = layer
            .element(target)
            .filter(|element| element.state != ElementState::Pending)
            .map(|element| element.geometry.bounds())
            .ok_or_else(|| Rejected::UnknownElement(target.clone()))?;
        let rect = layout::inflate(target_bounds, layout::ANNOTATION_MARGIN, 0);
        let id = generated_id("a", op);
        let draw = DrawOp::Annotate {
            id: id.clone(),
            op,
            target: target.clone(),
            mark,
            rect,
            text: text.map(str::to_owned),
        };
        layer.insert(Element {
            id: id.clone(),
            op,
            kind: ElementKind::Annotation {
                target: target.clone(),
                mark,
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

    /// The element's op starts now. Returns the redraw of its group when the group had to grow.
    pub fn start(
        &mut self,
        layer_id: LayerId,
        id: &ElementId,
        now: SessionTime,
        budget_ms: u32,
    ) -> Option<DrawOp> {
        let layer = self.layer_mut(layer_id).ok()?;
        let element = layer.element_mut(id)?;
        element.state = ElementState::Drawing {
            started_at: now,
            budget_ms,
        };
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
        let needed = layout::inflate(child, layout::GROUP_PADDING, layout::GROUP_LABEL_SPACE);
        let grown = old.union(needed);
        if grown == old {
            return None;
        }
        group_element.geometry = Geometry::Box(grown);
        Some(node_draw(group_element, grown))
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

    /// Removes an element that was laid out but never started; the user never saw it.
    pub fn remove_pending(&mut self, layer_id: LayerId, id: &ElementId) {
        let Ok(layer) = self.layer_mut(layer_id) else {
            return;
        };
        if layer.element(id).map(|element| element.state) != Some(ElementState::Pending) {
            return;
        }
        layer.elements.retain(|element| element.id != *id);
        layer.grid.release(id);
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

    /// Freezes the stroke in flight on `layer_id`: the most recently started one still within
    /// its budget. Any older stroke still drawing is treated as finished.
    pub fn freeze(&mut self, layer_id: LayerId, now: SessionTime) -> Option<FrozenOp> {
        self.settle(now);
        let layer = self.layer_mut(layer_id).ok()?;
        let latest = layer
            .elements
            .iter()
            .enumerate()
            .filter_map(|(index, element)| match element.state {
                ElementState::Drawing { started_at, .. } => Some((started_at, element.op, index)),
                ElementState::Pending
                | ElementState::Drawn
                | ElementState::Frozen { .. }
                | ElementState::Struck => None,
            })
            .max()
            .map(|(_, _, index)| index)?;
        for (index, element) in layer.elements.iter_mut().enumerate() {
            let ElementState::Drawing {
                started_at,
                budget_ms,
            } = element.state
            else {
                continue;
            };
            if index != latest {
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
        let element = layer.elements.get(latest)?;
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

    /// Restarts the frozen stroke on `layer_id`; returns the budget it has left.
    pub fn resume_frozen(&mut self, layer_id: LayerId, now: SessionTime) -> Option<u32> {
        let layer = self.layer_mut(layer_id).ok()?;
        let element = layer
            .elements
            .iter_mut()
            .find(|element| matches!(element.state, ElementState::Frozen { .. }))?;
        let ElementState::Frozen { remaining_ms, .. } = element.state else {
            return None;
        };
        element.state = ElementState::Drawing {
            started_at: now,
            budget_ms: remaining_ms,
        };
        Some(remaining_ms)
    }
}

/// A node to lay out.
#[derive(Clone, Copy, Debug)]
pub struct NodeRequest<'a> {
    pub id: &'a ElementId,
    pub label: &'a str,
    pub shape: NodeShape,
    pub placement: &'a Placement,
    pub stack: u8,
    /// The talker's tone; the board picks one when it is `None` (and for shapes with no fill
    /// ignores it).
    pub fill: Option<Fill>,
}

/// An edge to lay out.
#[derive(Clone, Copy, Debug)]
pub struct EdgeRequest<'a> {
    pub from: &'a ElementId,
    pub to: &'a ElementId,
    pub label: Option<&'a str>,
    pub style: LineStyle,
}

#[allow(
    clippy::expect_used,
    reason = "\"<letter>:<digits>\" is short and uses only letters, digits and ':'"
)]
fn generated_id(prefix: &str, op: OpId) -> ElementId {
    ElementId::parse(&format!("{prefix}:{}", op.0)).expect("generated ids are valid")
}

fn node_draw(element: &Element, rect: Rect) -> DrawOp {
    let (shape, stack, fill) = match element.kind {
        ElementKind::Node {
            shape, stack, fill, ..
        } => (shape, stack, fill),
        ElementKind::Edge { .. } | ElementKind::Annotation { .. } | ElementKind::Trace => {
            (NodeShape::Note, 1, None)
        }
    };
    DrawOp::Node {
        id: element.id.clone(),
        op: element.op,
        shape,
        label: element.label.clone(),
        rect,
        stack,
        fill,
    }
}
