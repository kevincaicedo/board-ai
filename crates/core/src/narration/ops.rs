//! What a narration tag asks for, before layout (LLD §2.3). Tags are parsed into these at the
//! narration boundary; the board, thread and surfaces only ever see typed ops.

use crate::ids::ElementId;
use crate::protocol::{CellMark, Fill, LineStyle, Mark, NodeShape};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NarrationOp {
    Plan {
        steps: Vec<String>,
    },
    Step {
        step: StepRef,
    },
    Node {
        id: ElementId,
        label: String,
        /// The talker's kind; without one a tree child takes its parent's, anything else is a
        /// service.
        shape: Option<NodeShape>,
        placement: Placement,
        stack: u8,
        /// The talker's tone; the board picks one when absent.
        fill: Option<Fill>,
    },
    Edge {
        from: ElementId,
        to: ElementId,
        label: Option<String>,
        style: LineStyle,
    },
    Annotate {
        target: ElementId,
        mark: Mark,
        text: Option<String>,
        /// A highlight's tone; the board picks one when absent. Pen marks have none.
        fill: Option<Fill>,
    },
    Strike {
        target: ElementId,
        text: String,
    },
    Focus {
        target: ElementId,
    },
    Detour {
        title: String,
    },
    Resume,
    Code {
        path: String,
        lines: Option<LineRange>,
    },
    Line {
        line: u32,
    },
    Trace {
        hops: Vec<ElementId>,
    },
    /// Options are the card's columns and criteria its rows.
    Compare {
        options: Vec<String>,
        criteria: Vec<String>,
        title: Option<String>,
    },
    Cell {
        criterion: AxisRef,
        option: AxisRef,
        mark: CellMark,
    },
    Close {
        surface: CloseTarget,
    },
    /// A pen mark on the open code card: `code` is the text to mark on `line` (1-based); the
    /// whole line when absent.
    MarkCode {
        line: u32,
        code: Option<String>,
        mark: Mark,
    },
    /// Opens the recap card with these bullets.
    Recap {
        bullets: Vec<String>,
    },
    /// A sequence diagram participant: a head box and its lifeline, right of the last one.
    Participant {
        id: ElementId,
        label: String,
    },
    /// A message between two participants' lifelines in the next row (`from == to` is a
    /// message to itself); a `Dashed` style is a return. `None` when the talker named no style:
    /// a message back along a call still open is its return (D-181).
    Message {
        from: ElementId,
        to: ElementId,
        label: Option<String>,
        style: Option<LineStyle>,
    },
    /// A row (or column) of cells; cell `i` is the element `{id}.{i}`.
    Cells(CellsSpec),
    /// Writes `text` into the target: an empty cell gets it written in, anything with a label
    /// is struck and rewritten.
    Write {
        target: ElementId,
        text: String,
    },
    /// The talker cited `path:line` in its speech: a source chip, if that line was read.
    Cite {
        path: String,
        line: u32,
    },
    /// Turns to a fresh, empty page for a new topic (D-177); the old page is kept.
    Page {
        title: Option<String>,
    },
}

/// What a `cells` tag asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellsSpec {
    pub id: ElementId,
    pub label: String,
    /// One value per cell from the first; cells past the values are empty.
    pub values: Vec<String>,
    /// How many cells, at least one and at least as many as the values.
    pub count: u8,
    pub index: IndexLabels,
    pub vertical: bool,
    pub placement: Placement,
}

/// The labels beside each cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexLabels {
    /// Positions counted from this number ("0", "1", …).
    From(u32),
    /// The talker's labels, one per cell from the first (addresses, keys).
    Named(Vec<String>),
    None,
}

impl NarrationOp {
    /// Ops that put strokes on the canvas. Only these take a drawing slot (LLD §2.4).
    #[must_use]
    pub fn draws(&self) -> bool {
        match self {
            Self::Node { .. }
            | Self::Edge { .. }
            | Self::Annotate { .. }
            | Self::Strike { .. }
            | Self::Trace { .. }
            | Self::Participant { .. }
            | Self::Message { .. }
            | Self::Cells(_)
            | Self::Write { .. } => true,
            Self::Cite { .. }
            | Self::Plan { .. }
            | Self::Step { .. }
            | Self::Focus { .. }
            | Self::Detour { .. }
            | Self::Resume
            | Self::Code { .. }
            | Self::Line { .. }
            | Self::Compare { .. }
            | Self::Cell { .. }
            | Self::Close { .. }
            | Self::MarkCode { .. }
            | Self::Recap { .. }
            | Self::Page { .. } => false,
        }
    }
}

/// `step n="2"` (1-based, as people count) or `step n="LB"` (a step's name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepRef {
    Number(u8),
    Name(String),
}

/// A compare criterion or option: a 0-based index, or its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AxisRef {
    Index(u8),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Continue the flow: right of the last node placed on the layer.
    Auto,
    Near {
        target: ElementId,
        direction: Direction,
    },
    /// Inside a group, optionally next to a sibling.
    Inside {
        group: ElementId,
        near: Option<(ElementId, Direction)>,
    },
    /// A tree node under its parent, joined to it by a line; `side` places a binary tree's
    /// left or right child, and without it children line up left to right.
    Child {
        parent: ElementId,
        side: Option<Side>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Right,
    Left,
    Above,
    Below,
}

/// Lines `[first, last]` of a file, 1-based and inclusive, as people write them ("42-52").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    first: u32,
    last: u32,
}

impl LineRange {
    #[must_use]
    pub fn new(first: u32, last: u32) -> Option<Self> {
        (first >= 1 && last >= first).then_some(Self { first, last })
    }

    #[must_use]
    pub fn first(self) -> u32 {
        self.first
    }

    #[must_use]
    pub fn last(self) -> u32 {
        self.last
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTarget {
    /// No `surface` attribute: the most recently opened surface.
    Latest,
    Code,
    Compare,
    Recap,
    All,
}
