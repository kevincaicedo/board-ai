//! A staged diagram's connectors (D-187), routed once every part has its place: from the nearest
//! sides of their ends, straight unless a box or a label is in the way, else the bow that crosses
//! the fewest; each label beside its line where it lands on nothing: above a level line (or under
//! it), right of a steep one (or left), outside a bend (or inside).

use crate::board::metrics::text_width;
use crate::board::route;
use crate::board::staged::layout::Detail;
use crate::board::staged::metrics::{
    CELLS_LABEL_ABOVE, EDGE_LABEL_FONT_SIZE, EDGE_LABEL_GAP, FRAME_LABEL_FONT_SIZE,
    LABEL_FONT_SIZE, NOTE_FONT_SIZE, RECORD_LABEL_CENTER, edge_label_center, text_centered,
    text_from_left,
};
use crate::board::staged::structure::{Edge, PartKind, Structure};
use crate::geometry::{Point, Rect};
use crate::ids::ElementId;
use crate::protocol::{EdgePath, TextAt};

/// How far a blocked connector first bows off its straight line: about a box's height, so a
/// curve clears a neighbour without looping across a section.
const CURVE_LIFT: i32 = 30;

/// A container's label and note, as written in its header.
fn header_box(detail: &Detail) -> Option<Rect> {
    let Detail::Frame { label, note } = detail else {
        return None;
    };
    let words = text_from_left(&label.text, label.at.x, label.at.y, FRAME_LABEL_FONT_SIZE);
    Some(note.as_ref().map_or(words, |note| {
        words.union(text_from_left(
            &note.text,
            note.at.x,
            note.at.y,
            NOTE_FONT_SIZE,
        ))
    }))
}

/// A part's box an end can join: a record's or a row's own boxes rather than its words.
fn joinable(boxes: &[Rect], details: &[Detail], at: usize) -> Rect {
    match &details[at] {
        Detail::Record { fields } => fields.iter().copied().reduce(Rect::union),
        Detail::Cells { cells } => cells.iter().copied().reduce(Rect::union),
        Detail::Frame { .. } | Detail::Box | Detail::Note { .. } => None,
    }
    .unwrap_or(boxes[at])
}

/// The box of one end: a part's, or one of its fields or cells (`arena.2`).
fn end_box(boxes: &[Rect], details: &[Detail], at: usize, sub: Option<usize>) -> Rect {
    let subs = match &details[at] {
        Detail::Record { fields } => Some(fields),
        Detail::Cells { cells } => Some(cells),
        Detail::Frame { .. } | Detail::Box | Detail::Note { .. } => None,
    };
    sub.and_then(|index| subs?.get(index).copied())
        .unwrap_or_else(|| joinable(boxes, details, at))
}

pub(super) fn route_edges(
    structure: &Structure,
    boxes: &[Rect],
    details: &[Detail],
) -> Vec<(EdgePath, Option<TextAt>)> {
    let mut labels: Vec<Rect> = Vec::new();
    structure
        .edges
        .iter()
        .map(|edge| {
            let (from, to) = ends(structure, boxes, details, edge);
            let obstacles = in_the_way(structure, boxes, details, edge);
            let path = route::staged_edge_path(from, to, &obstacles, CURVE_LIFT);
            let label = edge.label.clone().map(|text| {
                // Its ends' own words are in a label's way too (a row's name above its cells).
                let mut taken = labels.clone();
                taken.extend(words(structure, details, edge.from_part));
                taken.extend(words(structure, details, edge.to_part));
                let at = label_center(path, &text, &obstacles, &taken);
                labels.push(text_centered(&text, at, EDGE_LABEL_FONT_SIZE));
                TextAt { at, text }
            });
            (path, label)
        })
        .collect()
}

