//! Layout: meaning to geometry on a grid (LLD §2.5).
//!
//! Nodes sit in grid cells; a wide label spans several columns. `near X dir D` starts at the cell
//! next to X in direction D and nudges further along D past occupied cells. Layout only ever
//! places new elements, so nothing already drawn moves (I6); when the board outgrows the view the
//! camera pans instead.

use std::collections::BTreeMap;

use crate::board::metrics::{
    INDEX_FONT_SIZE, LABEL_FONT_SIZE, LABEL_LINE_HEIGHT, SMALL_LABEL_FONT_SIZE, text_width,
};
use crate::geometry::{Point, Rect};
use crate::ids::ElementId;
use crate::limits::LAYOUT_NUDGE_CELLS_MAX;
use crate::narration::ops::Direction;
use crate::protocol::{CellBox, DrawOp, EdgePath, Lifeline, NodeShape};

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

/// Smallest circle; a longer label stretches it into an ellipse of this height.
const CIRCLE_DIAMETER: u32 = 56;

/// Room around a circle's label, both sides together.
const CIRCLE_PADDING: u32 = 28;

/// A participant's head box: its height and its narrowest and widest.
pub const HEAD_HEIGHT: u32 = 48;
const HEAD_MIN_WIDTH: u32 = 96;
const HEAD_MAX_WIDTH: u32 = 260;

/// A cell's height, and its narrowest width; a wider value widens every cell of its row.
pub const CELL_BOX_HEIGHT: u32 = 48;
const CELL_BOX_MIN_WIDTH: u32 = 48;
const CELL_VALUE_PADDING: u32 = 20;

/// Where a row's label ends, left of its first cell, and where a column's label sits, above it
/// (the canvas's `LABEL_ABOVE`).
pub const STRIP_LABEL_GAP: i32 = 14;
pub const LABEL_ABOVE: i32 = 21;

/// Index labels: under a row's cells (their middle this far below), or left of a column's.
pub const INDEX_BELOW: i32 = 16;
pub const INDEX_LEFT_GAP: i32 = 10;

/// How far above a message its label sits (the edge label gap), and where a message to itself
/// writes its label, right of its loop.
pub const MESSAGE_LABEL_ABOVE: i32 = 14;
pub const SELF_LABEL_GAP: i32 = 10;

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

    /// The first free block of `cols` × `rows` cells at or beyond `start` along `direction`, or
    /// a fresh row under everything past the nudge limit.
    #[must_use]
    pub fn find_block(&self, start: Cell, cols: i32, rows: i32, direction: Direction) -> Cell {
        let (row_step, col_step) = step(direction);
        for distance in 0..LAYOUT_NUDGE_CELLS_MAX {
            let cell = Cell {
                row: start.row + row_step * distance,
                col: start.col + col_step * distance,
            };
            if (cell.row..cell.row + rows).all(|row| self.is_free(Cell { row, ..cell }, cols, None))
            {
                return cell;
            }
        }
        Cell {
            row: self.below_everything().max(start.row),
            col: start.col,
        }
    }

    /// The leftmost occupied column, or 0 on an empty grid.
    #[must_use]
    pub fn leftmost_col(&self) -> i32 {
        self.occupied.keys().map(|cell| cell.col).min().unwrap_or(0)
    }

    /// The first row under every occupied cell.
    #[must_use]
    pub fn below_everything(&self) -> i32 {
        self.occupied
            .keys()
            .map(|cell| cell.row)
            .max()
            .unwrap_or(-1)
            + 1
    }

    /// Marks the cells `rect` overlaps as taken by `id`, keeping any earlier owner, so grid
    /// placement steers clear of an element placed off the grid.
    pub fn occupy_rect(&mut self, rect: Rect, id: &ElementId) {
        for cell in cells_covering(rect) {
            self.occupied
                .entry(cell)
                .or_insert_with(|| Occupant::Node(id.clone()));
        }
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

/// Every grid cell `rect` overlaps.
fn cells_covering(rect: Rect) -> impl Iterator<Item = Cell> {
    let rows = rect.y.div_euclid(CELL_HEIGHT)..=(rect.bottom() - 1).div_euclid(CELL_HEIGHT);
    let cols = rect.x.div_euclid(CELL_WIDTH)..=(rect.right() - 1).div_euclid(CELL_WIDTH);
    rows.flat_map(move |row| cols.clone().map(move |col| Cell { row, col }))
}

/// How many rows a block this tall needs.
#[must_use]
pub fn rows_for(height: u32) -> i32 {
    let cell = CELL_HEIGHT.unsigned_abs();
    i32::try_from(height.div_ceil(cell).max(1)).unwrap_or(1)
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
        NodeShape::Circle => (
            (label_width + CIRCLE_PADDING).clamp(CIRCLE_DIAMETER, NODE_MAX_WIDTH),
            CIRCLE_DIAMETER,
        ),
    }
}

/// A participant's head box for this label.
#[must_use]
pub fn head_size(label: &str) -> (u32, u32) {
    let width = text_width(label, LABEL_FONT_SIZE) + 2 * NODE_PADDING;
    (width.clamp(HEAD_MIN_WIDTH, HEAD_MAX_WIDTH), HEAD_HEIGHT)
}

/// One cell's width for these values: every cell of a row or column is as wide as the widest.
#[must_use]
pub fn cell_width(values: &[String]) -> u32 {
    values
        .iter()
        .map(|value| text_width(value, LABEL_FONT_SIZE) + CELL_VALUE_PADDING)
        .max()
        .unwrap_or(0)
        .clamp(CELL_BOX_MIN_WIDTH, NODE_MAX_WIDTH)
}

