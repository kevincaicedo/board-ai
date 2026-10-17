//! Staged diagrams in the conversation (D-187): staging a `<diagram>` when its words play,
//! revealing its parts as the narration names them, and the camera that follows a reveal.
//!
//! - **Staging** lays the whole diagram out before anything of it is drawn. On a page that has
//!   drawing and no room for it, the page turns first (D-177); on a detour or detail layer it is
//!   laid out anyway and the camera shows it smaller, never cropped.
//! - **A reveal** draws parts in a hand's order within one budget that reaches the next drawing's
//!   word (or the end of what is said), up to [`REVEAL_BUDGET_MAX_MS`]. A drawing tag naming a
//!   staged part (`<node id="arena"/>`) reveals it too, and a diagram that nothing in its reply
//!   reveals is revealed whole as its words play: a staged diagram never stays empty.
//! - **The camera** frames what the reveal draws plus the next reveal, and may close up on it
//!   (`Camera.enlarge`): the canvas's small sizes read like a sketch's. A reply that revealed
//!   [`DIAGRAM_OVERVIEW_PARTS_MIN`] parts or more ends by showing its diagram whole, once.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use crate::board::staged::metrics::part_area;
use crate::board::staged::{Reveal, StageRefused, Staged};
use crate::board::{Placed, Rejected, layout};
use crate::effect::{Dropped, Effect, LogRecord};
use crate::engine::Core;
use crate::engine::ops::{Anchor, QueuedOp};
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{
    CAMERA_MARGIN_UNITS, DIAGRAM_OVERVIEW_PARTS_MIN, LEFTOVER_REVEAL_MS_PER_PART, OP_BUDGET_MIN_MS,
    REVEAL_BUDGET_MAX_MS,
};
use crate::narration::diagram::DiagramSpec;
use crate::narration::ops::NarrationOp;
use crate::protocol::{Category, DiagramProvenance, UiEvent};
use crate::time::SessionTime;

/// Staged parts named in a notice to the talker; the rest are counted.
const NOTICE_PARTS_MAX: usize = 24;

/// Diagrams staged recently, remembered for revealing them whole; older ones are forgotten (a
/// reply stages a few at most).
const STAGINGS_KEPT_MAX: usize = 2 * crate::limits::DIAGRAMS_PER_LAYER_MAX;

/// A diagram staged on the active layer.
pub(super) struct StagedHere {
    pub staged: Staged,
    pub layer: LayerId,
    /// The page turned to make room for it (D-177).
    pub page_turned: bool,
}

/// What the engine remembers about staging between a diagram's op and its reveals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Staging {
    /// The diagram each staging op laid out, and on which layer, newest last.
    by_op: VecDeque<(OpId, LayerId, ElementId)>,
    /// Staging ops whose diagram nothing of their reply reveals: revealed whole once staged.
    auto: BTreeSet<OpId>,
    /// Parts revealed per layer since the last reply ended.
    revealed: BTreeMap<LayerId, usize>,
    /// A detail layer the user opened by voice that nothing has been drawn on yet (D-188).
    pub(super) voice_detail: Option<LayerId>,
    /// Research briefs' diagrams staged for the reply being written (D-242): what it leaves
    /// planned is revealed as it ends.
    briefs: Vec<(LayerId, ElementId)>,
}

impl Staging {
    fn staged(&mut self, layer: LayerId, op: OpId, id: ElementId) {
        self.by_op.push_back((op, layer, id));
        while self.by_op.len() > STAGINGS_KEPT_MAX {
            self.by_op.pop_front();
        }
    }

    fn staged_by(&self, op: OpId) -> Option<(LayerId, ElementId)> {
        self.by_op
            .iter()
            .find(|(by, _, _)| *by == op)
            .map(|(_, layer, id)| (*layer, id.clone()))
    }

    fn want_auto_reveal(&mut self, op: OpId) {
        if self.auto.len() < STAGINGS_KEPT_MAX {
            self.auto.insert(op);
        }
    }

    fn take_auto_reveal(&mut self, op: OpId) -> bool {
        self.auto.remove(&op)
    }

    fn revealed(&mut self, layer: LayerId, parts: usize) {
        *self.revealed.entry(layer).or_default() += parts;
    }

    pub(super) fn brief_staged(&mut self, layer: LayerId, id: ElementId) {
        if self.briefs.len() < STAGINGS_KEPT_MAX {
            self.briefs.push((layer, id));
        }
    }

