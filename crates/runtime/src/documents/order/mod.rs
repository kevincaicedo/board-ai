//! A PDF page's reading order, rebuilt from where PDFKit laid out its lines (D-293).
//!
//! The order of PDFKit's own text differs between macOS versions: macOS 14 reads a table column
//! by column where macOS 27 reads it row by row. So `pdfkit.js` sends every line with its box,
//! and the order is decided here from the boxes alone, the same on every macOS:
//!
//! - **Rows.** Boxes whose bottoms lie within `PDF_ROW_BASELINE_TOLERANCE_POINTS` share a
//!   baseline. Inside one column they are one row, read left to right: a table's cells, a
//!   figure's labels.
//! - **Columns.** A gutter is the gap, at least `PDF_GUTTER_MIN_POINTS` wide, between two prose
//!   lines (`PDF_PROSE_LINE_MIN_CHARS`) side by side, or between prose and a box wholly outside
//!   the page's prose (a margin note); overlapping gaps are one gutter. A table's cells are short
//!   and inside the prose, so a table never makes one, and a one-column page has none.
//! - **Bands.** A row with a box across a gutter (a centred title, a wide table PDFKit joined
//!   into rows) is read whole, and the rows between two of them are a band. A band's column zone
//!   runs from its first side-by-side prose to its last, and its rows are read a column at a
//!   time, each column top to bottom: a two-column paper never interleaves. A row with no prose
//!   and a box in a gutter, or with boxes in several columns next to another such row, is full
//!   width: above the zone (a wide table whose cells miss the gutter) it is read in place, below
//!   it (a page number, a table at the foot) after the columns.
//!
//! Sorting the page by height alone would interleave every two-column paper's columns. A page
//! with more than `PDF_PAGE_BOXES_MAX` boxes, or whose text runs up the page (a table turned on
//! its side, which rows by height would scramble), keeps PDFKit's order.

use std::cmp::Ordering;

use super::pdfkit::PdfLine;
use crate::limits::{
    PDF_COORDINATE_MAX_POINTS, PDF_GUTTER_MIN_POINTS, PDF_PAGE_BOXES_MAX, PDF_PAGE_COLUMNS_MAX,
    PDF_PROSE_LINE_MIN_CHARS, PDF_ROW_BASELINE_TOLERANCE_POINTS,
};

#[cfg(test)]
mod tests;

/// Where PDFKit laid out a line: its box's origin and size in PDF points (y grows upward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A line of a page with text, and its box: finite, within `PDF_COORDINATE_MAX_POINTS`, never
/// inverted.
#[derive(Clone, Debug, PartialEq)]
pub struct LineBox {
    line: PdfLine,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

impl LineBox {
    /// A line at its box, its text trimmed and kept on one line; `None` when it is blank.
    #[must_use]
    pub fn new(line: PdfLine, bounds: Bounds) -> Option<Self> {
        let text = line.text.trim().replace(['\n', '\r'], " ");
        if text.is_empty() {
            return None;
        }
        let left = coordinate(bounds.x);
        let bottom = coordinate(bounds.y);
        // A coordinate past the bound is read at it, so the far edge never falls short of the
        // near one.
        let right = coordinate(bounds.x + bounds.width.max(0.0)).max(left);
        let top = coordinate(bounds.y + bounds.height.max(0.0)).max(bottom);
        Some(Self {
            line: PdfLine { text, ..line },
            left,
            right,
            bottom,
            top,
        })
    }

    fn height(&self) -> f32 {
        self.top - self.bottom
    }

    fn is_prose(&self) -> bool {
        self.line.text.chars().count() >= PDF_PROSE_LINE_MIN_CHARS
    }
}

fn coordinate(value: f64) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped to a page's extent first"
    )]
    let point = value.clamp(-PDF_COORDINATE_MAX_POINTS, PDF_COORDINATE_MAX_POINTS) as f32;
    point
}

/// A page's lines in reading order: rows within columns, columns within bands. The order is a
/// function of the boxes alone, whatever order PDFKit gave them in.
#[must_use]
pub fn reading_order(mut boxes: Vec<LineBox>) -> Vec<PdfLine> {
    if boxes.len() > PDF_PAGE_BOXES_MAX || runs_vertically(&boxes) {
        return boxes.into_iter().map(|line_box| line_box.line).collect();
    }
    boxes.sort_by(canonical);
    let page = Page::new(boxes);
    let mut lines = Vec::new();
    let mut band: Vec<&Row> = Vec::new();
    for row in &page.rows {
        if page.spans(row) {
            page.read_band(&band, &mut lines);
            band.clear();
            lines.push(page.line(&row.members));
        } else {
            band.push(row);
        }
    }
    page.read_band(&band, &mut lines);
    lines
}