/// A label's box when written centred on `center` at `font_size`.
#[must_use]
pub fn text_box(text: &str, center: Point, font_size: u32) -> Rect {
    let width = text_width(text, font_size);
    Rect {
        x: center.x - i32::try_from(width / 2).unwrap_or(0),
        y: center.y - i32::try_from(LABEL_LINE_HEIGHT / 2).unwrap_or(0),
        w: width,
        h: LABEL_LINE_HEIGHT,
    }
}

/// What a row or column of cells covers, its label and index labels included, for the camera
/// and for keeping other drawings clear of it. The UI writes them where this measures them.
#[must_use]
pub fn cells_extent(cells: &[CellBox], label: &str, vertical: bool) -> Rect {
    let Some(strip) = cells.iter().map(|cell| cell.rect).reduce(Rect::union) else {
        return Rect::default();
    };
    let mut extent = strip;
    for cell in cells {
        let Some(index) = &cell.index else {
            continue;
        };
        let width = i32::try_from(text_width(index, INDEX_FONT_SIZE)).unwrap_or(0);
        let center = if vertical {
            Point {
                x: cell.rect.x - INDEX_LEFT_GAP - width / 2,
                y: cell.rect.center().y,
            }
        } else {
            Point {
                x: cell.rect.center().x,
                y: cell.rect.bottom() + INDEX_BELOW,
            }
        };
        extent = extent.union(text_box(index, center, INDEX_FONT_SIZE));
    }
    if !label.is_empty() {
        let width = i32::try_from(text_width(label, SMALL_LABEL_FONT_SIZE)).unwrap_or(0);
        let center = if vertical {
            Point {
                x: strip.center().x,
                y: strip.y - LABEL_ABOVE,
            }
        } else {
            Point {
                x: strip.x - STRIP_LABEL_GAP - width / 2,
                y: strip.center().y,
            }
        };
        extent = extent.union(text_box(label, center, SMALL_LABEL_FONT_SIZE));
    }
    extent
}

/// Where a message's label is written: centred above a message between two lifelines, or right
/// of the loop of a message to itself.
#[must_use]
pub fn message_label_box(points: &[Point], label: &str) -> Option<Rect> {
    let (first, last) = (points.first()?, points.last()?);
    let center = if points.len() > 2 {
        let right = points.iter().map(|point| point.x).max().unwrap_or(first.x);
        let width = i32::try_from(text_width(label, SMALL_LABEL_FONT_SIZE)).unwrap_or(0);
        Point {
            x: right + SELF_LABEL_GAP + width / 2,
            y: (first.y + last.y) / 2,
        }
    } else {
        Point {
            x: (first.x + last.x) / 2,
            y: first.y - MESSAGE_LABEL_ABOVE,
        }
    };
    Some(text_box(label, center, SMALL_LABEL_FONT_SIZE))
}

/// The thin box of a stretch of lifeline.
#[must_use]
pub fn lifeline_box(x: i32, top: i32, bottom: i32) -> Rect {
    segment_box(Point { x, y: top }, Point { x, y: bottom })
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

/// A box of `width` × `height` centred in a block of `cols` × `rows` cells.
#[must_use]
pub fn rect_in_block(cell: Cell, cols: i32, rows: i32, width: u32, height: u32) -> Rect {
    let block_width = cols * CELL_WIDTH;
    let block_height = rows * CELL_HEIGHT;
    let width_i32 = i32::try_from(width).unwrap_or(block_width);
    let height_i32 = i32::try_from(height).unwrap_or(block_height);
    Rect {
        x: cell.col * CELL_WIDTH + (block_width - width_i32) / 2,
        y: cell.row * CELL_HEIGHT + (block_height - height_i32) / 2,
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

/// The thin box a straight segment sweeps, for the obstacle test.
pub(super) fn segment_box(start: Point, end: Point) -> Rect {
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
        DrawOp::Node { rect, link, .. } => link.map_or(*rect, |link| rect.union(path_bounds(link))),
        DrawOp::Annotate { rect, .. }
        | DrawOp::Strike { rect, .. }
        | DrawOp::Write { rect, .. } => *rect,
        DrawOp::Edge { path, .. } => path_bounds(*path),
        DrawOp::Trace { points, .. } => points_bounds(points),
        DrawOp::Participant {
            rect,
            lifeline_bottom,
            ..
        } => rect.union(lifeline_box(
            rect.center().x,
            rect.bottom(),
            *lifeline_bottom,
        )),
        DrawOp::Message {
            points,
            label,
            extend,
            ..
        } => {
            let mut bounds = points_bounds(points);
            if let Some(label) = label
                .as_deref()
                .and_then(|label| message_label_box(points, label))
            {
                bounds = bounds.union(label);
            }
            extend
                .iter()
                .map(|Lifeline { x, top, bottom, .. }| lifeline_box(*x, *top, *bottom))
                .fold(bounds, Rect::union)
        }
        DrawOp::Cells {
            cells,
            label,
            vertical,
            ..
        } => cells_extent(cells, label, *vertical),
    }
}

/// The bounding box of a polyline.
#[must_use]
pub fn points_bounds(points: &[Point]) -> Rect {
    points
        .iter()
        .map(|point| segment_box(*point, *point))
        .reduce(Rect::union)
        .unwrap_or_default()
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
    use super::{CELL_WIDTH, Cell, CellSpan, Grid, columns_for, node_size, rect_in, start_next_to};
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
}
