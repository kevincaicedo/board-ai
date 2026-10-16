//! Placement off the grid. A tree node sits under its parent, a hash chain's entry beside its
//! bucket, a linked list's next box beside the last: lined up with what they hang from, which
//! the 200 × 140 grid can't do. Such a drawing is nudged past anything in its way, never the
//! other way round (I6), and marks the grid cells it covers so grid placement keeps clear of it.

use crate::board::{ElementKind, Layer, layout};
use crate::geometry::Rect;
use crate::limits::FREE_NUDGE_STEPS_MAX;
use crate::narration::ops::Direction;
use crate::protocol::NodeShape;

/// Space between a drawing and the one it is placed beside, across and down.
pub(super) const BESIDE_GAP_X: i32 = 64;
pub(super) const BESIDE_GAP_Y: i32 = 56;

/// Space a nudged drawing keeps from the one it was moved past: room for an arrow between them.
const NUDGE_GAP: i32 = 48;

/// Room kept between a drawing placed off the grid and what is already there.
const CLEARANCE: i32 = 12;

/// What a new drawing must keep clear of: every node, row of cells and participant laid out on
/// the layer, labels included. Group frames hold other things, and connectors, marks, messages
/// and single cells (their row covers them) are drawn over or between.
fn obstacles(layer: &Layer) -> Vec<Rect> {
    layer
        .elements()
        .iter()
        .filter(|element| match &element.kind {
            ElementKind::Node { shape, .. } => *shape != NodeShape::Group,
            ElementKind::Cells { .. } | ElementKind::Participant { .. } => true,
            ElementKind::Edge { .. }
            | ElementKind::Annotation { .. }
            | ElementKind::Trace
            | ElementKind::Message { .. }
            | ElementKind::Cell { .. } => false,
        })
        .map(crate::board::Element::area)
        .collect()
}

/// A box `width` × `height` beside `target` in `direction`, lined up with its middle.
#[must_use]
pub(super) fn next_to(target: Rect, width: u32, height: u32, direction: Direction) -> Rect {
    let (w, h) = (
        i32::try_from(width).unwrap_or(0),
        i32::try_from(height).unwrap_or(0),
    );
    let middle = target.center();
    let (x, y) = match direction {
        Direction::Right => (target.right() + BESIDE_GAP_X, middle.y - h / 2),
        Direction::Left => (target.x - BESIDE_GAP_X - w, middle.y - h / 2),
        Direction::Below => (middle.x - w / 2, target.bottom() + BESIDE_GAP_Y),
        Direction::Above => (middle.x - w / 2, target.y - BESIDE_GAP_Y - h),
    };
    Rect {
        x,
        y,
        w: width,
        h: height,
    }
}

/// `next_to`, nudged clear of what is already there.
#[must_use]
pub(super) fn beside(
    layer: &Layer,
    target: Rect,
    width: u32,
    height: u32,
    direction: Direction,
) -> Rect {
    clear(layer, next_to(target, width, height, direction), direction)
}

/// `candidate`, or the first box past it along `direction` clear of every obstacle, each
/// nudge moving it just past what it would land on; under everything past the nudge limit.
#[must_use]
pub(super) fn clear(layer: &Layer, candidate: Rect, direction: Direction) -> Rect {
    let obstacles = obstacles(layer);
    let mut rect = candidate;
    let (width, height) = (
        i32::try_from(candidate.w).unwrap_or(0),
        i32::try_from(candidate.h).unwrap_or(0),
    );
    for _ in 0..FREE_NUDGE_STEPS_MAX {
        let room = layout::inflate(rect, CLEARANCE, 0);
        let mut blocking = obstacles
            .iter()
            .filter(|obstacle| obstacle.intersects(room));
        let Some(first) = blocking.next() else {
            return rect;
        };
        // Past the farthest of what is in the way, so every nudge makes progress.
        let past = blocking.fold(*first, |far, obstacle| match direction {
            Direction::Right if obstacle.right() > far.right() => *obstacle,
            Direction::Left if obstacle.x < far.x => *obstacle,
            Direction::Below if obstacle.bottom() > far.bottom() => *obstacle,
            Direction::Above if obstacle.y < far.y => *obstacle,
            Direction::Right | Direction::Left | Direction::Below | Direction::Above => far,
        });
        match direction {
            Direction::Right => rect.x = past.right() + NUDGE_GAP,
            Direction::Left => rect.x = past.x - NUDGE_GAP - width,
            Direction::Below => rect.y = past.bottom() + NUDGE_GAP,
            Direction::Above => rect.y = past.y - NUDGE_GAP - height,
        }
    }
    let below = obstacles
        .iter()
        .map(|obstacle| obstacle.bottom())
        .max()
        .unwrap_or(candidate.y);
    Rect {
        y: below + BESIDE_GAP_Y,
        ..candidate
    }
}
