//! Edge routing (LLD §2.5): an edge joins the nearest sides of its two boxes, straight unless a
//! node is in its way, then with one quadratic curve that clears every node.

use super::layout::{CELL_HEIGHT, inflate, segment_box};
use crate::geometry::{Point, Rect};
use crate::protocol::EdgePath;

/// How far a blocked edge's control point first moves off the straight line; each further try
/// moves it one more step (a quadratic reaches half its control's offset at the middle, and
/// less towards its ends).
const CURVE_LIFT: i32 = CELL_HEIGHT / 2;

/// Curves a blocked edge tries, bowing up then down (right then left for a vertical edge), one
/// `CURVE_LIFT` further each pair. When none clears, it keeps the first: crossing a node beats
/// looping far around the board.
const CURVE_TRIES_MAX: i32 = 6;

/// Segments a curve is checked along against the nodes in its way.
const CURVE_SEGMENTS: i64 = 16;

/// Room a curve keeps from a node it bows around, for the hand-drawn wobble of both strokes.
const CURVE_CLEARANCE: i32 = 6;

/// An edge between two boxes: from the nearest sides, straight unless another node is in the
/// way, then one curve bowing around every node, the nearest bow that clears them.
#[must_use]
pub fn edge_path(from: Rect, to: Rect, obstacles: &[Rect]) -> EdgePath {
    route(from, to, obstacles, CURVE_LIFT, Fallback::FirstBow)
}

/// A staged diagram's connector (D-187): `edge_path` bowing `lift` units further each try (its
/// boxes are smaller and closer than a sketch's), and when nothing clears every box, the line or
/// bow that crosses the fewest: a map's long connectors cross a section or two, and the first bow
/// alone ran through the boxes in their way.
#[must_use]
pub fn staged_edge_path(from: Rect, to: Rect, obstacles: &[Rect], lift: i32) -> EdgePath {
    route(from, to, obstacles, lift, Fallback::FewestCrossings)
}

/// What a blocked edge does when no bow clears everything.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fallback {
    FirstBow,
    FewestCrossings,
}

/// Where an edge between two boxes leaves and arrives: the nearest sides, and whether it runs
/// more across than down.
fn ends(from: Rect, to: Rect) -> (Point, Point, bool) {
    let (a, b) = (from.center(), to.center());
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let horizontal = dx.abs() >= dy.abs();
    let (start, end) = if horizontal {
        let (start_x, end_x) = if dx >= 0 {
            (from.right(), to.x)
        } else {
            (from.x, to.right())
        };
        (Point { x: start_x, y: a.y }, Point { x: end_x, y: b.y })
    } else {
        let (start_y, end_y) = if dy >= 0 {
            (from.bottom(), to.y)
        } else {
            (from.y, to.bottom())
        };
        (Point { x: a.x, y: start_y }, Point { x: b.x, y: end_y })
    };
    (start, end, horizontal)
}

fn route(from: Rect, to: Rect, obstacles: &[Rect], lift: i32, fallback: Fallback) -> EdgePath {
    let (start, end, horizontal) = ends(from, to);
    let corridor = segment_box(start, end);
    let blocked = obstacles.iter().any(|rect| rect.intersects(corridor));
    let middle = Point {
        x: (start.x + end.x) / 2,
        y: (start.y + end.y) / 2,
    };
    let bowed = |lift: i32| {
        if horizontal {
            Point {
                x: middle.x,
                y: middle.y - lift,
            }
        } else {
            Point {
                x: middle.x + lift,
                y: middle.y,
            }
        }
    };
    let crossings = |control: Point| {
        obstacles
            .iter()
            .filter(|rect| {
                let kept = inflate(**rect, CURVE_CLEARANCE, 0);
                (0..CURVE_SEGMENTS).any(|segment| {
                    let from = quadratic_at(start, control, end, segment);
                    let to = quadratic_at(start, control, end, segment + 1);
                    segment_crosses(from, to, kept)
                })
            })
            .count()
    };
    let bows = || {
        (0..CURVE_TRIES_MAX).map(|attempt| {
            let side = if attempt % 2 == 0 { 1 } else { -1 };
            bowed(side * (attempt / 2 + 1) * lift)
        })
    };
    let control = if !blocked {
        None
    } else if let Some(clear) = bows().find(|control| crossings(*control) == 0) {
        Some(clear)
    } else {
        match fallback {
            Fallback::FirstBow => Some(bowed(lift)),
            // The line itself is a candidate: its control point is its middle.
            Fallback::FewestCrossings => {
                let best = bows().min_by_key(|control| crossings(*control));
                best.filter(|control| crossings(*control) < crossings(middle))
            }
        }
    };
    EdgePath {
        start,
        control,
        end,
    }
}

/// True when the segment from `a` to `b` enters `rect`'s interior: their boxes overlap and the
/// rectangle's corners are not all on one side of the segment's line. (The segment's box alone
/// is far too wide for a steep segment beside a corner.)
fn segment_crosses(a: Point, b: Point, rect: Rect) -> bool {
    let span = Rect {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        w: a.x.abs_diff(b.x),
        h: a.y.abs_diff(b.y),
    };
    let overlap = span.x < rect.right()
        && rect.x < span.right().max(span.x + 1)
        && span.y < rect.bottom()
        && rect.y < span.bottom().max(span.y + 1);
    if !overlap {
        return false;
    }
    let side = |x: i32, y: i32| {
        i64::from(b.x - a.x) * i64::from(y - a.y) - i64::from(b.y - a.y) * i64::from(x - a.x)
    };
    let corners = [
        side(rect.x, rect.y),
        side(rect.right(), rect.y),
        side(rect.x, rect.bottom()),
        side(rect.right(), rect.bottom()),
    ];
    !(corners.iter().all(|side| *side > 0) || corners.iter().all(|side| *side < 0))
}

