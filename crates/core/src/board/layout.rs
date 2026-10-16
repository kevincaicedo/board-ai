//! Layout: meaning to geometry on a grid (LLD §2.5).
//!
//! Nodes sit in grid cells; a wide label spans several columns. `near X dir D` starts at the cell
//! next to X in direction D and nudges further along D past occupied cells. Layout only ever
//! places new elements, so nothing already drawn moves (I6); when the board outgrows the view the
//! camera pans instead.

use std::collections::BTreeMap;

use crate::board::metrics::{LABEL_FONT_SIZE, LABEL_LINE_HEIGHT, text_width};
use crate::geometry::{Point, Rect};
use crate::ids::ElementId;
use crate::limits::LAYOUT_NUDGE_CELLS_MAX;
use crate::narration::ops::Direction;
use crate::protocol::{DrawOp, EdgePath, NodeShape};

/// Grid pitch in world units. A cell holds one ordinary node plus the gap around it.
pub const CELL_WIDTH: i32 = 200;
pub const CELL_HEIGHT: i32 = 140;

/// Space kept between neighbouring nodes in a row.
const CELL_GAP: u32 = 40;

/// Room around a label inside its box, per side.
const NODE_PADDING: u32 = 24;

/// No box is narrower than this, so short labels ("lb") still read as boxes.
const NODE_MIN_WIDTH: u32 = 120;

/// Widest node; a longer label spans at most three cells.
const NODE_MAX_WIDTH: u32 = 520;

/// Inset of a group's box from the edge of its cells.
const GROUP_INSET: i32 = 4;

/// Space between a group's boundary and its children.
pub const GROUP_PADDING: i32 = 12;

/// Extra space above a group's children for its handwritten label.
pub const GROUP_LABEL_SPACE: i32 = 22;

/// How far an annotation mark sits outside its target.
pub const ANNOTATION_MARGIN: i32 = 10;

/// How far a blocked edge bows away from the straight line.
const CURVE_LIFT: i32 = CELL_HEIGHT / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub row: i32,
    pub col: i32,
}

/// Cells `[col, col + cols)` of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSpan {
    pub cell: Cell,
    pub cols: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Occupant {
    Node(ElementId),
    /// A group's first cell, kept for its first child.
    GroupReserve(ElementId),
}

/// Which cells are taken, on one layer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    occupied: BTreeMap<Cell, Occupant>,
}

impl Grid {
    /// The first free span at or beyond `start` along `direction`. Members of `group` may use the
    /// cell their group reserved. Past the nudge limit it falls back to a fresh row under
    /// everything, which is always free.
    #[must_use]
    pub fn find(
        &self,
        start: Cell,
        cols: i32,
        direction: Direction,
        group: Option<&ElementId>,
    ) -> Cell {
        let (row_step, col_step) = step(direction);
        for distance in 0..LAYOUT_NUDGE_CELLS_MAX {
            let cell = Cell {
                row: start.row + row_step * distance,
                col: start.col + col_step * distance,
            };
            if self.is_free(cell, cols, group) {
                return cell;
            }
        }
        let below_everything = self.occupied.keys().map(|cell| cell.row).max().unwrap_or(0) + 1;
        Cell {
            row: below_everything.max(start.row),
            col: start.col,
        }
    }

    fn is_free(&self, cell: Cell, cols: i32, group: Option<&ElementId>) -> bool {
        (cell.col..cell.col + cols).all(|col| {
            match self.occupied.get(&Cell { row: cell.row, col }) {
                None => true,
                Some(Occupant::GroupReserve(owner)) => group == Some(owner),
                Some(Occupant::Node(_)) => false,
            }
        })
    }

    pub fn occupy(&mut self, span: CellSpan, id: &ElementId) {
        for col in span.cell.col..span.cell.col + span.cols {
            let cell = Cell {
                row: span.cell.row,
                col,
            };
            self.occupied.insert(cell, Occupant::Node(id.clone()));
        }
    }

    /// A group keeps its first cell free for its first child.
    pub fn reserve(&mut self, cell: Cell, group: &ElementId) {
        self.occupied
            .entry(cell)
            .or_insert_with(|| Occupant::GroupReserve(group.clone()));
    }