    pub(super) fn take_briefs(&mut self) -> Vec<(LayerId, ElementId)> {
        std::mem::take(&mut self.briefs)
    }

    fn take_revealed(&mut self) -> BTreeMap<LayerId, usize> {
        self.auto.clear();
        self.voice_detail = None;
        std::mem::take(&mut self.revealed)
    }
}

impl Core {
    /// `<diagram>`, as its words play: the whole diagram is laid out, nothing drawn yet.
    pub(super) fn stage_diagram(
        &mut self,
        op: OpId,
        spec: &DiagramSpec,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let here = match self.stage_here(op, spec, None, effects) {
            Ok(here) => here,
            Err(refused) => {
                self.refused_diagram(spec, &refused);
                return false;
            }
        };
        let StagedHere { staged, layer, .. } = here;
        if staged.dropped > 0 {
            self.context.notice(format!(
                "{} parts of your diagram \"{}\" were dropped: an id written twice or already on \
                 the board, a container nested too deep, a fifth category, a connector naming \
                 nothing, or past {} parts.",
                staged.dropped,
                staged.title,
                crate::limits::DIAGRAM_PARTS_MAX
            ));
        }
        self.staging.staged(layer, op, staged.id.clone());
        if self.staging.take_auto_reveal(op) {
            self.queue_auto_reveal(layer, op, &staged.id);
        }
        true
    }

    /// Lays `spec` out on the active layer under `op`, nothing drawn yet, and names it in the
    /// header (with where it came from, when it wasn't the talker's: D-242). On a page with
    /// drawing and no room for it, the page turns first (D-177). Parts it dropped are counted.
    pub(super) fn stage_here(
        &mut self,
        op: OpId,
        spec: &DiagramSpec,
        provenance: Option<DiagramProvenance>,
        effects: &mut Vec<Effect>,
    ) -> Result<StagedHere, StageRefused> {
        let layer = self.thread.active_layer();
        let main = layer == LayerId::MAIN;
        let inherited = self.inherited_categories();
        let mut page_turned = false;
        let staged = match self.board.stage(layer, op, spec, main, &inherited) {
            Err(StageRefused::NeedsPage) => {
                // The page has drawing and no room for a map this size (D-177).
                page_turned = self.turn_page(Some(spec.title.clone()), effects).is_ok();
                self.board.stage(layer, op, spec, false, &inherited)
            }
            other => other,
        }?;
        if staged.dropped > 0 {
            effects.push(Effect::Log(LogRecord::Dropped {
                what: Dropped::DiagramPart,
                count: staged.dropped,
            }));
        }
        if let Some(provenance) = &provenance {
            self.board
                .credit_staged(layer, &staged.id, provenance.clone());
        }
        effects.push(Effect::Ui(UiEvent::Diagram {
            layer,
            title: staged.title.clone(),
            part: 0,
            parts: staged.sections,
            provenance,
        }));
        Ok(StagedHere {
            staged,
            layer,
            page_turned,
        })
    }

    /// On a detail layer, the categories of the diagrams it drills into: they keep their colours.
    fn inherited_categories(&self) -> Vec<(Category, String)> {
        let Some(anchor) = self
            .thread
            .detours()
            .top()
            .and_then(|detour| detour.anchor.as_ref())
        else {
            return Vec::new();
        };
        self.board
            .staged(anchor.layer)
            .iter()
            .flat_map(|diagram| &diagram.legend)
            .map(|legend| (legend.category, legend.label.clone()))
            .collect()
    }

    fn refused_diagram(&mut self, spec: &DiagramSpec, refused: &StageRefused) {
        self.context.notice(format!(
            "Your diagram \"{}\" was not staged: {}.",
            spec.title,
            not_staged_because(refused)
        ));
    }

    /// A reveal, or a drawing tag naming a staged part: the staged parts it draws. `None` when
    /// the op is no reveal at all.
    pub(super) fn place_staged(
        &mut self,
        layer: LayerId,
        queued: &QueuedOp,
    ) -> Option<Result<Placed, Rejected>> {
        if self.names_drawn_part(layer, &queued.op) {
            return Some(Ok(Placed::Unchanged));
        }
        let (target, deep) = self.reveal_target(layer, &queued.op)?;
        let reveal = match self.board.plan_reveal(layer, queued.id, &target, deep) {
            Ok(reveal) => reveal,
            Err(rejected) => {
                self.unknown_reveal(layer, &target);
                return Some(Err(rejected));
            }
        };
        if reveal.is_empty() && reveal.skipped == 0 {
            return Some(Ok(Placed::Unchanged));
        }
        Some(Ok(Placed::Reveal(Box::new(reveal))))
    }

