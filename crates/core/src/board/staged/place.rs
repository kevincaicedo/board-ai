//! Staging a diagram on a layer (D-187): it is laid out whole, given room clear of everything
//! on the layer, and added as planned elements. Nothing is drawn yet.

use crate::board::staged::StagedDiagram;
use crate::board::staged::entries;
use crate::board::staged::layout::{Laid, lay_out};
use crate::board::staged::structure::resolve;
use crate::board::{Board, Element, ElementState, Layer};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{DIAGRAMS_PER_LAYER_MAX, STAGED_PAGE_HEIGHT_UNITS, STAGED_PAGE_WIDTH_UNITS};
use crate::narration::diagram::DiagramSpec;
use crate::protocol::Category;

/// Space kept between a diagram and what was already on the layer.
const DIAGRAM_MARGIN: i32 = 80;

/// What staging produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    /// The diagram's id on the layer: the spec's, or a numbered one when that was taken.
    pub id: ElementId,
    pub title: String,
    /// Elements added, fields and cells included.
    pub elements: usize,
    /// Its top-level parts.
    pub sections: u8,
    /// Parts of the spec that were dropped, each counted.
    pub dropped: u32,
}

/// Why a diagram was not staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageRefused {
    LayerMissing,
    /// Nothing in it could be kept.
    Empty,
    /// The layer holds [`DIAGRAMS_PER_LAYER_MAX`] already.
    Full,
    /// It doesn't fit the page beside what is drawn there: turn to a fresh page first (D-177).
    NeedsPage,
}

impl Board {
    /// Stages `spec` on the layer under `op`. On a page with drawing, a diagram that would take
    /// the page past its bounds is refused with `NeedsPage` when `page_may_turn`; otherwise it
    /// is laid out anyway, and the camera shows it smaller.
    pub fn stage(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        spec: &DiagramSpec,
        page_may_turn: bool,
        inherited: &[(Category, String)],
    ) -> Result<Staged, StageRefused> {
        let layer = self
            .layer_mut(layer_id)
            .map_err(|_| StageRefused::LayerMissing)?;
        if layer.staged.diagrams.len() >= DIAGRAMS_PER_LAYER_MAX {
            return Err(StageRefused::Full);
        }
        let id = free_id(layer, &spec.id);
        let structure = resolve(
            spec,
            |taken| layer.element(taken).is_some() || layer.staged_id(taken),
            inherited,
        );
        if structure.parts.is_empty() {
            return Err(StageRefused::Empty);
        }
        let laid = lay_out(&structure, spec.flow);
        let origin = origin(layer, &laid, page_may_turn)?;
        let mut dropped = structure.dropped;
        let diagram = entries::build(&id, &spec.title, &structure, &laid, origin);
        let elements = insert(layer, op, &diagram, &mut dropped);
        layer.grid.occupy_rect(diagram.bounds, &id);
        let sections = diagram.sections;
        layer.staged.diagrams.push(diagram);
        Ok(Staged {
            id,
            title: spec.title.clone(),
            elements,
            sections,
            dropped,
        })
    }
}

impl Layer {
    /// Takes back a planned element that never became part of its diagram.
    fn remove_planned(&mut self, id: &ElementId) {
        if self
            .element(id)
            .is_none_or(|element| element.state != ElementState::Planned)
        {
            return;
        }
        self.elements.retain(|element| element.id != *id);
        self.index = self
            .elements
            .iter()
            .enumerate()
            .map(|(index, element)| (element.id.clone(), index))
            .collect();
    }

    /// An id a staged diagram on this layer goes by.
    pub(in crate::board) fn staged_id(&self, id: &ElementId) -> bool {
        self.staged.diagrams.iter().any(|diagram| diagram.id == *id)
    }
}

/// The spec's id, or the first numbered one (`diagram2`, …) free on the layer.
fn free_id(layer: &Layer, wanted: &ElementId) -> ElementId {
    let free = |id: &ElementId| layer.element(id).is_none() && !layer.staged_id(id);
    if free(wanted) {
        return wanted.clone();
    }
    (2..=DIAGRAMS_PER_LAYER_MAX + 1)
        .filter_map(|number| ElementId::parse(&format!("{}{number}", wanted.as_str())))
        .find(|id| free(id))
        .unwrap_or_else(|| wanted.clone())
}

/// Where the diagram's top-left corner goes: at the origin of an empty layer, else under
/// everything on it (the user's shapes and other diagrams' planned parts included).
fn origin(layer: &Layer, laid: &Laid, page_may_turn: bool) -> Result<Point, StageRefused> {
    let everything = layer.elements.iter().map(Element::area).reduce(Rect::union);
    let Some(everything) = everything else {
        return Ok(Point::default());
    };
    let origin = Point {
        x: everything.x,
        y: everything.bottom() + DIAGRAM_MARGIN,
    };
    let diagram = Rect {
        x: origin.x,
        y: origin.y,
        w: laid.width,
        h: laid.height,
    };
    let page = everything.union(diagram);
    let past_page = page.w > STAGED_PAGE_WIDTH_UNITS || page.h > STAGED_PAGE_HEIGHT_UNITS;
    let drawn = layer
        .elements
        .iter()
        .any(|element| element.state.is_shown());
    if past_page && drawn && page_may_turn {
        return Err(StageRefused::NeedsPage);
    }
    Ok(origin)
}

/// Adds the diagram's parts to the layer as planned elements, under the staging op; a part the
/// layer has no room for is dropped and counted, and so is a connector that lost an end.
/// Returns how many elements were added.
fn insert(layer: &mut Layer, op: OpId, diagram: &StagedDiagram, dropped: &mut u32) -> usize {
    let mut added = 0;
    let mut kept = vec![false; diagram.entries.len()];
    for (at, entry) in diagram.entries.iter().enumerate() {
        if let Some((from, to)) = entry.ends
            && !(kept[from] && kept[to])
        {
            *dropped += 1;
            continue;
        }
        let elements = entries::elements_of(entry, op);
        let count = elements.len();
        let mut inserted = 0;
        for element in elements {
            if layer.insert(element).is_err() {
                break;
            }
            inserted += 1;
        }
        if inserted < count {
            // A part is whole or not there: the room ran out part-way through its cells.
            for element_id in std::iter::once(&entry.id).chain(&entry.subs).take(inserted) {
                layer.remove_planned(element_id);
            }
            *dropped += 1;
            continue;
        }
        kept[at] = true;
        added += count;
    }
    added
}