    /// Frees the cells of an element that was laid out but never drawn.
    pub fn release(&mut self, id: &ElementId) {
        self.occupied.retain(|_, occupant| match occupant {
            Occupant::Node(owner) | Occupant::GroupReserve(owner) => owner != id,
        });
    }
}

fn step(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Right => (0, 1),
        Direction::Left => (0, -1),
        Direction::Above => (-1, 0),
        Direction::Below => (1, 0),
    }
}

/// The first cell to try next to `target` for a node `cols` wide.
#[must_use]
pub fn start_next_to(target: CellSpan, cols: i32, direction: Direction) -> Cell {
    let Cell { row, col } = target.cell;
    match direction {
        Direction::Right => Cell {
            row,
            col: col + target.cols,
        },
        Direction::Left => Cell {
            row,
            col: col - cols,
        },
        Direction::Above => Cell { row: row - 1, col },
        Direction::Below => Cell { row: row + 1, col },
    }
}

/// Width and height of a node with this shape and label.
#[must_use]
pub fn node_size(shape: NodeShape, label: &str) -> (u32, u32) {
    let label_width = text_width(label, LABEL_FONT_SIZE);
    let boxed =
        |extra: u32, min: u32| (label_width + 2 * NODE_PADDING + extra).clamp(min, NODE_MAX_WIDTH);
    match shape {
        NodeShape::Service => (boxed(0, NODE_MIN_WIDTH), 64),
        // Room for the message dividers on the right.
        NodeShape::Queue => (boxed(32, NODE_MIN_WIDTH + 20), 64),
        NodeShape::Db => (boxed(0, NODE_MIN_WIDTH - 10), 88),
        // A figure with the label under it.
        NodeShape::Actor => ((label_width + 16).clamp(80, NODE_MAX_WIDTH), 96),
        NodeShape::Note => ((label_width + 8).min(NODE_MAX_WIDTH), LABEL_LINE_HEIGHT + 8),
        NodeShape::Group => (
            (CELL_WIDTH - 2 * GROUP_INSET).unsigned_abs(),
            (CELL_HEIGHT - 2 * GROUP_INSET).unsigned_abs(),
        ),
    }
}

/// How many columns a node this wide needs, gap included.
#[must_use]
pub fn columns_for(width: u32) -> i32 {
    let cell = CELL_WIDTH.unsigned_abs();
    i32::try_from((width + CELL_GAP).div_ceil(cell).max(1)).unwrap_or(1)
}

/// A node's box, centred in its cells.
#[must_use]
pub fn rect_in(span: CellSpan, width: u32, height: u32) -> Rect {
    let cells_width = span.cols * CELL_WIDTH;
    let width_i32 = i32::try_from(width).unwrap_or(cells_width);
    let height_i32 = i32::try_from(height).unwrap_or(CELL_HEIGHT);
    Rect {
        x: span.cell.col * CELL_WIDTH + (cells_width - width_i32) / 2,
        y: span.cell.row * CELL_HEIGHT + (CELL_HEIGHT - height_i32) / 2,
        w: width,
        h: height,
    }
}

/// `rect` grown by `margin` on every side, with `top_extra` more above.
#[must_use]
pub fn inflate(rect: Rect, margin: i32, top_extra: i32) -> Rect {
    let grow = u32::try_from(2 * margin).unwrap_or(0);
    Rect {
        x: rect.x - margin,
        y: rect.y - margin - top_extra,
        w: rect.w + grow,
        h: rect.h + grow + u32::try_from(top_extra).unwrap_or(0),
    }
}

/// An edge between two boxes: from the nearest sides, straight unless another node is in the
/// way, then one curve bowing around it.
#[must_use]
pub fn edge_path(from: Rect, to: Rect, obstacles: &[Rect]) -> EdgePath {
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
    let corridor = segment_box(start, end);
    let blocked = obstacles.iter().any(|rect| rect.intersects(corridor));
    let control = blocked.then(|| {
        let middle = Point {
            x: (start.x + end.x) / 2,
            y: (start.y + end.y) / 2,
        };
        if horizontal {
            Point {
                x: middle.x,
                y: middle.y - CURVE_LIFT,
            }
        } else {
            Point {
                x: middle.x + CURVE_LIFT,
                y: middle.y,
            }
        }
    });
    EdgePath {
        start,
        control,
        end,
    }
}

