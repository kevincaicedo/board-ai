//! Your shapes (artboard 13, LLD §2.5 "Your shapes"): what you draw with the pen, the box and the
//! arrow, kept on the board beside BoardAI's drawing with ids of their own (`u1`, `u2`…).
//!
//! - Geometry is kept exactly as drawn. The box and arrow tools give tidy shapes: a rectangle,
//!   and a straight arrow whose ends connect to the shapes they land on and sit on their
//!   outlines (`snap`). The pen keeps every stroke; a stroke that mostly overlaps the shape drawn
//!   just before joins it, so a cylinder's body and its lid are one shape.
//! - What you say labels the latest shape; a second phrase becomes its note.
//! - They are yours: BoardAI points at them, joins them and draws beside them, but layout never
//!   places anything on them, and an op that would redraw, strike or write over one is refused
//!   (`Rejected::Yours`). Only you remove one, by undoing it while you draw.

mod describe;
mod snap;

pub use describe::describe;

use crate::board::metrics::{LABEL_LINE_HEIGHT, SMALL_LABEL_FONT_SIZE, text_width};
use crate::board::{Board, Element, ElementKind, ElementState, Geometry, Layer, Rejected, layout};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId};
use crate::limits::{
    LABEL_MAX_BYTES, USER_COORDINATE_MAX, USER_SHAPE_MIN_UNITS, USER_SHAPE_SPAN_MAX,
    USER_SHAPE_STROKES_MAX, USER_SHAPES_PER_LAYER_MAX, USER_STROKE_POINTS_MAX,
};
use crate::narration::tags::bounded;
use crate::protocol::{DrawnShape, EdgePath, UserShape, UserShapeKind};

/// How one of your shapes is drawn; where it is lives in its element's geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserMark {
    /// The box tool's rectangle (the geometry's box).
    Box,
    /// The arrow tool's line (the geometry's path) and the shapes its ends connect to.
    Arrow {
        from: Option<ElementId>,
        to: Option<ElementId>,
    },
    /// The pen's strokes, exactly as drawn; the geometry's box bounds them.
    Ink { strokes: Vec<Vec<Point>> },
}

/// Font size of the note written under one of your shapes (artboard 13's "source of truth").
pub const NOTE_FONT_SIZE: u32 = 17;

/// Where a note's middle sits below its shape, left-aligned with it (artboard 13).
pub const NOTE_BELOW: i32 = 18;

/// Where an arrow's label sits above its middle (the canvas's edge label gap).
pub const ARROW_LABEL_ABOVE: i32 = 20;

/// A pen stroke joins the shape drawn just before when the two boxes share at least this much
/// of the smaller one's area: a lid inside a cylinder's body joins it, an arrow leaving a box
/// does not.
const JOIN_OVERLAP_PERCENT: u64 = 60;

/// Room around a pen shape that still counts as on it, for a hand that misses by a little.
const JOIN_MARGIN: i32 = 12;

/// A shape the UI sent, parsed at the boundary: coordinates in range, a bounded point count, and
/// nothing smaller than a click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drawn {
    Pen(Vec<Point>),
    Box(Rect),
    Arrow { start: Point, end: Point },
}

/// Why a drawn shape was not kept. The UI's preview goes away when no echo comes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A pen stroke over `USER_STROKE_POINTS_MAX` points.
    TooManyPoints,
    /// A coordinate past `USER_COORDINATE_MAX`.
    OutOfRange,
    /// Wider or taller than `USER_SHAPE_SPAN_MAX`: the grid cells it would take grow with its
    /// area.
    TooLarge,
    /// Smaller than `USER_SHAPE_MIN_UNITS`: a click.
    TooSmall,
    /// The layer doesn't exist, is closed, or has `USER_SHAPES_PER_LAYER_MAX` of your shapes.
    Layer(Rejected),
}

