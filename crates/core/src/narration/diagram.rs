//! What a `<diagram>` block asks for (D-187), before layout: a whole diagram in the narration
//! vocabulary, parsed at the narration boundary into these types. The board lays the spec out at
//! once (`board::staged`); the narration then reveals it part by part.
//!
//! A block comes from the talker's own narration, a research brief's diagram passed to the
//! talker, or a document digest's figure. They all use the same syntax:
//!
//! ```text
//! <diagram id="alloc" title="inf-alloc · memory allocation">
//!   <node id="arena" kind="group" label="Arena" note="slab allocation" cat="RAM records"/>
//!   <record id="arena_s" label="struct Arena" fields="cfg|threshold|classes" in="arena"/>
//!   <node id="free" label="Free" kind="pill" in="slots"/>
//!   <edge from="free" to="leased" label="try lease"/>
//! </diagram>
//! ```

use crate::ids::ElementId;
use crate::narration::ops::IndexLabels;
use crate::protocol::{LineStyle, NodeShape};

/// A diagram spec, its parts in the order they were written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagramSpec {
    /// The diagram's own id: revealing it reveals its top-level parts (`deep`, everything).
    pub id: ElementId,
    pub title: String,
    /// How the top-level parts stand: side by side unless the block says otherwise.
    pub flow: Option<Flow>,
    pub items: Vec<SpecItem>,
    /// Tags inside the block that were dropped (malformed, unknown inside a diagram, or past a
    /// limit). The stream counts each where it drops it; this is the spec's own tally.
    pub dropped: u32,
}

/// One part and the container it was written `in`, as the talker named it. The board resolves
/// the names when it lays the diagram out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecItem {
    pub parent: Option<ElementId>,
    pub part: SpecPart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecPart {
    /// A frame holding other parts (`node kind="group"`).
    Container {
        id: ElementId,
        label: String,
        note: Option<String>,
        category: Option<String>,
        dashed: bool,
        flow: Option<Flow>,
    },
    /// A box, pill, cylinder, queue, actor or circle; a numbered `step` writes "1. " before its
    /// label. `new_row` (`dir="below"`) starts a new row of boxes in its container.
    Node {
        id: ElementId,
        label: String,
        shape: NodeShape,
        note: Option<String>,
        step: Option<u8>,
        category: Option<String>,
        dashed: bool,
        new_row: bool,
    },
    /// A remark written inside the diagram; an id is made for it when it has none.
    Note { id: Option<ElementId>, text: String },
    /// A struct with its fields in a row; field `i` is the element `{id}.{i}`.
    Record {
        id: ElementId,
        label: String,
        fields: Vec<String>,
    },
    /// A row (or column) of cells, as `cells` draws one; `values` has one entry per cell.
    Cells {
        id: ElementId,
        label: String,
        values: Vec<String>,
        index: IndexLabels,
        vertical: bool,
    },
    /// A connector between two parts; its id is `from>to` unless it names one.
    Edge {
        id: Option<ElementId>,
        from: ElementId,
        to: ElementId,
        label: Option<String>,
        style: LineStyle,
    },
}

/// How a container's parts stand: side by side, or one under another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Row,
    Column,
}

impl SpecPart {
    /// The part's own id, when it has one before layout.
    #[must_use]
    pub fn id(&self) -> Option<&ElementId> {
        match self {
            Self::Container { id, .. }
            | Self::Node { id, .. }
            | Self::Record { id, .. }
            | Self::Cells { id, .. } => Some(id),
            Self::Note { id, .. } | Self::Edge { id, .. } => id.as_ref(),
        }
    }
}
