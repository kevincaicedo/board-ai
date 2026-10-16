//! Sequence diagrams (D-150). A layer's participants stand in one row, left to right in the order
//! they were drawn, each a head box with a dashed lifeline down from it. Messages go down in
//! order, one row each (a message to itself takes a loop's height more). Lifelines grow as the
//! messages need them: a message carries the stretch each lifeline needs to reach below it, so
//! the head and every stroke already drawn stay where they are (I6). A layer holds one sequence;
//! a detour draws its own.

use crate::board::layout::{self, CellSpan};
use crate::board::place::repeated;
use crate::board::{
    Board, EdgeRequest, Element, ElementKind, ElementState, Geometry, Layer, Placed, Rejected,
    generated_id,
};
use crate::geometry::Point;
use crate::ids::{ElementId, LayerId, OpId};
use crate::narration::ops::Direction;
use crate::protocol::{DrawOp, Lifeline};

/// From a head's bottom to the first message.
const FIRST_MESSAGE_GAP: i32 = 44;

/// From one message to the next.
const MESSAGE_ROW: i32 = 52;

/// A message to itself: how far its loop reaches right, and how far down it comes back.
const SELF_LOOP_WIDTH: i32 = 44;
const SELF_LOOP_HEIGHT: i32 = 26;

/// How far a new lifeline reaches below its head: room for the first two messages.
const LIFELINE_START: i32 = FIRST_MESSAGE_GAP + 2 * MESSAGE_ROW;

/// A lifeline runs at least this far past the lowest message; when it wouldn't, every lifeline
/// grows to two rows below that message, so they grow every other message rather than each.
const LIFELINE_TAIL: i32 = MESSAGE_ROW / 2;

/// Where a layer's sequence diagram stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sequence {
    /// Where the next message row goes; `None` until the first participant.
    next_y: Option<i32>,
    /// Where every lifeline ends.
    bottom: i32,
}

/// What starting a message does to its sequence: where the next row goes, and where the
/// lifelines end. Kept with the message until it starts, so a message dropped before it was
/// drawn changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowCommit {
    pub next_y: i32,
    pub bottom: i32,
}

impl Layer {
    pub(super) fn is_participant(&self, id: &ElementId) -> bool {
        self.element(id)
            .is_some_and(|element| matches!(element.kind, ElementKind::Participant { .. }))
    }

    /// A participant's lifeline: its x, and where it ends now.
    fn lifeline(&self, id: &ElementId) -> Option<(i32, i32)> {
        let element = self.element(id)?;
        if let (
            ElementKind::Participant {
                lifeline_bottom, ..
            },
            Geometry::Box(head),
        ) = (&element.kind, &element.geometry)
        {
            Some((head.center().x, *lifeline_bottom))
        } else {
            None
        }
    }

    /// A message started: the next row moves down, and the lifelines reach what it needed.
    pub(super) fn commit_row(&mut self, commit: RowCommit) {
        self.sequence.next_y = Some(commit.next_y);
        if commit.bottom <= self.sequence.bottom {
            return;
        }
        self.sequence.bottom = commit.bottom;
        let mut covered = Vec::new();
        for element in &mut self.elements {
            if let (
                ElementKind::Participant {
                    lifeline_bottom, ..
                },
                Geometry::Box(head),
            ) = (&mut element.kind, &element.geometry)
            {
                *lifeline_bottom = (*lifeline_bottom).max(commit.bottom);
                let line = layout::lifeline_box(head.center().x, head.bottom(), *lifeline_bottom);
                covered.push((line, element.id.clone()));
            }
        }
        for (line, id) in covered {
            self.grid.occupy_rect(line, &id);
        }
    }
}

impl Board {
    /// A participant's head, right of the last one on the layer; the first one starts a row
    /// under everything drawn so far, so its lifeline has room to grow.
    pub fn place_participant(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        id: &ElementId,
        label: &str,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        if let Some(existing) = layer.element(id) {
            // A participant drawn for a message first ("server" → "Server") isn't corrected by
            // the talker naming it afterwards.
            if existing.label.eq_ignore_ascii_case(label) {
                return Ok(Placed::Unchanged);
            }
            return Ok(repeated(existing, op, label));
        }
        let (width, height) = layout::head_size(label);
        let cols = layout::columns_for(width);
        let last = layer.elements().iter().rev().find_map(|element| {
            if let ElementKind::Participant { span, .. } = element.kind {
                Some(span)
            } else {
                None
            }
        });
        let cell = match last {
            Some(span) => layer.grid.find(
                layout::start_next_to(span, cols, Direction::Right),
                cols,
                Direction::Right,
                None,
            ),
            None => layout::Cell {
                row: layer.grid.below_everything(),
                col: layer.grid.leftmost_col(),
            },
        };
        let span = CellSpan { cell, cols };
        let head = layout::rect_in(span, width, height);
        let lifeline_bottom = layer.sequence.bottom.max(head.bottom() + LIFELINE_START);
        layer.sequence.bottom = lifeline_bottom;
        if layer.sequence.next_y.is_none() {
            layer.sequence.next_y = Some(head.bottom() + FIRST_MESSAGE_GAP);
        }
        layer.grid.occupy(span, id);
        let line = layout::lifeline_box(head.center().x, head.bottom(), lifeline_bottom);
        layer.grid.occupy_rect(line, id);
        layer.insert(Element {
            id: id.clone(),
            op,
            kind: ElementKind::Participant {
                span,
                lifeline_bottom,
            },
            label: label.to_owned(),
            geometry: Geometry::Box(head),
            state: ElementState::Pending,
        })?;
        Ok(Placed::New {
            id: id.clone(),
            draw: DrawOp::Participant {
                id: id.clone(),
                op,
                label: label.to_owned(),
                rect: head,
                lifeline_bottom,
            },
        })
    }

