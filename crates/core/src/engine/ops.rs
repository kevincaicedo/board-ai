//! Speech-to-stroke scheduling (LLD §2.4) and applying ops to the board, thread and surfaces.
//!
//! Ops run strictly in narration order. An op is due once the word it is anchored to has begun
//! playing (I1); drawing ops then start at least [`OP_SPACING_MIN_MS`] apart, each with a budget
//! that reaches the next op's anchor, clamped to [`OP_BUDGET_MIN_MS`]–[`OP_BUDGET_MAX_MS`]. A
//! frozen stroke on the layer finishes before anything new is drawn there. When a new element
//! starts, the camera follows first, so the view eases toward the stroke as it begins.

use std::collections::{BTreeSet, VecDeque};

use crate::board::{AnnotationRequest, EdgeRequest, NodeRequest, Placed, Rejected, layout};
use crate::effect::{Dropped, Effect, LogRecord, Milestone};
use crate::engine::{Core, milestone};
use crate::geometry::Rect;
use crate::ids::{ClauseId, ElementId, LayerId, OpId};
use crate::limits::{OP_BUDGET_MAX_MS, OP_BUDGET_MIN_MS, OP_SPACING_MIN_MS};
use crate::narration::ops::NarrationOp;
use crate::protocol::{DrawOp, LineStyle, UiEvent};
use crate::surfaces::SurfaceRefused;
use crate::thread::Detour;
use crate::time::SessionTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Anchor {
    /// The first spoken char after the tag: a speakable byte offset in a clause.
    Spoken { clause: ClauseId, offset: usize },
    /// No speech in the stream: due when the stream ends.
    Immediate { stream: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct QueuedOp {
    id: OpId,
    op: NarrationOp,
    anchor: Anchor,
    /// Laid out (the element is `Pending`) while it waits for its drawing slot.
    placed: Option<(LayerId, Placed)>,
}

impl QueuedOp {
    pub(super) fn new(id: OpId, op: NarrationOp, anchor: Anchor) -> Self {
        Self {
            id,
            op,
            anchor,
            placed: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct OpQueue {
    ops: VecDeque<QueuedOp>,
    pub next_op: u32,
    next_draw_at: SessionTime,
    /// Ops of the current reply that ran; their tags are part of what was heard.
    pub fired: BTreeSet<OpId>,
}

impl OpQueue {
    pub(super) fn push(&mut self, op: QueuedOp) {
        self.ops.push_back(op);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The detour depth once every queued op has run, so a new stream refuses a fourth detour
    /// while it is being written.
    pub(super) fn planned_depth(&self, depth: usize) -> usize {
        self.ops
            .iter()
            .fold(depth, |depth, queued| match queued.op {
                NarrationOp::Detour { .. } => depth + 1,
                NarrationOp::Resume => depth.saturating_sub(1),
                NarrationOp::Plan { .. }
                | NarrationOp::Step { .. }
                | NarrationOp::Node { .. }
                | NarrationOp::Edge { .. }
                | NarrationOp::Annotate { .. }
                | NarrationOp::Strike { .. }
                | NarrationOp::Focus { .. }
                | NarrationOp::Code { .. }
                | NarrationOp::Line { .. }
                | NarrationOp::Trace { .. }
                | NarrationOp::Compare { .. }
                | NarrationOp::Cell { .. }
                | NarrationOp::Close { .. }
                | NarrationOp::MarkCode { .. }
                | NarrationOp::Recap { .. }
                | NarrationOp::Participant { .. }
                | NarrationOp::Message { .. }
                | NarrationOp::Cells(_)
                | NarrationOp::Write { .. }
                | NarrationOp::Cite { .. } => depth,
            })
    }
}

/// Whether an op can run yet.
enum Due {
    Waiting,
    Now,
    /// Anchored in a clause that will never play: it is never drawn.
    Never,
}

/// What an edge or a message joins.
fn link<'a>(
    from: &'a ElementId,
    to: &'a ElementId,
    label: &'a Option<String>,
    style: LineStyle,
) -> EdgeRequest<'a> {
    EdgeRequest {
        from,
        to,
        label: label.as_deref(),
        style,
    }
}

fn clamp_budget(ms: u64) -> u32 {
    u32::try_from(ms.clamp(u64::from(OP_BUDGET_MIN_MS), u64::from(OP_BUDGET_MAX_MS)))
        .unwrap_or(OP_BUDGET_MAX_MS)
}

impl Core {
    /// Runs every op whose words have played, in order, respecting the drawing slots.
    pub(super) fn advance_ops(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if self.stop.is_some() {
            return;
        }
        // Each pass runs or drops an op, or stops; the queue's length bounds the loop.
        for _ in 0..=self.queue.ops.len() {
            let Some(mut queued) = self.queue.ops.pop_front() else {
                return;
            };
            match self.due(queued.anchor) {
                Due::Waiting => {
                    self.queue.ops.push_front(queued);
                    return;
                }
                Due::Never => continue,
                Due::Now => {}
            }
            if !queued.op.draws() {
                self.apply(now, &queued, effects);
                continue;
            }
            if queued.placed.is_none() {
                match self.place(&queued) {
                    Ok((_, Placed::Unchanged)) => {
                        self.queue.fired.insert(queued.id);
                        continue;
                    }
                    Ok(placed) => queued.placed = Some(placed),
                    Err(_) => {
                        effects.push(Effect::Log(LogRecord::Dropped {
                            what: Dropped::Op,
                            count: 1,
                        }));
                        continue;
                    }
                }
            }
            if let Some((layer, placed)) = &queued.placed
                && !matches!(placed, Placed::ResumeFrozen)
                && now >= self.queue.next_draw_at
            {
                self.resume_frozen(now, *layer, effects);
            }
            if now < self.queue.next_draw_at {
                let at = self.queue.next_draw_at;
                self.queue.ops.push_front(queued);
                self.wake(at, effects);
                return;
            }
            self.start_draw(now, queued, effects);
        }
    }

    fn due(&self, anchor: Anchor) -> Due {
        match anchor {
            Anchor::Spoken { clause, offset } => {
                if self.voice.clause(clause).is_none() || self.voice.skipped(clause) {
                    Due::Never
                } else if self.voice.reached(clause, offset) {
                    Due::Now
                } else {
                    Due::Waiting
                }
            }
            Anchor::Immediate { stream } => {
                let stopped = self.reply.as_ref().is_none_or(|reply| {
                    reply
                        .streams
                        .get(stream)
                        .is_none_or(super::reply::Stream::has_stopped)
                });
                if stopped { Due::Now } else { Due::Waiting }
            }
        }
    }

    /// Lays a drawing op out on the active layer.
    fn place(&mut self, queued: &QueuedOp) -> Result<(LayerId, Placed), Rejected> {
        let layer = self.thread.active_layer();
        let placed = match &queued.op {
            NarrationOp::Node {
                id,
                label,
                shape,
                placement,
                stack,
                fill,
            } => {
                let node = NodeRequest {
                    id,
                    label,
                    shape: *shape,
                    placement,
                    stack: *stack,
                    fill: *fill,
                };
                self.board.place_node(layer, queued.id, node)?
            }
            NarrationOp::Edge {
                from,
                to,
                label,
                style,
            } => self
                .board
                .place_edge(layer, queued.id, link(from, to, label, *style))?,
            NarrationOp::Annotate {
                target,
                mark,
                text,
                fill,
            } => self.board.place_annotation(
                layer,
                queued.id,
                AnnotationRequest {
                    target,
                    mark: *mark,
                    text: text.as_deref(),
                    fill: *fill,
                },
            )?,
            NarrationOp::Strike { target, text } => {
                self.board.place_strike(layer, queued.id, target, text)?
            }
            NarrationOp::Trace { hops } => self.board.place_trace(layer, queued.id, hops)?,
            NarrationOp::Participant { .. }
            | NarrationOp::Message { .. }
            | NarrationOp::Cells(_)
            | NarrationOp::Write { .. } => self.place_diagram(layer, queued)?,
            // Ops that don't draw are applied, never laid out.
            NarrationOp::Cite { .. }
            | NarrationOp::Plan { .. }
            | NarrationOp::Step { .. }
            | NarrationOp::Focus { .. }
            | NarrationOp::Detour { .. }
            | NarrationOp::Resume
            | NarrationOp::Code { .. }
            | NarrationOp::Line { .. }
            | NarrationOp::Compare { .. }
            | NarrationOp::Cell { .. }
            | NarrationOp::Close { .. }
            | NarrationOp::MarkCode { .. }
            | NarrationOp::Recap { .. } => Placed::Unchanged,
        };
        Ok((layer, placed))
    }

    /// Lays out a sequence diagram's or a row of cells' op.
    fn place_diagram(&mut self, layer: LayerId, queued: &QueuedOp) -> Result<Placed, Rejected> {
        let op = queued.id;
        match &queued.op {
            NarrationOp::Participant { id, label } => {
                self.board.place_participant(layer, op, id, label)
            }
            NarrationOp::Message {
                from,
                to,
                label,
                style,
            } => self
                .board
                .place_message(layer, op, link(from, to, label, *style)),
            NarrationOp::Cells(spec) => self.board.place_cells(layer, op, spec),
            NarrationOp::Write { target, text } => self.board.place_write(layer, op, target, text),
            NarrationOp::Node { .. }
            | NarrationOp::Edge { .. }
            | NarrationOp::Annotate { .. }
            | NarrationOp::Strike { .. }
            | NarrationOp::Trace { .. }
            | NarrationOp::Cite { .. }
            | NarrationOp::Plan { .. }
            | NarrationOp::Step { .. }
            | NarrationOp::Focus { .. }
            | NarrationOp::Detour { .. }
            | NarrationOp::Resume
            | NarrationOp::Code { .. }
            | NarrationOp::Line { .. }
            | NarrationOp::Compare { .. }
            | NarrationOp::Cell { .. }
            | NarrationOp::Close { .. }
            | NarrationOp::MarkCode { .. }
            | NarrationOp::Recap { .. } => Ok(Placed::Unchanged),
        }
    }

    fn start_draw(&mut self, now: SessionTime, queued: QueuedOp, effects: &mut Vec<Effect>) {
        let Some((layer, placed)) = queued.placed.clone() else {
            return;
        };
        let budget_ms = self.budget(now, &queued);
        match placed {
            Placed::New { id, draw } => {
                let growth = self.board.start(layer, &id, now, budget_ms);
                let stroke = growth
                    .iter()
                    .map(layout::op_bounds)
                    .fold(layout::op_bounds(&draw), Rect::union);
                self.follow_camera(layer, stroke, effects);
                self.op_start(now, layer, draw, budget_ms, effects);
                if let Some(growth) = growth {
                    self.op_start(now, layer, growth, budget_ms, effects);
                }
            }
            Placed::Strike { target, draw } => {
                if let DrawOp::Strike { label, .. } = &draw {
                    self.board.apply_strike(layer, &target, label);
                }
                self.follow_camera(layer, layout::op_bounds(&draw), effects);
                self.op_start(now, layer, draw, budget_ms, effects);
            }
            Placed::Write { target, draw } => {
                if let DrawOp::Write { label, .. } = &draw {
                    self.board.apply_write(layer, &target, label);
                }
                self.follow_camera(layer, layout::op_bounds(&draw), effects);
                self.op_start(now, layer, draw, budget_ms, effects);
            }
            Placed::Before { id, draw } => {
                let _ = self.board.start(layer, &id, now, budget_ms);
                self.follow_camera(layer, layout::op_bounds(&draw), effects);
                self.op_start(now, layer, draw, budget_ms, effects);
                // The op itself is laid out again at its next slot, with what it needed drawn.
                self.queue.ops.push_front(QueuedOp::new(
                    queued.id,
                    queued.op.clone(),
                    queued.anchor,
                ));
            }
            Placed::ResumeFrozen => self.resume_frozen(now, layer, effects),
            Placed::Unchanged => {}
        }
        self.queue.fired.insert(queued.id);
        self.queue.next_draw_at = self
            .queue
            .next_draw_at
            .max(now.plus_ms(u64::from(OP_SPACING_MIN_MS)));
    }

    fn op_start(
        &mut self,
        now: SessionTime,
        layer: LayerId,
        op: DrawOp,
        budget_ms: u32,
        effects: &mut Vec<Effect>,
    ) {
        effects.push(Effect::Ui(UiEvent::OpStart {
            layer,
            op,
            budget_ms,
        }));
        if !self.marks.first_op {
            self.marks.first_op = true;
            milestone(effects, Milestone::FirstOp);
        }
        self.drawing_until(now.plus_ms(u64::from(budget_ms)));
    }

    /// The orb shows Drawing until `until` (plus its linger); the latest end wins.
    fn drawing_until(&mut self, until: SessionTime) {
        self.drawing_until = Some(self.drawing_until.map_or(until, |known| known.max(until)));
    }

    /// A stroke covering `stroke` is about to start on `layer`: move the camera if the view no
    /// longer holds it, or the layer's drawing outgrew the view.
    fn follow_camera(&mut self, layer: LayerId, stroke: Rect, effects: &mut Vec<Effect>) {
        let drawing = self
            .board
            .layer(layer)
            .and_then(crate::board::Layer::bounds)
            .map_or(stroke, |bounds| bounds.union(stroke));
        if let Some(event) = self.cameras.follow(layer, drawing, stroke) {
            effects.push(Effect::Ui(event));
        }
    }

    /// The frozen stroke on `layer` continues from where it stopped, and finishes before the
    /// next stroke starts.
    pub(super) fn resume_frozen(
        &mut self,
        now: SessionTime,
        layer: LayerId,
        effects: &mut Vec<Effect>,
    ) {
        let Some(remaining_ms) = self.board.resume_frozen(layer, now) else {
            return;
        };
        let budget_ms = clamp_budget(u64::from(remaining_ms));
        effects.push(Effect::Ui(UiEvent::Resume { layer, budget_ms }));
        let finished = now.plus_ms(u64::from(budget_ms));
        self.queue.next_draw_at = self.queue.next_draw_at.max(finished);
        self.drawing_until(finished);
    }

    /// Time until the next drawing op's anchor plays, or until this op's clause ends.
    fn budget(&self, now: SessionTime, queued: &QueuedOp) -> u32 {
        let next = self
            .queue
            .ops
            .iter()
            .find(|op| op.op.draws())
            .map(|op| op.anchor);
        let ms = match (next, queued.anchor) {
            (Some(Anchor::Spoken { clause, offset }), _) => {
                self.voice.estimate_ms_until(clause, offset, now)
            }
            (None, Anchor::Spoken { clause, .. }) => {
                self.voice.estimate_ms_until(clause, usize::MAX, now)
            }
            (Some(Anchor::Immediate { .. }), _) => u64::from(OP_SPACING_MIN_MS),
            (None, Anchor::Immediate { .. }) => u64::from(OP_BUDGET_MIN_MS),
        };
        clamp_budget(ms)
    }

    /// Drops every op not started; returns how many would have drawn.
    pub(super) fn drop_queued_ops(&mut self) -> usize {
        let mut dropped = 0;
        for queued in std::mem::take(&mut self.queue.ops) {
            if let Some((layer, Placed::New { id, .. } | Placed::Before { id, .. })) =
                &queued.placed
            {
                self.board.remove_pending(*layer, id);
            }
            if queued.op.draws() {
                dropped += 1;
            }
        }
        dropped
    }

    /// Ops that change the thread, the camera or a surface rather than drawing.
    fn apply(&mut self, now: SessionTime, queued: &QueuedOp, effects: &mut Vec<Effect>) {
        let mut events = Vec::new();
        let applied = match &queued.op {
            NarrationOp::Plan { steps } => {
                self.thread.set_plan(steps.clone());
                events.push(self.thread.event());
                true
            }
            NarrationOp::Step { step } => {
                let moved = self.thread.set_step(step);
                if moved {
                    events.push(self.thread.event());
                }
                moved
            }
            NarrationOp::Focus { target } => self.focus(target, &mut events),
            NarrationOp::Detour { title } => self.open_detour(title, &mut events, effects),
            NarrationOp::Resume => {
                self.close_detour(&mut events);
                effects.extend(events.drain(..).map(Effect::Ui));
                let layer = self.thread.active_layer();
                self.resume_frozen(now, layer, effects);
                true
            }
            NarrationOp::Code { .. }
            | NarrationOp::Line { .. }
            | NarrationOp::Compare { .. }
            | NarrationOp::Cell { .. }
            | NarrationOp::Close { .. }
            | NarrationOp::MarkCode { .. }
            | NarrationOp::Recap { .. } => self.apply_surface(&queued.op, &mut events),
            // A citation is speech, not a tag: one that shows no chip drops nothing.
            NarrationOp::Cite { path, line } => {
                self.cite(path, *line, &mut events);
                true
            }
            NarrationOp::Node { .. }
            | NarrationOp::Edge { .. }
            | NarrationOp::Annotate { .. }
            | NarrationOp::Strike { .. }
            | NarrationOp::Trace { .. }
            | NarrationOp::Participant { .. }
            | NarrationOp::Message { .. }
            | NarrationOp::Cells(_)
            | NarrationOp::Write { .. } => false,
        };
        effects.extend(events.into_iter().map(Effect::Ui));
        if applied {
            self.queue.fired.insert(queued.id);
        } else {
            effects.push(Effect::Log(LogRecord::Dropped {
                what: Dropped::Op,
                count: 1,
            }));
        }
    }

    /// Ops on the surfaces beside the canvas; false when the op did nothing.
    fn apply_surface(&mut self, op: &NarrationOp, events: &mut Vec<UiEvent>) -> bool {
        match op {
            NarrationOp::Code { path, lines } => {
                let shown = self.surfaces.open_code(path, *lines, events);
                self.surface_refused(shown)
            }
            NarrationOp::Line { line } => {
                let shown = self.surfaces.highlight(*line, events);
                self.surface_refused(shown)
            }
            NarrationOp::Compare {
                options,
                criteria,
                title,
            } => {
                self.surfaces.open_compare(
                    options.clone(),
                    criteria.clone(),
                    title.clone(),
                    events,
                );
                true
            }
            NarrationOp::Cell {
                criterion,
                option,
                mark,
            } => {
                let filled = self
                    .surfaces
                    .fill_cell(criterion, option, mark.clone(), events);
                self.surface_refused(filled)
            }
            NarrationOp::Close { surface } => self.surfaces.close(*surface, events).is_ok(),
            NarrationOp::MarkCode { line, code, mark } => {
                let marked = self
                    .surfaces
                    .mark_code(*line, code.as_deref(), *mark, events);
                self.surface_refused(marked)
            }
            NarrationOp::Recap { bullets } => {
                self.surfaces.open_recap(bullets.clone(), events);
                true
            }
            NarrationOp::Plan { .. }
            | NarrationOp::Step { .. }
            | NarrationOp::Node { .. }
            | NarrationOp::Edge { .. }
            | NarrationOp::Annotate { .. }
            | NarrationOp::Strike { .. }
            | NarrationOp::Focus { .. }
            | NarrationOp::Detour { .. }
            | NarrationOp::Resume
            | NarrationOp::Trace { .. }
            | NarrationOp::Participant { .. }
            | NarrationOp::Message { .. }
            | NarrationOp::Cells(_)
            | NarrationOp::Write { .. }
            | NarrationOp::Cite { .. } => false,
        }
    }

    fn surface_refused(&mut self, result: Result<(), SurfaceRefused>) -> bool {
        let Err(refused) = result else {
            return true;
        };
        if let SurfaceRefused::NotRead { path } = refused {
            self.context.notice(format!(
                "The code card for {path} was not shown: those lines were not read in this \
                 session. Read the file first, then show it."
            ));
        }
        false
    }

    fn focus(&mut self, target: &ElementId, events: &mut Vec<UiEvent>) -> bool {
        let layer = self.thread.active_layer();
        let Some(bounds) = self
            .board
            .layer(layer)
            .and_then(|board_layer| board_layer.element(target))
            .map(|element| element.geometry.bounds())
        else {
            return false;
        };
        events.push(self.cameras.focus(layer, target, bounds));
        true
    }

    fn open_detour(
        &mut self,
        title: &str,
        events: &mut Vec<UiEvent>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let refused = || {
            Effect::Log(LogRecord::Dropped {
                what: Dropped::Detour,
                count: 1,
            })
        };
        let Some(layer) = self.board.push_layer(title) else {
            effects.push(refused());
            return false;
        };
        let detour = Detour {
            title: title.to_owned(),
            layer,
        };
        if self.thread.push_detour(detour).is_err() {
            self.board.close_layer(layer);
            effects.push(refused());
            return false;
        }
        events.push(UiEvent::LayerPush {
            layer,
            title: title.to_owned(),
            question: self.turn_question.clone(),
        });
        events.push(self.thread.event());
        true
    }

    fn close_detour(&mut self, events: &mut Vec<UiEvent>) {
        let Some(detour) = self.thread.pop_detour() else {
            return;
        };
        self.board.close_layer(detour.layer);
        events.push(UiEvent::LayerPop {
            layer: detour.layer,
        });
        events.push(self.thread.event());
    }
}