/// The thin box a straight segment sweeps, for the obstacle test.
fn segment_box(start: Point, end: Point) -> Rect {
    let x = start.x.min(end.x);
    let y = start.y.min(end.y);
    Rect {
        x: x - 2,
        y: y - 2,
        w: start.x.abs_diff(end.x) + 4,
        h: start.y.abs_diff(end.y) + 4,
    }
}

/// The area a drawing op covers, for the camera (stacked copies and labels written beside a
/// shape stay within the camera's margin).
#[must_use]
pub fn op_bounds(op: &DrawOp) -> Rect {
    match op {
        DrawOp::Node { rect, .. } | DrawOp::Annotate { rect, .. } | DrawOp::Strike { rect, .. } => {
            *rect
        }
        DrawOp::Edge { path, .. } => path_bounds(*path),
        DrawOp::Trace { points, .. } => points
            .iter()
            .map(|point| segment_box(*point, *point))
            .reduce(Rect::union)
            .unwrap_or_default(),
    }
}

/// The bounding box of an edge, for annotations and the camera.
#[must_use]
pub fn path_bounds(path: EdgePath) -> Rect {
    let mut bounds = segment_box(path.start, path.end);
    if let Some(control) = path.control {
        bounds = bounds.union(segment_box(control, control));
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::{
        CELL_WIDTH, Cell, CellSpan, Grid, columns_for, edge_path, node_size, rect_in, start_next_to,
    };
    use crate::geometry::Rect;
    use crate::ids::ElementId;
    use crate::narration::ops::Direction;
    use crate::protocol::NodeShape;

    fn id(text: &str) -> ElementId {
        ElementId::parse(text).expect("valid id")
    }

    #[test]
    fn near_right_takes_the_next_free_cell_and_collisions_nudge_along() {
        let mut grid = Grid::default();
        let origin = CellSpan {
            cell: Cell { row: 0, col: 0 },
            cols: 1,
        };
        grid.occupy(origin, &id("cdn"));
        let start = start_next_to(origin, 1, Direction::Right);
        assert_eq!(
            grid.find(start, 1, Direction::Right, None),
            Cell { row: 0, col: 1 }
        );
        grid.occupy(
            CellSpan {
                cell: Cell { row: 0, col: 1 },
                cols: 1,
            },
            &id("lb"),
        );
        assert_eq!(
            grid.find(start, 1, Direction::Right, None),
            Cell { row: 0, col: 2 }
        );
        let above = start_next_to(origin, 1, Direction::Above);
        assert_eq!(
            grid.find(above, 1, Direction::Above, None),
            Cell { row: -1, col: 0 }
        );
    }

    #[test]
    fn a_group_reserve_is_free_only_for_its_members() {
        let mut grid = Grid::default();
        let cell = Cell { row: 2, col: 2 };
        grid.reserve(cell, &id("vpc"));
        assert_eq!(grid.find(cell, 1, Direction::Right, Some(&id("vpc"))), cell);
        assert_eq!(
            grid.find(cell, 1, Direction::Right, None),
            Cell { row: 2, col: 3 }
        );
        grid.release(&id("vpc"));
        assert_eq!(grid.find(cell, 1, Direction::Right, None), cell);
    }

    #[test]
    fn wide_labels_span_more_cells_and_boxes_stay_centred() {
        let (short_width, _) = node_size(NodeShape::Service, "lb");
        assert_eq!(columns_for(short_width), 1);
        let (long_width, _) = node_size(NodeShape::Service, "Authentication gateway service");
        assert!(columns_for(long_width) >= 2);
        let span = CellSpan {
            cell: Cell { row: 0, col: 1 },
            cols: 1,
        };
        let rect = rect_in(span, 120, 64);
        assert_eq!(rect.center().x, CELL_WIDTH + CELL_WIDTH / 2);
    }

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
}
