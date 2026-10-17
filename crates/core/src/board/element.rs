//! What the board is made of: elements with stable ids, their geometry and their states, and
//! what laying out a drawing op produces.

use super::UserMark;
use super::sequence::RowCommit;
use crate::board::{layout, user};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, OpId};
use crate::protocol::{DrawOp, EdgePath, Fill, LineStyle, Mark, NodeShape};
use crate::time::SessionTime;
use layout::CellSpan;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: ElementId,
    /// The op that drew it (a group's growth redraws under the same op); `None` for a shape
    /// you drew, and only for one.
    pub op: Option<OpId>,
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
    /// A shape you drew (artboard 13). It is yours: BoardAI never moves, redraws, strikes or
    /// removes it (I6), and layout keeps clear of it. Its geometry is its box (a pen shape's
    /// bounds), or an arrow's line; its label is what you said it is.
    User {
        mark: UserMark,
        /// A second thing you said about it, written under it ("source of truth").
        note: Option<String>,
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
    /// and a row of cells' labels, and the words written beside one of your shapes.
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
            (ElementKind::User { mark, note }, _) => {
                user::area(mark, &self.label, note.as_deref(), &self.geometry)
            }
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
    /// A staged diagram's part (D-187): laid out with the whole diagram, never drawn until the
    /// narration reveals it. An interruption leaves it planned, for a later reveal.
    Planned,
}

impl ElementState {
    /// On the board for the user to see: laid out and started. A pending element waits for its
    /// slot; a planned one for its reveal.
    #[must_use]
    pub fn is_shown(self) -> bool {
        match self {
            Self::Pending | Self::Planned => false,
            Self::Drawing { .. } | Self::Drawn | Self::Frozen { .. } | Self::Struck => true,
        }
    }
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
    /// Parts of a staged diagram to reveal (D-187): start them with [`Board::start_reveal`].
    Reveal(Box<super::staged::Reveal>),
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
    /// A shape you drew: BoardAI never redraws, strikes or writes over it (I6).
    Yours(ElementId),
}