/// Top to bottom by the box's bottom, then left to right; the rest only breaks ties, so the
/// order is total and the result never depends on PDFKit's order.
fn canonical(a: &LineBox, b: &LineBox) -> Ordering {
    b.bottom
        .total_cmp(&a.bottom)
        .then(a.left.total_cmp(&b.left))
        .then(a.right.total_cmp(&b.right))
        .then(b.top.total_cmp(&a.top))
        .then_with(|| a.line.text.cmp(&b.line.text))
        .then(a.line.size.total_cmp(&b.line.size))
        .then(a.line.bold.cmp(&b.line.bold))
}

/// Most of the page's text sits in boxes taller than they are wide: its lines run up or down
/// the page.
fn runs_vertically(boxes: &[LineBox]) -> bool {
    let mut vertical = 0_usize;
    let mut horizontal = 0_usize;
    for line_box in boxes {
        let characters = line_box.line.text.chars().count();
        // A single character's box is taller than wide in any direction.
        if characters < 2 {
            continue;
        }
        if line_box.height() > line_box.right - line_box.left {
            vertical += characters;
        } else {
            horizontal += characters;
        }
    }
    vertical > horizontal
}

/// Boxes that share a baseline, left to right.
struct Row {
    /// Indexes into the page's boxes.
    members: Vec<usize>,
    /// The highest bottom among them: the row's baseline.
    anchor: f32,
    bottom: f32,
    top: f32,
}

/// An empty band between columns, `[begin, end)` across the page.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Gutter {
    begin: f32,
    end: f32,
}

impl Gutter {
    fn middle(self) -> f32 {
        (self.begin + self.end) / 2.0
    }
}

/// Where a box sits among the page's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// Inside the column with this index, 0 the leftmost.
    Column(usize),
    /// Inside a gutter (a page number between the columns); the column is its centre's.
    Gutter(usize),
    /// Across a gutter.
    Across,
}

/// What a row of a band holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Prose, or boxes in one column only.
    Column,
    /// No prose, and boxes in more than one column.
    Split,
    /// No prose, and a box in a gutter.
    Gutter,
}

/// Where a row of a band sits against the band's column zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    Above,
    Inside,
    Below,
}

struct Page {
    /// In canonical order.
    boxes: Vec<LineBox>,
    rows: Vec<Row>,
    gutters: Vec<Gutter>,
    places: Vec<Place>,
    /// Prose with prose beside it across a gutter: the column zone's evidence.
    paired: Vec<bool>,
}

impl Page {
    fn new(boxes: Vec<LineBox>) -> Self {
        let rows = rows(&boxes);
        let (gutters, paired) = gutters(&boxes);
        let places = boxes
            .iter()
            .map(|line_box| place(line_box, &gutters))
            .collect();
        Self {
            boxes,
            rows,
            gutters,
            places,
            paired,
        }
    }

    fn spans(&self, row: &Row) -> bool {
        row.members
            .iter()
            .any(|index| self.places[*index] == Place::Across)
    }

    /// What a row of a band holds, for telling a full-width row from a row of the columns.
    fn shape(&self, row: &Row) -> Shape {
        let mut first: Option<usize> = None;
        let mut several = false;
        let mut gutter = false;
        for index in &row.members {
            if self.boxes[*index].is_prose() {
                return Shape::Column;
            }
            match self.places[*index] {
                Place::Column(column) => {
                    several |= first.is_some_and(|first| first != column);
                    first.get_or_insert(column);
                }
                Place::Gutter(_) | Place::Across => gutter = true,
            }
        }
        if gutter {
            Shape::Gutter
        } else if several {
            Shape::Split
        } else {
            Shape::Column
        }
    }

    fn column(&self, index: usize) -> usize {
        match self.places[index] {
            Place::Column(column) | Place::Gutter(column) => column,
            Place::Across => {
                debug_assert!(
                    false,
                    "a row across a gutter is read whole, never by column"
                );
                0
            }
        }
    }

    /// The highest and lowest edges of the band's paired prose.
    fn zone(&self, band: &[&Row]) -> Option<(f32, f32)> {
        band.iter()
            .flat_map(|row| row.members.iter())
            .filter(|index| self.paired[**index])
            .map(|index| &self.boxes[*index])
            .fold(None, |zone, line_box| {
                Some(
                    zone.map_or((line_box.bottom, line_box.top), |(bottom, top)| {
                        (line_box.bottom.min(bottom), line_box.top.max(top))
                    }),
                )
            })
    }