    /// A drawing tag naming a staged part that is already drawn points at it and changes nothing
    /// (D-251): live, gpt-oss-120b wrote `<reveal id="mod"/>` then `<node id="mod"/>`, and a bare
    /// node's label is its id, so each struck the part's real label ("harness/mod.rs") and wrote
    /// "mod ?" (D-180) over it, 13 times in one map. A staged part's label is corrected by
    /// `<strike>`.
    fn names_drawn_part(&self, layer: LayerId, op: &NarrationOp) -> bool {
        let Some(named) = drawing_tag_target(op) else {
            return false;
        };
        let staged = self
            .board
            .staged(layer)
            .iter()
            .any(|diagram| diagram.entry_of(&named).is_some());
        let drawn = self
            .board
            .layer(layer)
            .and_then(|board| board.element(&named))
            .is_some_and(|element| element.state.is_shown());
        staged && drawn
    }

    /// What `op` reveals: a reveal's target, or the staged part a drawing tag names (models
    /// write `<node id="arena"/>` for a part they staged).
    fn reveal_target(&self, layer: LayerId, op: &NarrationOp) -> Option<(ElementId, bool)> {
        if let NarrationOp::Reveal { target, deep } = op {
            return Some((target.clone(), *deep));
        }
        let named = drawing_tag_target(op)?;
        if !self.board.is_planned(layer, &named) {
            return None;
        }
        // A container named by a drawing tag is drawn with what it holds: models write
        // `<node id="pool"/>` for a section they staged, and its frame alone shows nothing.
        let container = self
            .board
            .layer(layer)
            .and_then(|board| board.element(&named))
            .is_some_and(|element| {
                matches!(
                    element.kind,
                    crate::board::ElementKind::Node {
                        shape: crate::protocol::NodeShape::Group,
                        ..
                    }
                )
            });
        Some((named, container))
    }

    /// The talker named nothing staged: say what is.
    fn unknown_reveal(&mut self, layer: LayerId, target: &ElementId) {
        let mut text = format!(
            "Your <reveal id=\"{}\"/> named nothing staged on this board, so nothing was drawn.",
            target.as_str()
        );
        let diagrams = self.board.staged(layer);
        if diagrams.is_empty() {
            text.push_str(" No diagram is staged here: write a <diagram> block first.");
        }
        for diagram in diagrams {
            let mut planned: Vec<&str> = diagram
                .entries
                .iter()
                .filter(|entry| entry.ends.is_none() && self.board.is_planned(layer, &entry.id))
                .map(|entry| entry.id.as_str())
                .collect();
            let more = planned.len().saturating_sub(NOTICE_PARTS_MAX);
            planned.truncate(NOTICE_PARTS_MAX);
            let _ = write!(
                text,
                " Diagram {} is staged; its parts not drawn yet: {}",
                diagram.id.as_str(),
                if planned.is_empty() {
                    "none".to_owned()
                } else {
                    planned.join(", ")
                }
            );
            if more > 0 {
                let _ = write!(text, " and {more} more");
            }
            text.push('.');
        }
        self.context.notice(text);
    }

