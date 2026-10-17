//! Revealing a staged diagram's parts (D-187) in a hand's order: the containers a part sits in,
//! outermost first; the part; with `deep`, everything inside it, each container before what it
//! holds; each connector right after the second of its ends; and with the diagram's first part,
//! its whole legend. A part that would land on a shape the user drew since the diagram was
//! staged stays planned (I6 for the user's shapes), and so do its connectors.

use crate::board::staged::metrics::part_area;
use crate::board::staged::{StagedDiagram, reveal_op};
use crate::board::{Board, ElementKind, ElementState, Layer, Rejected, UserMark, generated_id};
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId, OpId};
use crate::protocol::{DrawOp, StagedPart};
use crate::time::SessionTime;

/// What a reveal draws, planned and not yet started.
#[derive(Clone, Debug, PartialEq)]
pub struct Reveal {
    /// `DrawOp::Reveal`, its parts in drawing order; none when everything named is drawn.
    pub draw: DrawOp,
    /// The layer's diagram it reveals, as an index of its staged diagrams.
    pub diagram: usize,
    /// The entries it draws, then the legend entries.
    pub entries: Vec<usize>,
    pub legend: Vec<usize>,
    /// The diagram's top-level part it starts in, from 1.
    pub section: u8,
    /// Parts left planned because they would land on a shape the user drew.
    pub skipped: u32,
}

impl Reveal {
    /// Nothing named is left to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.legend.is_empty()
    }
}

impl Board {
    /// What revealing `target` draws now: a staged part, one of its fields, or a diagram's own id
    /// (its top-level parts; everything when `deep`). `UnknownElement` when nothing staged on the
    /// layer goes by that id.
    pub fn plan_reveal(
        &self,
        layer_id: LayerId,
        op: OpId,
        target: &ElementId,
        deep: bool,
    ) -> Result<Reveal, Rejected> {
        let layer = self.layer(layer_id).ok_or(Rejected::LayerMissing)?;
        let found = layer
            .staged
            .diagrams
            .iter()
            .enumerate()
            .find_map(|(index, diagram)| {
                if diagram.id == *target {
                    return Some((index, None));
                }
                diagram.entry_of(target).map(|entry| (index, Some(entry)))
            });
        let Some((index, entry)) = found else {
            return Err(Rejected::UnknownElement(target.clone()));
        };
        let diagram = &layer.staged.diagrams[index];
        let mut plan = Plan::new(layer, diagram);
        match entry {
            None => plan.whole(deep),
            Some(entry) => plan.part(entry, deep),
        }
        let legend = plan.legend();
        let section = plan
            .order
            .first()
            .map_or(diagram.part_shown, |first| diagram.entries[*first].section);
        let mut parts: Vec<StagedPart> = plan
            .order
            .iter()
            .map(|entry| diagram.entries[*entry].draw.clone())
            .collect();
        parts.extend(legend.iter().map(|at| {
            let entry = &diagram.legend[*at];
            StagedPart::Legend {
                id: entry.id.clone(),
                rect: entry.rect,
                category: entry.category,
                label: entry.label.clone(),
            }
        }));
        Ok(Reveal {
            draw: reveal_op(generated_id("r", op), op, parts),
            diagram: index,
            entries: plan.order,
            legend,
            section,
            skipped: plan.skipped,
        })
    }

    /// The reveal starts now: its parts, with their fields and cells, are drawing under `op`.
    pub fn start_reveal(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        reveal: &Reveal,
        now: SessionTime,
        budget_ms: u32,
    ) {
        let Ok(layer) = self.layer_mut(layer_id) else {
            return;
        };
        let Some(diagram) = layer.staged.diagrams.get_mut(reveal.diagram) else {
            return;
        };
        let mut ids: Vec<ElementId> = Vec::new();
        for entry in &reveal.entries {
            if let Some(entry) = diagram.entries.get(*entry) {
                ids.push(entry.id.clone());
                ids.extend(entry.subs.iter().cloned());
            }
        }
        for at in &reveal.legend {
            if let Some(entry) = diagram.legend.get_mut(*at) {
                entry.drawn = true;
            }
        }
        diagram.part_shown = reveal.section;
        for id in &ids {
            if let Some(element) = layer.element_mut(id)
                && element.state == ElementState::Planned
            {
                element.op = Some(op);
                element.state = ElementState::Drawing {
                    started_at: now,
                    budget_ms,
                };
            }
        }
    }

    /// A staged part laid out but not yet revealed.
    #[must_use]
    pub fn is_planned(&self, layer_id: LayerId, id: &ElementId) -> bool {
        self.layer(layer_id)
            .and_then(|layer| layer.element(id))
            .is_some_and(|element| element.state == ElementState::Planned)
    }