    /// A message in the next row between two participants' lifelines (or a loop back to the
    /// same one). Between anything else it is an edge.
    pub fn place_message(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        message: EdgeRequest<'_>,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let ends = (layer.lifeline(message.from), layer.lifeline(message.to));
        let ((from_x, _), (to_x, _)) = match ends {
            (Some(from), Some(to)) => (from, to),
            (None, None) | (Some(_), None) | (None, Some(_)) => {
                return self.place_unknown_end(layer_id, op, message);
            }
        };
        let y = layer.sequence.next_y.unwrap_or(0);
        let points = arrow(from_x, to_x, y, message.from == message.to);
        let lowest = points.iter().map(|point| point.y).max().unwrap_or(y);
        let next_y = lowest + MESSAGE_ROW;
        let bottom = if lowest + LIFELINE_TAIL > layer.sequence.bottom {
            lowest + 2 * MESSAGE_ROW
        } else {
            layer.sequence.bottom
        };
        let extend: Vec<Lifeline> = layer
            .elements()
            .iter()
            .filter_map(|element| {
                let (x, top) = layer.lifeline(&element.id)?;
                (top < bottom).then(|| Lifeline {
                    participant: element.id.clone(),
                    x,
                    top,
                    bottom,
                })
            })
            .collect();
        let id = generated_id("m", op);
        layer.insert(Element {
            id: id.clone(),
            op,
            kind: ElementKind::Message {
                from: message.from.clone(),
                to: message.to.clone(),
                style: message.style,
                commit: RowCommit { next_y, bottom },
            },
            label: message.label.unwrap_or_default().to_owned(),
            geometry: Geometry::Points(points.clone()),
            state: ElementState::Pending,
        })?;
        Ok(Placed::New {
            id: id.clone(),
            draw: DrawOp::Message {
                id,
                op,
                from: message.from.clone(),
                to: message.to.clone(),
                points,
                label: message.label.map(str::to_owned),
                style: message.style,
                extend,
            },
        })
    }
}

impl Board {
    /// A message with an end that isn't a participant. On a layer with a sequence diagram, an
    /// end nobody drew is drawn first as a participant named by its id (live runs lost whole
    /// handshakes to a forgotten `participant`); otherwise the message is an edge.
    fn place_unknown_end(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        message: EdgeRequest<'_>,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let has_sequence = layer
            .elements()
            .iter()
            .any(|element| matches!(element.kind, ElementKind::Participant { .. }));
        let missing = [message.from, message.to]
            .into_iter()
            .find(|end| layer.element(end).is_none());
        if let (true, Some(missing)) = (has_sequence, missing) {
            let missing = missing.clone();
            let label = capitalized(missing.as_str());
            return Ok(
                match self.place_participant(layer_id, op, &missing, &label)? {
                    Placed::New { id, draw } => Placed::Before { id, draw },
                    other @ (Placed::Strike { .. }
                    | Placed::Write { .. }
                    | Placed::Before { .. }
                    | Placed::ResumeFrozen
                    | Placed::Unchanged) => other,
                },
            );
        }
        if message.from == message.to {
            return Err(Rejected::NotParticipant(message.from.clone()));
        }
        self.place_edge(layer_id, op, message)
    }
}

/// A message's arrow at row `y`: across from one lifeline to the other, or a loop back to the
/// same one.
fn arrow(from_x: i32, to_x: i32, y: i32, to_itself: bool) -> Vec<Point> {
    if !to_itself {
        return vec![Point { x: from_x, y }, Point { x: to_x, y }];
    }
    let (right, back) = (from_x + SELF_LOOP_WIDTH, y + SELF_LOOP_HEIGHT);
    vec![
        Point { x: from_x, y },
        Point { x: right, y },
        Point { x: right, y: back },
        Point { x: from_x, y: back },
    ]
}

/// An id as a label: its first letter upper case ("server" → "Server").
fn capitalized(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