    /// The reveal starts: the camera frames it and the next reveal, the header names its part,
    /// and its parts draw.
    pub(super) fn start_reveal(
        &mut self,
        now: SessionTime,
        layer: LayerId,
        op: OpId,
        reveal: &Reveal,
        budget_ms: u32,
        effects: &mut Vec<Effect>,
    ) {
        self.skipped_parts(reveal.skipped, effects);
        if reveal.is_empty() {
            return;
        }
        let next = self.next_reveal_area(layer);
        let area = layout::op_bounds(&reveal.draw);
        let frame = layout::inflate(
            next.map_or(area, |next| area.union(next)),
            CAMERA_MARGIN_UNITS,
            0,
        );
        // A close-up only for a part small beside its diagram: the canvas's small sizes read
        // like a sketch's there, and a large part stays whole in view.
        let enlarge = self
            .board
            .staged(layer)
            .get(reveal.diagram)
            .is_some_and(|diagram| {
                frame.w.saturating_mul(2) <= diagram.bounds.w
                    && frame.h.saturating_mul(2)
                        <= diagram.bounds.h.saturating_add(diagram.bounds.h / 2)
            });
        if let Some(event) = self.cameras.frame(layer, frame, enlarge) {
            effects.push(Effect::Ui(event));
        }
        let shown_before = self
            .board
            .staged(layer)
            .get(reveal.diagram)
            .map(|diagram| diagram.part_shown);
        self.board.start_reveal(layer, op, reveal, now, budget_ms);
        if let Some(diagram) = self.board.staged(layer).get(reveal.diagram)
            && shown_before != Some(diagram.part_shown)
        {
            effects.push(Effect::Ui(UiEvent::Diagram {
                layer,
                title: diagram.title.clone(),
                part: diagram.part_shown,
                parts: diagram.sections,
                provenance: diagram.provenance.clone(),
            }));
        }
        self.staging
            .revealed(layer, reveal.entries.len() + reveal.legend.len());
        self.op_start(now, layer, reveal.draw.clone(), budget_ms, effects);
    }

    /// Parts a reveal left planned: they would land on a shape the user drew since staging.
    fn skipped_parts(&mut self, skipped: u32, effects: &mut Vec<Effect>) {
        if skipped == 0 {
            return;
        }
        effects.push(Effect::Log(LogRecord::Dropped {
            what: Dropped::DiagramPart,
            count: skipped,
        }));
        self.context.notice(format!(
            "{skipped} staged parts were not drawn: they would land on shapes the user drew there."
        ));
    }

    /// What the next queued reveal on `layer` would draw, for the camera.
    /// The part the next reveal names, for the camera: its own box, not everything it brings
    /// along (a long connector back to an earlier section would widen the frame to the whole
    /// map).
    fn next_reveal_area(&self, layer: LayerId) -> Option<Rect> {
        let next = self.queue.ops().find(|queued| queued.op.draws())?;
        let (target, _) = self.reveal_target(layer, &next.op)?;
        self.board.staged(layer).iter().find_map(|diagram| {
            let entry = diagram.entry_of(&target)?;
            let shown = self
                .board
                .layer(layer)
                .and_then(|board| board.element(&target))
                .is_some_and(|element| element.state.is_shown());
            (!shown).then(|| part_area(&diagram.entries[entry].draw))
        })
    }

