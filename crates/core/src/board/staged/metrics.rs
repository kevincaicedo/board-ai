//! The sizes a staged diagram is laid out in (canvas 15): smaller than a sketch's, so a map of a
//! hundred parts reads at natural size. The UI writes with the same numbers (the
//! `--diagram-*` tokens in `ui/src/styles/tokens.css`); the core measures with them, so every
//! label fits the box laid out for it and nothing drawn later lands on it.
//!
//! Text is placed by its centre line, as the UI writes it (`dominant-baseline: central`).

use crate::board::layout::path_bounds;
use crate::board::metrics::text_width;
use crate::geometry::{Point, Rect};
use crate::protocol::{CellBox, EdgePath, StagedPart};

/// A container's label, written inside its top-left corner (`--diagram-frame-label`).
pub const FRAME_LABEL_FONT_SIZE: u32 = 20;
/// Notes, a container's note, cells' names and index labels (`--diagram-note`).
pub const NOTE_FONT_SIZE: u32 = 13;
/// A box's label and a record's name (`--diagram-label`).
pub const LABEL_FONT_SIZE: u32 = 14;
/// A field's or a cell's value, in the code font (`--diagram-field`).
pub const FIELD_FONT_SIZE: u32 = 11;
/// An edge's label (`--diagram-edge`).
pub const EDGE_LABEL_FONT_SIZE: u32 = 12;
/// A legend entry's name (`--diagram-legend`).
pub const LEGEND_FONT_SIZE: u32 = 14;

/// Line heights: a note's lines, and one line of any other diagram text.
pub const NOTE_LINE_HEIGHT: u32 = 18;
pub const TEXT_LINE_HEIGHT: u32 = 20;

/// A container's padding on the sides and at the bottom, and where its label sits.
pub const FRAME_PADDING: i32 = 18;
/// The label's centre line below the frame's top; a note on its own line sits one line lower.
pub const FRAME_LABEL_CENTER: i32 = 24;
pub const FRAME_NOTE_LINE_CENTER: i32 = 46;
/// Space between the label and a note written after it on the same line.
pub const FRAME_NOTE_GAP: i32 = 12;
/// Where a container's content starts below its top: under a one-line or two-line header.
pub const FRAME_HEADER_ONE_LINE: i32 = 50;
pub const FRAME_HEADER_TWO_LINES: i32 = 68;

/// Vertical space between stacked items, and the space between boxes side by side (room for an
/// arrow and its head).
pub const STACK_GAP: i32 = 14;
pub const INLINE_GAP: i32 = 40;
/// Space between containers side by side (canvas 15's sections).
pub const SECTION_GAP: i32 = 16;

/// A box: padding beside its label, its height, the height a note under the label adds, and
/// its narrowest and widest.
pub const NODE_PADDING_X: u32 = 12;
pub const NODE_HEIGHT: u32 = 32;
pub const NODE_NOTE_HEIGHT: u32 = 20;
pub const NODE_MIN_WIDTH: u32 = 44;
/// A box with a note: its label's centre line and the note's, below the box's top. Without a
/// note the label is centred in the box.
pub const NODE_LABEL_CENTER_WITH_NOTE: i32 = 16;
pub const NODE_NOTE_CENTER: i32 = 36;
pub const NODE_MAX_WIDTH: u32 = 280;
/// Room a pill's rounded ends need past its label.
pub const PILL_EXTRA_WIDTH: u32 = 12;
/// A cylinder's caps take this much more height.
pub const DB_EXTRA_HEIGHT: u32 = 14;
/// A step's number is written before its label ("1. "), measured with it.
pub const STEP_PREFIX_SUFFIX: &str = ". ";

/// A field of a record and a cell: height, padding beside the value, narrowest.
pub const FIELD_HEIGHT: u32 = 30;
pub const FIELD_PADDING_X: u32 = 8;
pub const FIELD_MIN_WIDTH: u32 = 36;
/// A record's name: its centre line below the fields' bottom.
pub const RECORD_LABEL_CENTER: i32 = 16;
/// The room a record's name takes under its fields.
pub const RECORD_LABEL_SPACE: u32 = 28;

/// A row of cells in a diagram: their height, and the name written above them (centre line
/// above the cells' top), and index labels under each cell (centre line below its bottom).
pub const CELL_HEIGHT: u32 = 28;
pub const CELLS_LABEL_ABOVE: i32 = 12;
pub const CELLS_INDEX_BELOW: i32 = 11;
/// The room a name above and indexes below take.
pub const CELLS_LABEL_SPACE: u32 = 22;
pub const CELLS_INDEX_SPACE: u32 = 20;