impl Drawn {
    /// # Errors
    /// The shape is refused as [`Refused`] says.
    pub fn parse(shape: DrawnShape) -> Result<Self, Refused> {
        let drawn = match shape {
            DrawnShape::Pen { points } => {
                if points.len() > USER_STROKE_POINTS_MAX {
                    return Err(Refused::TooManyPoints);
                }
                Self::Pen(points)
            }
            DrawnShape::Box { rect } => Self::Box(rect),
            DrawnShape::Arrow { start, end } => Self::Arrow { start, end },
        };
        let points: Vec<Point> = match &drawn {
            Self::Pen(points) => points.clone(),
            Self::Box(rect) => vec![
                Point {
                    x: rect.x,
                    y: rect.y,
                },
                Point {
                    x: rect.right(),
                    y: rect.bottom(),
                },
            ],
            Self::Arrow { start, end } => vec![*start, *end],
        };
        let in_range = |value: i32| value.unsigned_abs() <= USER_COORDINATE_MAX.unsigned_abs();
        if !points
            .iter()
            .all(|point| in_range(point.x) && in_range(point.y))
        {
            return Err(Refused::OutOfRange);
        }
        let reach = match &drawn {
            Self::Box(rect) => *rect,
            Self::Pen(_) | Self::Arrow { .. } => points_rect(&points),
        };
        if reach.w > USER_SHAPE_SPAN_MAX || reach.h > USER_SHAPE_SPAN_MAX {
            return Err(Refused::TooLarge);
        }
        let big_enough = match &drawn {
            Self::Box(rect) => rect.w >= USER_SHAPE_MIN_UNITS && rect.h >= USER_SHAPE_MIN_UNITS,
            Self::Pen(_) | Self::Arrow { .. } => reach.w.max(reach.h) >= USER_SHAPE_MIN_UNITS,
        };
        if big_enough {
            Ok(drawn)
        } else {
            Err(Refused::TooSmall)
        }
    }
}

/// What undoing a shape did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undone {
    /// The shape is gone. Arrows of yours that joined it lost that end (it stays where it was
    /// drawn), and are given as the UI draws them now.
    Removed { disconnected: Vec<UserShape> },
    /// A pen shape lost its last stroke and keeps the others.
    Changed(UserShape),
}

/// What adding a shape changed, as the UI draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    /// The new shape, or the pen shape the stroke joined.
    pub shape: UserShape,
    /// Arrows of yours drawn before it whose loose end lands on it, now connected to it.
    pub connected: Vec<UserShape>,
}

impl Board {
    /// Adds a shape you drew to `layer_id`. A pen stroke that mostly overlaps `join` (the
    /// latest pen shape of this drawing) joins it instead. Returns the shape as the UI draws it.
    ///
    /// # Errors
    /// The layer is missing or closed, or already holds `USER_SHAPES_PER_LAYER_MAX` of yours.
    pub fn add_user_shape(
        &mut self,
        layer_id: LayerId,
        drawn: Drawn,
        join: Option<&ElementId>,
    ) -> Result<UserShape, Refused> {
        self.add_user_shape_connecting(layer_id, drawn, join)
            .map(|added| added.shape)
    }

    /// [`Board::add_user_shape`], also connecting your arrows drawn before the shape: an arrow
    /// is often drawn before what it points at, and a loose end that lands on the new shape joins
    /// it (where it was drawn, nothing moves).
    ///
    /// # Errors
    /// As [`Board::add_user_shape`].
    pub fn add_user_shape_connecting(
        &mut self,
        layer_id: LayerId,
        drawn: Drawn,
        join: Option<&ElementId>,
    ) -> Result<Added, Refused> {
        let layer = self.layer_mut(layer_id).map_err(Refused::Layer)?;
        if !layer.open {
            return Err(Refused::Layer(Rejected::LayerMissing));
        }
        let drawn = match (drawn, join) {
            (Drawn::Pen(points), Some(join)) => match join_stroke(layer, join, points) {
                Ok(shape) => {
                    let connected = connect_loose_ends(layer, &shape.id);
                    return Ok(Added { shape, connected });
                }
                Err(points) => Drawn::Pen(points),
            },
            (drawn, None | Some(_)) => drawn,
        };
        let yours = layer
            .elements
            .iter()
            .filter(|element| matches!(element.kind, ElementKind::User { .. }))
            .count();
        if yours >= USER_SHAPES_PER_LAYER_MAX {
            return Err(Refused::Layer(Rejected::LayerFull));
        }
        let id = self
            .next_user_id()
            .ok_or(Refused::Layer(Rejected::LayerFull))?;
        let layer = self.layer_mut(layer_id).map_err(Refused::Layer)?;
        let (mark, geometry) = match drawn {
            Drawn::Box(rect) => (UserMark::Box, Geometry::Box(rect)),
            Drawn::Pen(points) => {
                let bounds = points_rect(&points);
                (
                    UserMark::Ink {
                        strokes: vec![points],
                    },
                    Geometry::Box(bounds),
                )
            }
            Drawn::Arrow { start, end } => {
                let snapped = snap::snap(layer, start, end);
                let path = EdgePath {
                    start: snapped.start,
                    control: None,
                    end: snapped.end,
                };
                let mark = UserMark::Arrow {
                    from: snapped.from,
                    to: snapped.to,
                };
                (mark, Geometry::Path(path))
            }
        };
        let element = Element {
            id: id.clone(),
            op: None,
            kind: ElementKind::User { mark, note: None },
            label: String::new(),
            geometry,
            state: ElementState::Drawn,
        };
        let shape = user_shape(&element);
        layer.occupy_yours(&element);
        layer.insert(element).map_err(Refused::Layer)?;
        let shape = shape.ok_or(Refused::Layer(Rejected::UnknownElement(id)))?;
        let connected = connect_loose_ends(layer, &shape.id);
        Ok(Added { shape, connected })
    }

