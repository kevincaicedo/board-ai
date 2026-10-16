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
        shape: NodeShape,
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
            | Self::Trace { .. } => true,
            Self::Plan { .. }
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
            | Self::Recap { .. } => false,
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