/// An edge label's centre line above a straight edge's middle.
pub const EDGE_LABEL_GAP: i32 = 9;

/// A legend entry: the swatch's size, the gap before the name, and one entry's height.
pub const LEGEND_SWATCH_WIDTH: u32 = 14;
pub const LEGEND_SWATCH_HEIGHT: u32 = 10;
pub const LEGEND_GAP: u32 = 8;
pub const LEGEND_ROW_HEIGHT: u32 = 22;
/// Space between legend entries, and between the legend's row and the diagram under it.
pub const LEGEND_MARGIN: i32 = 24;

/// A container's content is at least this wide, and notes and runs of boxes wrap at no more
/// than this.
pub const CONTENT_WIDTH_MIN: u32 = 240;
pub const CONTENT_WIDTH_MAX: u32 = 560;
/// Containers side by side wrap onto a new row past this width: five or six sections of a map
/// stand in one row, as canvas 15's four allocators do.
pub const ROW_WIDTH_MAX: u32 = 2_400;

/// Geist Mono advances 0.6 em for every character.
const MONO_ADVANCE_THOUSANDTHS: u32 = 600;

/// Width of `text` in the code font at `font_size`.
#[must_use]
pub fn mono_width(text: &str, font_size: u32) -> u32 {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    chars
        .saturating_mul(MONO_ADVANCE_THOUSANDTHS)
        .saturating_mul(font_size)
        .div_ceil(1_000)
}

/// A line of handwriting at `font_size` starting at `left` on the centre line `center_y`.
#[must_use]
pub fn text_from_left(text: &str, left: i32, center_y: i32, font_size: u32) -> Rect {
    let height = font_size + font_size / 2;
    Rect {
        x: left,
        y: center_y - i32::try_from(height / 2).unwrap_or(0),
        w: text_width(text, font_size),
        h: height,
    }
}

/// A line of handwriting at `font_size` centred on `center`.
#[must_use]
pub fn text_centered(text: &str, center: Point, font_size: u32) -> Rect {
    let width = i32::try_from(text_width(text, font_size)).unwrap_or(0);
    text_from_left(text, center.x - width / 2, center.y, font_size)
}

/// A box's width and height for its label, its note and its step number.
#[must_use]
pub fn node_size(label: &str, note: Option<&str>, step: Option<u8>, pill: bool) -> (u32, u32) {
    let numbered = step.map(|number| format!("{number}{STEP_PREFIX_SUFFIX}{label}"));
    let label_width = text_width(numbered.as_deref().unwrap_or(label), LABEL_FONT_SIZE);
    let note_width = note.map_or(0, |note| text_width(note, NOTE_FONT_SIZE));
    let extra = if pill { PILL_EXTRA_WIDTH } else { 0 };
    let width = (label_width.max(note_width) + 2 * NODE_PADDING_X + extra)
        .clamp(NODE_MIN_WIDTH, NODE_MAX_WIDTH);
    let height = NODE_HEIGHT + note.map_or(0, |_| NODE_NOTE_HEIGHT);
    (width, height)
}

/// A field's or a cell's width for its value.
#[must_use]
pub fn field_width(value: &str) -> u32 {
    (mono_width(value, FIELD_FONT_SIZE) + 2 * FIELD_PADDING_X).max(FIELD_MIN_WIDTH)
}

/// Where a record's name is written: left-aligned under its fields.
#[must_use]
pub fn record_label_box(fields: &[CellBox], label: &str) -> Option<Rect> {
    let strip = fields.iter().map(|field| field.rect).reduce(Rect::union)?;
    (!label.is_empty()).then(|| {
        text_from_left(
            label,
            strip.x,
            strip.bottom() + RECORD_LABEL_CENTER,
            LABEL_FONT_SIZE,
        )
    })
}