    /// A band's rows: full-width rows above its zone in place, the rest a column at a time, then
    /// full-width rows below the zone. A row is full width when it holds no prose and has a box
    /// in a gutter (a page number), or boxes in several columns next to another such row (a
    /// table has a header and a row). A lone split row is the columns' own: a heading at the top
    /// of one column beside a paragraph's last words at the top of the other.
    fn read_band(&self, band: &[&Row], lines: &mut Vec<PdfLine>) {
        let zone = self.zone(band);
        let shapes: Vec<Shape> = band.iter().map(|row| self.shape(row)).collect();
        let split =
            |index: Option<usize>| index.and_then(|index| shapes.get(index)) == Some(&Shape::Split);
        let mut run: Vec<&Row> = Vec::new();
        let mut after: Vec<&Row> = Vec::new();
        for (index, &row) in band.iter().enumerate() {
            let full_width = match shapes[index] {
                Shape::Gutter => true,
                Shape::Split => split(index.checked_sub(1)) || split(index.checked_add(1)),
                Shape::Column => false,
            };
            match (full_width, position(row, zone)) {
                (true, Position::Above) => {
                    self.read_columns(&run, lines);
                    run.clear();
                    lines.push(self.line(&row.members));
                }
                (true, Position::Below) => after.push(row),
                (true, Position::Inside)
                | (false, Position::Above | Position::Inside | Position::Below) => run.push(row),
            }
        }
        self.read_columns(&run, lines);
        for row in after {
            lines.push(self.line(&row.members));
        }
    }

    /// Each column top to bottom, left column first; a row's boxes in one column are one line.
    fn read_columns(&self, run: &[&Row], lines: &mut Vec<PdfLine>) {
        for column in 0..=self.gutters.len() {
            for row in run {
                let members: Vec<usize> = row
                    .members
                    .iter()
                    .copied()
                    .filter(|index| self.column(*index) == column)
                    .collect();
                if !members.is_empty() {
                    lines.push(self.line(&members));
                }
            }
        }
    }

    /// Boxes, left to right, as one line: their texts joined by a space (as PDFKit joins a
    /// row's cells), in the font of the first.
    fn line(&self, members: &[usize]) -> PdfLine {
        let mut text = String::new();
        for index in members {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&self.boxes[*index].line.text);
        }
        let first = members.first().map(|index| &self.boxes[*index].line);
        PdfLine {
            text,
            size: first.map_or(0.0, |line| line.size),
            bold: first.is_some_and(|line| line.bold),
        }
    }
}

fn position(row: &Row, zone: Option<(f32, f32)>) -> Position {
    let Some((bottom, top)) = zone else {
        // No side-by-side prose in the band: nothing to keep in columns.
        return Position::Above;
    };
    if row.bottom >= top {
        Position::Above
    } else if row.top <= bottom {
        Position::Below
    } else {
        Position::Inside
    }
}

/// Boxes in canonical order grouped by baseline: a box joins the row above when its bottom is
/// within the tolerance of the row's first.
fn rows(boxes: &[LineBox]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, line_box) in boxes.iter().enumerate() {
        match rows.last_mut() {
            Some(row) if row.anchor - line_box.bottom <= PDF_ROW_BASELINE_TOLERANCE_POINTS => {
                row.members.push(index);
                row.bottom = row.bottom.min(line_box.bottom);
                row.top = row.top.max(line_box.top);
            }
            Some(_) | None => rows.push(Row {
                members: vec![index],
                anchor: line_box.bottom,
                bottom: line_box.bottom,
                top: line_box.top,
            }),
        }
    }
    for row in &mut rows {
        row.members
            .sort_by(|a, b| boxes[*a].left.total_cmp(&boxes[*b].left).then(a.cmp(b)));
    }
    rows
}

fn place(line_box: &LineBox, gutters: &[Gutter]) -> Place {
    if gutters
        .iter()
        .any(|gutter| line_box.left < gutter.begin && line_box.right > gutter.end)
    {
        return Place::Across;
    }
    let centre = (line_box.left + line_box.right) / 2.0;
    let column = gutters
        .iter()
        .filter(|gutter| gutter.middle() < centre)
        .count();
    let inside = gutters
        .iter()
        .any(|gutter| line_box.left >= gutter.begin && line_box.right <= gutter.end);
    if inside {
        Place::Gutter(column)
    } else {
        Place::Column(column)
    }
}