    /// Undoes your shape `id` on `layer_id`: a pen shape of several strokes loses its last one,
    /// anything else goes. A shape something of BoardAI's joins or marks (an edge, a message, an
    /// annotation, a member or child placed in it) stays: BoardAI's drawing would point at
    /// nothing. An arrow of yours that joined it only loses that end: an arrow is often drawn
    /// before what it points at, and refusing would leave undo stuck on that shape.
    pub fn undo_user_shape(&mut self, layer_id: LayerId, id: &ElementId) -> Option<Undone> {
        let layer = self.layer_mut(layer_id).ok()?;
        let element = layer.element_mut(id)?;
        let ElementKind::User { mark, .. } = &mut element.kind else {
            return None;
        };
        if let UserMark::Ink { strokes } = mark
            && strokes.len() > 1
        {
            strokes.pop();
            let bounds = strokes
                .iter()
                .map(|stroke| points_rect(stroke))
                .reduce(Rect::union)
                .unwrap_or_default();
            element.geometry = Geometry::Box(bounds);
            let changed = user_shape(element)?;
            layer.grid.release(id);
            let element = layer.element(id)?.clone();
            layer.occupy_yours(&element);
            return Some(Undone::Changed(changed));
        }
        if layer.is_used_by_boardai(id) {
            return None;
        }
        let disconnected = disconnect_arrows(layer, id);
        layer.elements.retain(|element| element.id != *id);
        layer.grid.release(id);
        layer.reindex();
        Some(Undone::Removed { disconnected })
    }

    /// Writes what you said onto your shape `id`: its label, or its note when it has a label.
    /// Returns the shape as the UI draws it now.
    pub fn label_user_shape(
        &mut self,
        layer_id: LayerId,
        id: &ElementId,
        words: &str,
    ) -> Option<UserShape> {
        let words = bounded(words, LABEL_MAX_BYTES);
        if words.is_empty() {
            return None;
        }
        let layer = self.layer_mut(layer_id).ok()?;
        let element = layer.element_mut(id)?;
        let ElementKind::User { note, .. } = &mut element.kind else {
            return None;
        };
        if element.label.is_empty() {
            element.label = words;
        } else {
            *note = Some(words);
        }
        let element = element.clone();
        layer.occupy_yours(&element);
        user_shape(&element)
    }

    /// Replaces the label of your shape `id` with what you said, keeping its note: a label said
    /// with a lead-in ("this is the cache") over one said alone. Returns the shape as the UI
    /// draws it now.
    pub fn relabel_user_shape(
        &mut self,
        layer_id: LayerId,
        id: &ElementId,
        words: &str,
    ) -> Option<UserShape> {
        let words = bounded(words, LABEL_MAX_BYTES);
        if words.is_empty() {
            return None;
        }
        let layer = self.layer_mut(layer_id).ok()?;
        let element = layer.element_mut(id)?;
        if !matches!(element.kind, ElementKind::User { .. }) {
            return None;
        }
        element.label = words;
        let element = element.clone();
        layer.occupy_yours(&element);
        user_shape(&element)
    }

    /// The next free id of yours, `u1`, `u2`…, unique across the session's layers (the talker
    /// may have used one): at most one try per element on the board, plus one.
    fn next_user_id(&mut self) -> Option<ElementId> {
        let elements: usize = self.layers.iter().map(|layer| layer.elements.len()).sum();
        for _ in 0..=elements {
            self.next_user = self.next_user.checked_add(1)?;
            let id = ElementId::parse(&format!("u{}", self.next_user))?;
            if self.layers.iter().all(|layer| layer.element(&id).is_none()) {
                return Some(id);
            }
        }
        None
    }
}