/// The boxes a connector's ends join: their parts', or a field's or a cell's (`arena.2`).
fn ends(structure: &Structure, boxes: &[Rect], details: &[Detail], edge: &Edge) -> (Rect, Rect) {
    let sub_of = |id: &ElementId, at: usize| {
        let base = structure.parts[at].id.as_str();
        id.as_str()
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(|index| index.parse::<usize>().ok())
    };
    (
        end_box(
            boxes,
            details,
            edge.from_part,
            sub_of(&edge.from, edge.from_part),
        ),
        end_box(boxes, details, edge.to_part, sub_of(&edge.to, edge.to_part)),
    )
}

/// Boxes and words in a connector's way; a container's frame is crossed when its ends are in
/// different sections, but never its label.
fn in_the_way(structure: &Structure, boxes: &[Rect], details: &[Detail], edge: &Edge) -> Vec<Rect> {
    structure
        .parts
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != edge.from_part && *at != edge.to_part)
        .filter_map(|(at, part)| {
            if part.is_container() {
                header_box(&details[at])
            } else {
                Some(joinable(boxes, details, at))
            }
        })
        .collect()
}

/// A part's words outside its boxes: a container's header, a record's name under its fields, a
/// row's name above its cells.
fn words(structure: &Structure, details: &[Detail], at: usize) -> Option<Rect> {
    let label = match &structure.parts[at].kind {
        PartKind::Record { label, .. } | PartKind::Cells { label, .. } => label.as_str(),
        PartKind::Container { .. } | PartKind::Node { .. } | PartKind::Note { .. } => "",
    };
    match &details[at] {
        Detail::Frame { .. } => header_box(&details[at]),
        Detail::Record { fields } => {
            let strip = fields.iter().copied().reduce(Rect::union)?;
            let center = strip.bottom() + RECORD_LABEL_CENTER;
            (!label.is_empty()).then(|| text_from_left(label, strip.x, center, LABEL_FONT_SIZE))
        }
        Detail::Cells { cells } => {
            let strip = cells.iter().copied().reduce(Rect::union)?;
            let center = strip.y - CELLS_LABEL_ABOVE;
            (!label.is_empty()).then(|| text_from_left(label, strip.x, center, NOTE_FONT_SIZE))
        }
        Detail::Box | Detail::Note { .. } => None,
    }
}

/// Where a connector's label is centred: the first place beside its line where it lands on no
/// box, word or other label, else the usual place.
fn label_center(path: EdgePath, text: &str, obstacles: &[Rect], labels: &[Rect]) -> Point {
    let usual = edge_label_center(path, text);
    let clear = |center: &Point| {
        let words = text_centered(text, *center, EDGE_LABEL_FONT_SIZE);
        !obstacles
            .iter()
            .chain(labels)
            .any(|other| other.intersects(words))
    };
    if clear(&usual) {
        return usual;
    }
    alternatives(path, text, usual)
        .into_iter()
        .find(|center| clear(center))
        .unwrap_or(usual)
}

/// The other sides of the line, then a third of the way along each.
fn alternatives(path: EdgePath, text: &str, usual: Point) -> [Point; 3] {
    // The label's mirror across the line's middle, or across a bend's chord.
    let (start, end) = (path.start, path.end);
    let through = path.control.unwrap_or(Point {
        x: (start.x + end.x) / 2,
        y: (start.y + end.y) / 2,
    });
    let middle = Point {
        x: (start.x + 2 * through.x + end.x) / 4,
        y: (start.y + 2 * through.y + end.y) / 4,
    };
    let mirror = Point {
        x: 2 * middle.x - usual.x,
        y: 2 * middle.y - usual.y,
    };
    let steep = start.y.abs_diff(end.y) > start.x.abs_diff(end.x);
    let shift =
        i32::try_from(text_width(text, EDGE_LABEL_FONT_SIZE)).unwrap_or(0) / 2 + EDGE_LABEL_GAP;
    let along = |fraction_x3: i32| Point {
        x: start.x + (end.x - start.x) * fraction_x3 / 3 + if steep { shift } else { 0 },
        y: start.y + (end.y - start.y) * fraction_x3 / 3 - if steep { 0 } else { EDGE_LABEL_GAP },
    };
    [mirror, along(1), along(2)]
}
