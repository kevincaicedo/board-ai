//! An arrow you drew, tidied: each end that lands on a shape, or within [`SNAP_UNITS`] of one,
//! connects to it and moves onto its outline along the line you drew. Pure geometry over the
//! layer's elements, one pass per end, so it is bounded by the layer's element count.

use crate::board::{ElementKind, Geometry, Layer, UserMark};
use crate::geometry::{Point, Rect};
use crate::ids::ElementId;
use crate::protocol::NodeShape;

use super::points_rect;
use crate::limits::USER_SHAPE_MIN_UNITS;

/// How far outside a shape an arrow's end may land and still connect to it: a hand lifting the
/// pen near a box means the box.
pub const SNAP_UNITS: i32 = 24;

/// An arrow after snapping: its ends, and what each connects to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapped {
    pub start: Point,
    pub end: Point,
    pub from: Option<ElementId>,
    pub to: Option<ElementId>,
}

/// Connects the arrow `start` → `end` to the shapes its ends land on. Both ends on the same shape
/// connect nothing (a scribble inside a box), and ends that would meet once moved stay as drawn.
#[must_use]
pub fn snap(layer: &Layer, start: Point, end: Point) -> Snapped {
    let from = target_at(layer, start);
    let to = target_at(layer, end);
    let as_drawn = Snapped {
        start,
        end,
        from: None,
        to: None,
    };
    if let (Some((from, _)), Some((to, _))) = (&from, &to)
        && from == to
    {
        return as_drawn;
    }
    let moved_start = from
        .as_ref()
        .map_or(start, |(_, rect)| onto_outline(*rect, start, end));
    let moved_end = to
        .as_ref()
        .map_or(end, |(_, rect)| onto_outline(*rect, end, start));
    let reach = points_rect(&[moved_start, moved_end]);
    let (start, end) = if reach.w.max(reach.h) >= USER_SHAPE_MIN_UNITS {
        (moved_start, moved_end)
    } else {
        (start, end)
    };
    Snapped {
        start,
        end,
        from: from.map(|(id, _)| id),
        to: to.map(|(id, _)| id),
    }
}

/// The most specific shape at `point`: the smallest whose box, grown by [`SNAP_UNITS`], holds it;
/// on a tie the one drawn last, which sits on top.
fn target_at(layer: &Layer, point: Point) -> Option<(ElementId, Rect)> {
    let mut best: Option<(ElementId, Rect)> = None;
    for element in layer.elements() {
        if !element.state.is_shown() {
            continue;
        }
        let Some(rect) = target_rect(&element.kind, &element.geometry) else {
            continue;
        };
        let reach = crate::board::layout::inflate(rect, SNAP_UNITS, 0);
        if !holds(reach, point) {
            continue;
        }
        let area = |rect: Rect| u64::from(rect.w) * u64::from(rect.h);
        if best
            .as_ref()
            .is_none_or(|(_, kept)| area(rect) <= area(*kept))
        {
            best = Some((element.id.clone(), rect));
        }
    }
    best
}

/// The box an arrow can connect to: a node (not a group's frame or a note), a participant's
/// head, a cell, or a box or pen shape of yours. Connectors and marks take no arrows.
fn target_rect(kind: &ElementKind, geometry: &Geometry) -> Option<Rect> {
    let Geometry::Box(rect) = geometry else {
        return None;
    };
    match kind {
        ElementKind::Node { shape, .. } => {
            (!matches!(shape, NodeShape::Group | NodeShape::Note)).then_some(*rect)
        }
        ElementKind::Participant { .. } | ElementKind::Cell { .. } => Some(*rect),
        ElementKind::User { mark, .. } => match mark {
            UserMark::Box | UserMark::Ink { .. } => Some(*rect),
            UserMark::Arrow { .. } => None,
        },
        ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Message { .. }
        | ElementKind::Cells { .. } => None,
    }
}

fn holds(rect: Rect, point: Point) -> bool {
    rect.x <= point.x && point.x <= rect.right() && rect.y <= point.y && point.y <= rect.bottom()
}