    /// A reveal draws while it is explained: until the next drawing's word, or the end of what
    /// is said so far.
    pub(super) fn reveal_budget(&self, now: SessionTime, parts: usize) -> u32 {
        let next = self
            .queue
            .ops()
            .find(|queued| queued.op.draws())
            .map(|queued| queued.anchor);
        let ms = match next {
            Some(Anchor::Spoken { clause, offset }) => {
                self.voice.estimate_ms_until(clause, offset, now)
            }
            // Nothing more is said to time it by: a pace per part (D-237's leftovers, D-250).
            Some(Anchor::Immediate { .. }) | None if self.voice.is_silent() => u64::try_from(parts)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::from(LEFTOVER_REVEAL_MS_PER_PART)),
            Some(Anchor::Immediate { .. }) | None => self.voice.estimate_ms_until_end(now),
        };
        let clamped = ms.clamp(u64::from(OP_BUDGET_MIN_MS), u64::from(REVEAL_BUDGET_MAX_MS));
        u32::try_from(clamped).unwrap_or(REVEAL_BUDGET_MAX_MS)
    }

    /// Stream `index` ended: a diagram it staged that nothing of it revealed is revealed whole,
    /// as its words play (D-187).
    pub(super) fn auto_reveal(&mut self, index: usize) {
        let Some(stream) = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
        else {
            return;
        };
        let base = stream.op_base;
        let unrevealed: Vec<OpId> = stream
            .narration
            .unrevealed_diagrams()
            .map(|op| OpId(base.saturating_add(op)))
            .collect();
        for op in unrevealed {
            match self.staging.staged_by(op) {
                Some((layer, id)) => self.queue_auto_reveal(layer, op, &id),
                None => self.staging.want_auto_reveal(op),
            }
        }
    }

    /// The reply ended normally and everything it said has played: each diagram staged for it on
    /// the active layer, its own (`<diagram>`, `<detail>`) and the research briefs' it was given
    /// (D-242), that still holds parts it can draw is revealed whole, once, in a hand's order, at
    /// `LEFTOVER_REVEAL_MS_PER_PART` (D-237, D-250). Live models revealed a section's frame and
    /// named a few of its parts, and the rest stayed planned: DeepSeek's repo maps showed 19 and
    /// 22 of the 31 and 36 parts they staged (T-035). A reply that was interrupted, refused or
    /// failed reveals nothing: what it didn't reach stays planned, and the summary names it. A
    /// diagram whose layer is no longer in front (a detour opened, a page turned) stays planned
    /// too. True when a reveal was queued.
    pub(super) fn queue_leftovers(&mut self) -> bool {
        let layer = self.thread.active_layer();
        let Some(reply) = self.reply.as_mut().filter(|reply| !reply.leftovers_queued) else {
            return false;
        };
        reply.leftovers_queued = true;
        let ended = reply
            .streams
            .last()
            .is_some_and(super::reply::Stream::ended_normally);
        let first_op = reply
            .streams
            .first()
            .map_or(u32::MAX, |stream| stream.op_base);
        let stream = reply.streams.len().saturating_sub(1);
        let briefs = self.staging.take_briefs();
        if !ended {
            return false;
        }
        let staged: Vec<ElementId> = self
            .staging
            .by_op
            .iter()
            .filter(|(op, on, _)| op.0 >= first_op && *on == layer)
            .map(|(_, _, id)| id.clone())
            .chain(
                briefs
                    .into_iter()
                    .filter(|(on, _)| *on == layer)
                    .map(|(_, id)| id),
            )
            .collect();
        let mut queued = false;
        for id in staged {
            // Nothing left that can be drawn (every part drawn, or under the user's shapes, which
            // were counted when they were passed over): no reveal.
            let left = self
                .board
                .plan_reveal(layer, OpId(self.queue.next_op), &id, true)
                .is_ok_and(|reveal| !reveal.is_empty());
            if !left {
                continue;
            }
            let op = self.fresh_op();
            let reveal = NarrationOp::Reveal {
                target: id,
                deep: true,
            };
            self.queue
                .push(QueuedOp::new(op, reveal, Anchor::Immediate { stream }));
            queued = true;
        }
        queued
    }

    /// Reveals diagram `id` whole, next in line.
    fn queue_auto_reveal(&mut self, layer: LayerId, op: OpId, id: &ElementId) {
        if layer != self.thread.active_layer() {
            return;
        }
        let stream = self
            .reply
            .as_ref()
            .map_or(0, |reply| reply.streams.len().saturating_sub(1));
        self.queue.push_front(QueuedOp::new(
            op,
            NarrationOp::Reveal {
                target: id.clone(),
                deep: true,
            },
            Anchor::Immediate { stream },
        ));
    }

    /// The reply has finished: after a big reveal, the diagram is shown whole, once.
    pub(super) fn show_overview(&mut self, effects: &mut Vec<Effect>) {
        for (layer, parts) in self.staging.take_revealed() {
            if parts < DIAGRAM_OVERVIEW_PARTS_MIN {
                continue;
            }
            let Some(bounds) = self
                .board
                .layer(layer)
                .and_then(crate::board::Layer::bounds)
            else {
                continue;
            };
            let frame = layout::inflate(bounds, CAMERA_MARGIN_UNITS, 0);
            if let Some(event) = self.cameras.frame(layer, frame, false) {
                effects.push(Effect::Ui(event));
            }
        }
    }
}

/// Why a diagram wasn't staged, as the talker is told.
pub(super) fn not_staged_because(refused: &StageRefused) -> &'static str {
    match refused {
        StageRefused::LayerMissing => "the layer it was for has closed",
        StageRefused::Empty => "none of its parts could be kept",
        StageRefused::Full => "this layer holds as many diagrams as it can; turn the page",
        StageRefused::NeedsPage => "there was no room for it",
    }
}

/// The element a node or edge tag names: a node's id, an edge's `from>to`.
fn drawing_tag_target(op: &NarrationOp) -> Option<ElementId> {
    if let NarrationOp::Node { id, .. } = op {
        Some(id.clone())
    } else if let NarrationOp::Edge { from, to, .. } = op {
        ElementId::parse(&format!("{}>{}", from.as_str(), to.as_str()))
    } else {
        None
    }
}