/// A gap between a line and the line beside it on its right: evidence of a gutter.
struct Gap {
    gutter: Gutter,
    left: usize,
    right: usize,
}

/// The page's gutters, left to right (at most `PDF_PAGE_COLUMNS_MAX - 1`, those seen beside
/// the most lines), and which boxes sit beside one. A gap is a gutter's evidence when prose is
/// beside prose, or beside a box wholly outside the page's prose (a margin note, a line
/// number): a table's cells sit inside it, so a long cell beside a short one makes none.
fn gutters(boxes: &[LineBox]) -> (Vec<Gutter>, Vec<bool>) {
    let tallest = boxes.iter().map(LineBox::height).fold(0.0_f32, f32::max);
    let (prose_left, prose_right) = boxes.iter().filter(|line_box| line_box.is_prose()).fold(
        (f32::INFINITY, f32::NEG_INFINITY),
        |(left, right), line_box| (left.min(line_box.left), right.max(line_box.right)),
    );
    let mut gaps: Vec<Gap> = Vec::new();
    for (index, line_box) in boxes.iter().enumerate() {
        let Some(next) = right_neighbour(boxes, tallest, index) else {
            continue;
        };
        let beside = &boxes[next];
        let evidence = match (line_box.is_prose(), beside.is_prose()) {
            (true, true) => true,
            (true, false) => beside.left >= prose_right,
            (false, true) => line_box.right <= prose_left,
            (false, false) => false,
        };
        if evidence && beside.left - line_box.right >= PDF_GUTTER_MIN_POINTS {
            gaps.push(Gap {
                gutter: Gutter {
                    begin: line_box.right,
                    end: beside.left,
                },
                left: index,
                right: next,
            });
        }
    }
    let mut clusters = clusters(gaps);
    clusters.sort_by(|a, b| {
        b.1.len()
            .cmp(&a.1.len())
            .then(a.0.begin.total_cmp(&b.0.begin))
    });
    clusters.truncate(PDF_PAGE_COLUMNS_MAX.saturating_sub(1));
    clusters.sort_by(|a, b| a.0.begin.total_cmp(&b.0.begin));
    let mut paired = vec![false; boxes.len()];
    for (left, right) in clusters.iter().flat_map(|(_, pairs)| pairs) {
        paired[*left] = true;
        paired[*right] = true;
    }
    (
        clusters.into_iter().map(|(gutter, _)| gutter).collect(),
        paired,
    )
}

/// Gaps that overlap, as one gutter: what all of them leave empty, and the pairs beside it.
fn clusters(mut gaps: Vec<Gap>) -> Vec<(Gutter, Vec<(usize, usize)>)> {
    gaps.sort_by(|a, b| {
        a.gutter
            .begin
            .total_cmp(&b.gutter.begin)
            .then(a.gutter.end.total_cmp(&b.gutter.end))
            .then(a.left.cmp(&b.left))
    });
    let mut clusters: Vec<(Gutter, Vec<(usize, usize)>)> = Vec::new();
    for gap in gaps {
        match clusters.last_mut() {
            Some((gutter, pairs)) if gap.gutter.begin < gutter.end => {
                gutter.begin = gutter.begin.max(gap.gutter.begin);
                gutter.end = gutter.end.min(gap.gutter.end);
                pairs.push((gap.left, gap.right));
            }
            Some(_) | None => clusters.push((gap.gutter, vec![(gap.left, gap.right)])),
        }
    }
    clusters
}

/// The nearest box wholly to the right of `index` at the same height (their boxes overlap
/// vertically). Boxes are in canonical order, so only those whose bottoms could overlap it are
/// looked at: below its top, and above its bottom less the tallest box.
fn right_neighbour(boxes: &[LineBox], tallest: f32, index: usize) -> Option<usize> {
    let line_box = &boxes[index];
    let first = boxes.partition_point(|other| other.bottom >= line_box.top);
    let last = boxes.partition_point(|other| other.bottom > line_box.bottom - tallest);
    (first..last.max(first))
        .filter(|other| *other != index)
        .filter(|other| {
            let other = &boxes[*other];
            other.left >= line_box.right
                && other.top.min(line_box.top) > other.bottom.max(line_box.bottom)
        })
        .min_by(|a, b| boxes[*a].left.total_cmp(&boxes[*b].left).then(a.cmp(b)))
}