/// Where an end at `at` meets `rect`'s outline: inside the box, where the line toward `toward`
/// leaves it; outside (within reach), the nearest point of the outline. An arrow drawn wholly
/// inside the box keeps its end.
fn onto_outline(rect: Rect, at: Point, toward: Point) -> Point {
    if !holds(rect, at) {
        return Point {
            x: at.x.clamp(rect.x, rect.right()),
            y: at.y.clamp(rect.y, rect.bottom()),
        };
    }
    let (dx, dy) = (
        f64::from(toward.x) - f64::from(at.x),
        f64::from(toward.y) - f64::from(at.y),
    );
    let exit = |delta: f64, from: i32, low: i32, high: i32| {
        if delta > 0.0 {
            (f64::from(high) - f64::from(from)) / delta
        } else if delta < 0.0 {
            (f64::from(low) - f64::from(from)) / delta
        } else {
            f64::INFINITY
        }
    };
    let t = exit(dx, at.x, rect.x, rect.right()).min(exit(dy, at.y, rect.y, rect.bottom()));
    // The other end is inside the box too: there is nowhere along the line to leave it.
    if t >= 1.0 {
        return at;
    }
    Point {
        x: round(f64::from(at.x) + dx * t),
        y: round(f64::from(at.y) + dy * t),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "a point between two in-range coordinates, rounded, fits an i32"
)]
fn round(value: f64) -> i32 {
    value.round() as i32
}

#[cfg(test)]
mod tests {
    use super::{onto_outline, snap};
    use crate::board::Board;
    use crate::board::user::Drawn;
    use crate::geometry::{Point, Rect};
    use crate::ids::LayerId;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    #[test]
    fn an_end_inside_a_box_moves_to_where_the_line_leaves_it() {
        let boxed = rect(0, 0, 100, 60);
        assert_eq!(
            onto_outline(boxed, point(50, 30), point(250, 30)),
            point(100, 30)
        );
        assert_eq!(
            onto_outline(boxed, point(50, 30), point(50, -200)),
            point(50, 0)
        );
        // Outside, within reach: the nearest point of the outline.
        assert_eq!(
            onto_outline(boxed, point(120, 40), point(400, 40)),
            point(100, 40)
        );
        // Both ends inside: nothing to move toward.
        assert_eq!(
            onto_outline(boxed, point(10, 10), point(20, 20)),
            point(10, 10)
        );
    }

    #[test]
    fn arrows_connect_the_shapes_their_ends_land_on() {
        let mut board = Board::new();
        let main = LayerId::MAIN;
        let client = board
            .add_user_shape(main, Drawn::Box(rect(0, 0, 120, 70)), None)
            .expect("a box");
        let api = board
            .add_user_shape(main, Drawn::Box(rect(300, 0, 120, 70)), None)
            .expect("a box");
        let layer = board.layer(main).expect("the main layer");
        // Drawn from inside the first box to just short of the second.
        let snapped = snap(layer, point(60, 35), point(290, 40));
        assert_eq!(snapped.from.as_ref(), Some(&client.id));
        assert_eq!(snapped.to.as_ref(), Some(&api.id));
        assert_eq!(snapped.start, point(120, 36));
        assert_eq!(snapped.end, point(300, 40));
        // Too far from anything: drawn as it was, connecting nothing.
        let loose = snap(layer, point(60, 300), point(290, 300));
        assert_eq!((loose.from, loose.to), (None, None));
        assert_eq!(loose.start, point(60, 300));
        // Both ends in one box: a scribble, not a connection.
        let inside = snap(layer, point(10, 10), point(100, 60));
        assert_eq!((inside.from, inside.to), (None, None));
    }

    #[test]
    fn the_smallest_shape_under_an_end_wins() {
        let mut board = Board::new();
        let main = LayerId::MAIN;
        let _outer = board.add_user_shape(main, Drawn::Box(rect(0, 0, 400, 300)), None);
        let inner = board
            .add_user_shape(main, Drawn::Box(rect(100, 100, 80, 50)), None)
            .expect("a box");
        let layer = board.layer(main).expect("the main layer");
        let snapped = snap(layer, point(600, 120), point(150, 120));
        assert_eq!(snapped.to.as_ref(), Some(&inner.id));
    }
}