    /// The staged diagrams of a layer.
    #[must_use]
    pub fn staged(&self, layer_id: LayerId) -> &[StagedDiagram] {
        self.layer(layer_id)
            .map_or(&[], |layer| layer.staged.diagrams())
    }
}

/// The order a reveal draws its entries in, built one entry at a time.
struct Plan<'a> {
    layer: &'a Layer,
    diagram: &'a StagedDiagram,
    order: Vec<usize>,
    queued: Vec<bool>,
    skipped: u32,
    /// The user's boxes and pen shapes: nothing solid lands on them.
    yours: Vec<Rect>,
}

impl<'a> Plan<'a> {
    fn new(layer: &'a Layer, diagram: &'a StagedDiagram) -> Self {
        let yours = layer
            .elements
            .iter()
            .filter(|element| {
                matches!(
                    element.kind,
                    ElementKind::User {
                        mark: UserMark::Box | UserMark::Ink { .. },
                        ..
                    }
                )
            })
            .map(crate::board::Element::area)
            .collect();
        Self {
            layer,
            diagram,
            order: Vec::new(),
            queued: vec![false; diagram.entries.len()],
            skipped: 0,
            yours,
        }
    }

    fn on(&self, entry: usize) -> bool {
        self.queued[entry]
            || self
                .layer
                .element(&self.diagram.entries[entry].id)
                .is_some_and(|element| element.state.is_shown())
    }

    /// Frames and connectors may cross a shape of the user's; nothing else lands on one.
    fn blocked(&self, entry: usize) -> bool {
        let draw = &self.diagram.entries[entry].draw;
        let solid = match draw {
            StagedPart::Frame { .. } | StagedPart::Edge { .. } => false,
            StagedPart::Node { .. }
            | StagedPart::Note { .. }
            | StagedPart::Record { .. }
            | StagedPart::Cells { .. }
            | StagedPart::Legend { .. } => true,
        };
        solid && {
            let area = part_area(draw);
            self.yours.iter().any(|yours| yours.intersects(area))
        }
    }

    /// Draws `entry` unless it is drawn or queued, then each connector it completes.
    fn push(&mut self, entry: usize) {
        let missing = self
            .layer
            .element(&self.diagram.entries[entry].id)
            .is_none();
        if self.on(entry) || missing {
            return;
        }
        if self.blocked(entry) {
            self.skipped += 1;
            return;
        }
        self.order.push(entry);
        self.queued[entry] = true;
        for edge in 0..self.diagram.entries.len() {
            let Some((from, to)) = self.diagram.entries[edge].ends else {
                continue;
            };
            let touches = from == entry || to == entry;
            if touches && !self.on(edge) && self.on(from) && self.on(to) {
                self.order.push(edge);
                self.queued[edge] = true;
            }
        }
    }

    /// A part, after the containers it sits in; everything inside it when `deep`. A connector's
    /// ends come first, then the connector.
    fn part(&mut self, entry: usize, deep: bool) {
        if let Some((from, to)) = self.diagram.entries[entry].ends {
            for end in [from, to] {
                self.with_ancestors(end);
            }
            // Ends drawn earlier don't bring their connector along: it is named here.
            if self.on(from) && self.on(to) {
                self.push(entry);
            }
            return;
        }
        self.with_ancestors(entry);
        if deep {
            self.inside(entry);
        }
    }

    /// The whole diagram's top-level parts; everything when `deep`.
    fn whole(&mut self, deep: bool) {
        let tops: Vec<usize> = self.children(None);
        for top in tops {
            self.push(top);
            if deep {
                self.inside(top);
            }
        }
    }

    fn with_ancestors(&mut self, entry: usize) {
        for ancestor in self.diagram.ancestors(entry) {
            self.push(ancestor);
        }
        self.push(entry);
    }

    /// Everything inside `entry`, each container before what it holds, in the order written.
    fn inside(&mut self, entry: usize) {
        let mut stack: Vec<usize> = self.children(Some(entry)).into_iter().rev().collect();
        // Each entry has one container, so each is pushed onto the stack once.
        for _ in 0..self.diagram.entries.len() {
            let Some(next) = stack.pop() else {
                return;
            };
            self.push(next);
            stack.extend(self.children(Some(next)).into_iter().rev());
        }
    }

    fn children(&self, parent: Option<usize>) -> Vec<usize> {
        self.diagram
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.ends.is_none() && entry.parent == parent)
            .map(|(at, _)| at)
            .collect()
    }

    /// The legend, whole, with the diagram's first drawn part: it decodes the colours of the
    /// parts still to come (canvas 15).
    fn legend(&self) -> Vec<usize> {
        if self.order.is_empty() {
            return Vec::new();
        }
        (0..self.diagram.legend.len())
            .filter(|at| !self.diagram.legend[*at].drawn)
            .collect()
    }
}