impl Layer {
    /// The grid cells your shape covers are taken, so grid placement keeps clear of it; an
    /// arrow is a connector and takes none.
    fn occupy_yours(&mut self, element: &Element) {
        if let ElementKind::User {
            mark: UserMark::Box | UserMark::Ink { .. },
            ..
        } = element.kind
        {
            self.grid.occupy_rect(element.area(), &element.id);
        }
    }

    /// Whether anything of BoardAI's on the layer is joined to or marks `id`.
    fn is_used_by_boardai(&self, id: &ElementId) -> bool {
        self.elements.iter().any(|element| match &element.kind {
            ElementKind::Edge { from, to, .. } | ElementKind::Message { from, to, .. } => {
                from == id || to == id
            }
            ElementKind::Annotation { target, .. } => target == id,
            ElementKind::Node { group, parent, .. } => {
                group.as_ref() == Some(id) || parent.as_ref() == Some(id)
            }
            ElementKind::Trace
            | ElementKind::Participant { .. }
            | ElementKind::Cells { .. }
            | ElementKind::Cell { .. }
            | ElementKind::User { .. } => false,
        })
    }

    /// Rebuilds the id index after elements were removed.
    pub(super) fn reindex(&mut self) {
        self.index = self
            .elements
            .iter()
            .enumerate()
            .map(|(index, element)| (element.id.clone(), index))
            .collect();
    }
}

/// Your arrows with a loose end on the box or pen shape `id` connect to it, unless the other end
/// is on it too. Returns them as the UI draws them now.
fn connect_loose_ends(layer: &mut Layer, id: &ElementId) -> Vec<UserShape> {
    let Some(Geometry::Box(bounds)) = layer.element(id).map(|element| element.geometry.clone())
    else {
        return Vec::new();
    };
    let reach = layout::inflate(bounds, snap::SNAP_UNITS, 0);
    let on = |point: Point| {
        reach.x <= point.x
            && point.x <= reach.right()
            && reach.y <= point.y
            && point.y <= reach.bottom()
    };
    let mut connected = Vec::new();
    for element in &mut layer.elements {
        let (
            ElementKind::User {
                mark: UserMark::Arrow { from, to },
                ..
            },
            Geometry::Path(path),
        ) = (&mut element.kind, &element.geometry)
        else {
            continue;
        };
        let mut changed = false;
        if from.is_none() && on(path.start) && to.as_ref() != Some(id) && !on(path.end) {
            *from = Some(id.clone());
            changed = true;
        }
        if to.is_none() && on(path.end) && from.as_ref() != Some(id) && !on(path.start) {
            *to = Some(id.clone());
            changed = true;
        }
        if changed {
            connected.extend(user_shape(element));
        }
    }
    connected
}

/// Your arrows joined to `id` lose that end; the end stays where it was drawn. Returns them as
/// the UI draws them now.
fn disconnect_arrows(layer: &mut Layer, id: &ElementId) -> Vec<UserShape> {
    let mut disconnected = Vec::new();
    for element in &mut layer.elements {
        let ElementKind::User {
            mark: UserMark::Arrow { from, to },
            ..
        } = &mut element.kind
        else {
            continue;
        };
        let mut changed = false;
        for end in [from, to] {
            if end.as_ref() == Some(id) {
                *end = None;
                changed = true;
            }
        }
        if changed {
            disconnected.extend(user_shape(element));
        }
    }
    disconnected
}

/// A pen stroke onto `join` when it mostly overlaps that pen shape; the points come back when
/// it doesn't join.
fn join_stroke(
    layer: &mut Layer,
    join: &ElementId,
    points: Vec<Point>,
) -> Result<UserShape, Vec<Point>> {
    let stroke = points_rect(&points);
    let Some(element) = layer.element_mut(join) else {
        return Err(points);
    };
    let (
        ElementKind::User {
            mark: UserMark::Ink { strokes },
            ..
        },
        Geometry::Box(bounds),
    ) = (&mut element.kind, &mut element.geometry)
    else {
        return Err(points);
    };
    if strokes.len() >= USER_SHAPE_STROKES_MAX || !mostly_overlaps(*bounds, stroke) {
        return Err(points);
    }
    strokes.push(points);
    *bounds = bounds.union(stroke);
    let element = element.clone();
    layer.occupy_yours(&element);
    user_shape(&element).ok_or_else(Vec::new)
}

