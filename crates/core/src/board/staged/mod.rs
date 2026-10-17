//! Staged diagrams (D-187): a whole diagram spec laid out at once, before its first stroke, and
//! revealed part by part as the narration names it.
//!
//! Staging (`Board::stage`) resolves the spec's structure, lays it out globally (nested
//! containers sized from their content, sections side by side, records along a container's
//! bottom, connectors routed around what they don't join), finds it room on the layer clear of
//! everything there (the user's shapes included), and adds every part as a `Planned` element.
//! Nothing is drawn. A reveal (`Board::plan_reveal`, then `Board::start_reveal`) draws parts in a
//! hand's order under one op: the containers a part sits in first, the part, everything inside
//! it when the reveal is deep, each connector once both its ends are drawn, and the legend with
//! the diagram's first part. Since every part was laid out before the first stroke,
//! nothing drawn ever moves (I6), and a part that was never revealed was never drawn.

pub mod metrics;

mod connect;
mod entries;
mod layout;
mod place;
mod reveal;
mod structure;
pub mod summary;
mod text;

use crate::geometry::Rect;
use crate::ids::ElementId;
use crate::protocol::{Category, DiagramProvenance, DrawOp, StagedPart};

pub use place::{StageRefused, Staged};
pub use reveal::Reveal;

/// The staged diagrams of one layer, in the order they were staged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagedDiagrams {
    diagrams: Vec<StagedDiagram>,
}

impl StagedDiagrams {
    #[must_use]
    pub fn diagrams(&self) -> &[StagedDiagram] {
        &self.diagrams
    }
}

/// One staged diagram, with every part's geometry as it was laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedDiagram {
    pub id: ElementId,
    pub title: String,
    /// Everything it covers, its legend included.
    pub bounds: Rect,
    /// Its parts, then its connectors, in the order written.
    pub entries: Vec<Entry>,
    pub legend: Vec<LegendEntry>,
    /// Its top-level parts: the header's "part N of M".
    pub sections: u8,
    /// The part the header last named, from 1; 0 before anything was revealed.
    pub part_shown: u8,
    /// Where it came from when the talker didn't write it: a research brief's diagram (D-242).
    pub provenance: Option<DiagramProvenance>,
}

/// One part of a staged diagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: ElementId,
    /// The container it sits in, an index into `entries`.
    pub parent: Option<usize>,
    /// Its top-level part, from 1 (a connector's is its first end's).
    pub section: u8,
    /// How it is drawn, where it was laid out.
    pub draw: StagedPart,
    /// A connector's ends, as the parts they belong to.
    pub ends: Option<(usize, usize)>,
    /// The category its tone comes from, if any: its legend entry.
    pub tone: Option<Category>,
    /// Its fields' or cells' element ids (`arena.0`, …).
    pub subs: Vec<ElementId>,
}

/// A legend entry: drawn with the first part of its category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegendEntry {
    pub id: ElementId,
    pub category: Category,
    pub label: String,
    pub rect: Rect,
    pub drawn: bool,
}

impl StagedDiagram {
    /// The entry that draws `id`, a part or one of its fields or cells.
    #[must_use]
    pub fn entry_of(&self, id: &ElementId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.id == *id || entry.subs.contains(id))
    }

    /// The legend entry drawn as `id`.
    #[must_use]
    pub fn legend_entry(&self, id: &ElementId) -> Option<&LegendEntry> {
        self.legend.iter().find(|entry| entry.id == *id)
    }

    /// The containers holding `entry`, outermost first (bounded by the entries' count).
    #[must_use]
    pub fn ancestors(&self, entry: usize) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut cursor = self.entries.get(entry).and_then(|entry| entry.parent);
        while let Some(parent) = cursor {
            if chain.len() >= self.entries.len() {
                break;
            }
            chain.push(parent);
            cursor = self.entries[parent].parent;
        }
        chain.reverse();
        chain
    }
}

/// The parts of a reveal as one draw op.
#[must_use]
pub fn reveal_op(id: ElementId, op: crate::ids::OpId, parts: Vec<StagedPart>) -> DrawOp {
    DrawOp::Reveal { id, op, parts }
}