/// The point `segment / CURVE_SEGMENTS` of the way along a quadratic curve, in whole units.
fn quadratic_at(start: Point, control: Point, end: Point, segment: i64) -> Point {
    let (before, after) = (CURVE_SEGMENTS - segment, segment);
    let along = |a: i32, b: i32, c: i32| {
        let sum = before * before * i64::from(a)
            + 2 * before * after * i64::from(b)
            + after * after * i64::from(c);
        i32::try_from(sum / (CURVE_SEGMENTS * CURVE_SEGMENTS)).unwrap_or(a)
    };
    Point {
        x: along(start.x, control.x, end.x),
        y: along(start.y, control.y, end.y),
    }
}

#[cfg(test)]
mod tests {
    use super::{CURVE_LIFT, CURVE_SEGMENTS, edge_path, quadratic_at, segment_crosses};
    use crate::geometry::{Point, Rect};
    use crate::protocol::EdgePath;

    #[test]
    fn edges_join_nearest_sides_and_bow_around_obstacles() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 60,
        };
        let b = Rect {
            x: 400,
            y: 0,
            w: 100,
            h: 60,
        };
        let straight = edge_path(a, b, &[]);
        assert_eq!((straight.start.x, straight.end.x), (100, 400));
        assert_eq!(straight.control, None);
        let between = Rect {
            x: 200,
            y: 10,
            w: 100,
            h: 40,
        };
        assert!(edge_path(a, b, &[between]).control.is_some());
        let below = Rect {
            x: 0,
            y: 300,
            w: 100,
            h: 60,
        };
        let down = edge_path(a, below, &[]);
        assert_eq!((down.start.y, down.end.y), (60, 300));
    }

    /// Every segment of the edge's curve stays out of `rect`.
    fn clear_of(path: EdgePath, rect: Rect) -> bool {
        let control = path.control.unwrap_or(path.start);
        (0..CURVE_SEGMENTS).all(|segment| {
            let from = quadratic_at(path.start, control, path.end, segment);
            let to = quadratic_at(path.start, control, path.end, segment + 1);
            !segment_crosses(from, to, rect)
        })
    }

    fn boxed(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// A live hash chain ran through the corner of the box between its ends: one fixed bow
    /// lifts least near the ends, where that box was.
    #[test]
    fn a_curve_clears_a_node_beside_its_end() {
        let (from, to) = (boxed(198, 60, 120, 48), boxed(662, 60, 120, 48));
        let between = boxed(382, 60, 232, 48);
        let path = edge_path(from, to, &[between]);
        let control = path.control.expect("bowed");
        assert!(control.y < path.start.y, "up first");
        assert!(clear_of(path, between), "{path:?}");
    }

    /// The first bow passes about 3 units over this box's corner: too close for a hand-drawn
    /// line, so it bows further (LLD §2.5: 6 units of room).
    #[test]
    fn a_curve_keeps_room_from_a_node_it_bows_around() {
        let (from, to) = (boxed(0, 0, 100, 60), boxed(400, 0, 100, 60));
        let between = boxed(200, 11, 140, 39);
        let path = edge_path(from, to, &[between]);
        let room = Rect {
            x: between.x - 6,
            y: between.y - 6,
            w: between.w + 12,
            h: between.h + 12,
        };
        assert!(
            clear_of(path, room),
            "{path:?} within 6 units of {between:?}"
        );
    }

    #[test]
    fn a_curve_bows_down_when_up_is_taken_and_keeps_the_first_bow_when_nothing_clears() {
        let (from, to) = (boxed(0, 0, 100, 60), boxed(400, 0, 100, 60));
        let between = boxed(200, 10, 100, 40);
        let above = boxed(120, -400, 260, 398);
        let path = edge_path(from, to, &[between, above]);
        let control = path.control.expect("bowed");
        assert!(
            control.y > path.start.y,
            "down, clear of {above:?}: {path:?}"
        );
        assert!(clear_of(path, between) && clear_of(path, above));
        let everywhere = boxed(120, -2000, 260, 4000);
        let first = edge_path(from, to, &[everywhere]);
        assert_eq!(
            first.control,
            Some(Point {
                x: 250,
                y: 30 - CURVE_LIFT
            })
        );
    }

    #[test]
    fn a_segment_crosses_a_box_only_through_its_inside() {
        let rect = boxed(100, 100, 100, 50);
        let crosses = |(ax, ay): (i32, i32), (bx, by): (i32, i32)| {
            segment_crosses(Point { x: ax, y: ay }, Point { x: bx, y: by }, rect)
        };
        assert!(crosses((0, 120), (300, 120)), "straight through");
        assert!(crosses((150, 0), (150, 300)), "down through");
        assert!(crosses((90, 90), (110, 110)), "into a corner");
        assert!(
            !crosses((185, 80), (225, 120)),
            "past a corner, its box overlapping"
        );
        assert!(!crosses((0, 160), (300, 160)), "under it");
        assert!(!crosses((90, 0), (90, 300)), "beside it");
    }
}