/// The two boxes share at least [`JOIN_OVERLAP_PERCENT`] of the smaller one's area, with
/// [`JOIN_MARGIN`] of room around the shape.
fn mostly_overlaps(shape: Rect, stroke: Rect) -> bool {
    let shape = layout::inflate(shape, JOIN_MARGIN, 0);
    let left = shape.x.max(stroke.x);
    let top = shape.y.max(stroke.y);
    let right = shape.right().min(stroke.right());
    let bottom = shape.bottom().min(stroke.bottom());
    if right <= left || bottom <= top {
        return false;
    }
    let area = |rect: Rect| u64::from(rect.w.max(1)) * u64::from(rect.h.max(1));
    let shared = u64::from(right.abs_diff(left)) * u64::from(bottom.abs_diff(top));
    let smaller = area(shape).min(area(stroke));
    shared * 100 >= smaller * JOIN_OVERLAP_PERCENT
}

/// The smallest box holding every point, at least one unit each way.
#[must_use]
pub fn points_rect(points: &[Point]) -> Rect {
    let Some(first) = points.first() else {
        return Rect::default();
    };
    let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
    for point in points {
        left = left.min(point.x);
        top = top.min(point.y);
        right = right.max(point.x);
        bottom = bottom.max(point.y);
    }
    Rect {
        x: left,
        y: top,
        w: right.abs_diff(left).max(1),
        h: bottom.abs_diff(top).max(1),
    }
}

/// A BoardAI op must not redraw, strike or write over a shape you drew.
///
/// # Errors
/// `Rejected::Yours` when `element` is yours.
pub fn not_yours(element: &Element) -> Result<(), Rejected> {
    match element.kind {
        ElementKind::User { .. } => Err(Rejected::Yours(element.id.clone())),
        ElementKind::Node { .. }
        | ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Participant { .. }
        | ElementKind::Message { .. }
        | ElementKind::Cells { .. }
        | ElementKind::Cell { .. } => Ok(()),
    }
}

/// Your shape as the UI draws it; `None` for anything that isn't yours.
#[must_use]
pub fn user_shape(element: &Element) -> Option<UserShape> {
    let ElementKind::User { mark, note } = &element.kind else {
        return None;
    };
    let kind = match (mark, &element.geometry) {
        (UserMark::Box, Geometry::Box(rect)) => UserShapeKind::Box { rect: *rect },
        (UserMark::Arrow { from, to }, Geometry::Path(path)) => UserShapeKind::Arrow {
            start: path.start,
            end: path.end,
            from: from.clone(),
            to: to.clone(),
        },
        (UserMark::Ink { strokes }, Geometry::Box(bounds)) => UserShapeKind::Ink {
            strokes: strokes.clone(),
            bounds: *bounds,
        },
        (UserMark::Box | UserMark::Arrow { .. } | UserMark::Ink { .. }, _) => return None,
    };
    Some(UserShape {
        id: element.id.clone(),
        kind,
        label: (!element.label.is_empty()).then(|| element.label.clone()),
        note: note.clone(),
    })
}

/// Everything one of your shapes covers: its geometry, an arrow's label above its middle, and a
/// note under it (left-aligned under a box or a pen shape, centred under an arrow).
#[must_use]
pub fn area(mark: &UserMark, label: &str, note: Option<&str>, geometry: &Geometry) -> Rect {
    let bounds = geometry.bounds();
    let mut area = bounds;
    let line = i32::try_from(LABEL_LINE_HEIGHT).unwrap_or(0);
    match (mark, geometry) {
        (UserMark::Arrow { .. }, Geometry::Path(path)) => {
            let middle = Point {
                x: (path.start.x + path.end.x) / 2,
                y: (path.start.y + path.end.y) / 2,
            };
            if !label.is_empty() {
                let above = Point {
                    x: middle.x,
                    y: middle.y - ARROW_LABEL_ABOVE,
                };
                area = area.union(layout::text_box(label, above, SMALL_LABEL_FONT_SIZE));
            }
            if let Some(note) = note {
                let below = Point {
                    x: middle.x,
                    y: middle.y + NOTE_BELOW,
                };
                area = area.union(layout::text_box(note, below, NOTE_FONT_SIZE));
            }
        }
        (UserMark::Box | UserMark::Ink { .. } | UserMark::Arrow { .. }, _) => {
            if let Some(note) = note {
                area = area.union(Rect {
                    x: bounds.x,
                    y: bounds.bottom() + NOTE_BELOW - line / 2,
                    w: text_width(note, NOTE_FONT_SIZE),
                    h: LABEL_LINE_HEIGHT,
                });
            }
        }
    }
    area
}

#[cfg(test)]
mod tests;