/// What a row of cells in a diagram covers: the cells, the name above them and the index
/// labels under them.
#[must_use]
pub fn cells_area(cells: &[CellBox], label: &str) -> Rect {
    let Some(strip) = cells.iter().map(|cell| cell.rect).reduce(Rect::union) else {
        return Rect::default();
    };
    let mut area = strip;
    if !label.is_empty() {
        area = area.union(text_from_left(
            label,
            strip.x,
            strip.y - CELLS_LABEL_ABOVE,
            NOTE_FONT_SIZE,
        ));
    }
    for cell in cells {
        if let Some(index) = &cell.index {
            area = area.union(text_centered(
                index,
                Point {
                    x: cell.rect.center().x,
                    y: cell.rect.bottom() + CELLS_INDEX_BELOW,
                },
                FIELD_FONT_SIZE,
            ));
        }
    }
    area
}

/// Where an edge's label is centred: above the middle of a straight edge (beside a steep one, to
/// its right, so the words never sit on the line), or beside the middle of a curved one, on the
/// outside of its bend.
#[must_use]
pub fn edge_label_center(path: EdgePath, label: &str) -> Point {
    match path.control {
        None if path.start.y.abs_diff(path.end.y) > path.start.x.abs_diff(path.end.x) => {
            let width = i32::try_from(text_width(label, EDGE_LABEL_FONT_SIZE)).unwrap_or(0);
            Point {
                x: (path.start.x + path.end.x) / 2 + EDGE_LABEL_GAP + width / 2,
                y: (path.start.y + path.end.y) / 2,
            }
        }
        None => Point {
            x: (path.start.x + path.end.x) / 2,
            y: (path.start.y + path.end.y) / 2 - EDGE_LABEL_GAP,
        },
        Some(control) => {
            // A quadratic's middle is a quarter of each end plus half the control point.
            let middle = Point {
                x: (path.start.x + 2 * control.x + path.end.x) / 4,
                y: (path.start.y + 2 * control.y + path.end.y) / 4,
            };
            let chord_y = (path.start.y + path.end.y) / 2;
            let outward = if middle.y <= chord_y { -1 } else { 1 };
            Point {
                x: middle.x,
                y: middle.y + outward * EDGE_LABEL_GAP,
            }
        }
    }
}

/// Everything a part covers, its words included: for the camera (I11) and the checks.
#[must_use]
pub fn part_area(part: &StagedPart) -> Rect {
    match part {
        StagedPart::Frame { rect, .. }
        | StagedPart::Node { rect, .. }
        | StagedPart::Note { rect, .. }
        | StagedPart::Legend { rect, .. } => *rect,
        StagedPart::Record { label, fields, .. } => {
            let strip = fields
                .iter()
                .map(|field| field.rect)
                .reduce(Rect::union)
                .unwrap_or_default();
            record_label_box(fields, label).map_or(strip, |words| strip.union(words))
        }
        StagedPart::Cells { label, cells, .. } => cells_area(cells, label),
        StagedPart::Edge { path, label, .. } => {
            let line = path_bounds(*path);
            label.as_ref().map_or(line, |label| {
                line.union(text_centered(&label.text, label.at, EDGE_LABEL_FONT_SIZE))
            })
        }
    }
}

impl StagedPart {
    /// The element the part draws.
    #[must_use]
    pub fn id(&self) -> &crate::ids::ElementId {
        match self {
            Self::Frame { id, .. }
            | Self::Node { id, .. }
            | Self::Note { id, .. }
            | Self::Record { id, .. }
            | Self::Cells { id, .. }
            | Self::Edge { id, .. }
            | Self::Legend { id, .. } => id,
        }
    }

    /// Its words, for a past board's description; a note's first line.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Node { label, .. }
            | Self::Record { label, .. }
            | Self::Cells { label, .. }
            | Self::Legend { label, .. } => Some(label.as_str()),
            Self::Frame { label, .. } => Some(label.text.as_str()),
            Self::Note { lines, .. } => lines.first().map(String::as_str),
            Self::Edge { label, .. } => label.as_ref().map(|label| label.text.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FIELD_FONT_SIZE, mono_width, node_size};

    #[test]
    fn the_code_font_is_monospaced_and_boxes_fit_their_words() {
        assert_eq!(mono_width("free", FIELD_FONT_SIZE), 27);
        assert_eq!(mono_width("live", FIELD_FONT_SIZE), 27);
        let (plain, short) = node_size("class index", None, None, false);
        let (numbered, _) = node_size("class index", None, Some(3), false);
        let (_, noted) = node_size("class index", Some("O(1), no search"), None, false);
        assert!(numbered > plain);
        assert!(noted > short);
        let (pill, _) = node_size("class index", None, None, true);
        assert!(pill > plain);
    }
}
