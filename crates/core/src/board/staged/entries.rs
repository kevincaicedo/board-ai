//! A staged diagram's registry entry and its planned elements (D-187): every part drawn where
//! the layout put it, moved to where the diagram stands on the layer.

use crate::board::staged::layout::{Detail, Laid};
use crate::board::staged::metrics::{cells_area, part_area};
use crate::board::staged::structure::{Edge, Part, PartKind, Structure, sub_id};
use crate::board::staged::{Entry, LegendEntry, StagedDiagram};
use crate::board::{Element, ElementKind, ElementState, Geometry};
use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, OpId};
use crate::narration::ops::IndexLabels;
use crate::protocol::{CellBox, EdgePath, NodeShape, StagedPart, TextAt};

fn moved(rect: Rect, by: Point) -> Rect {
    Rect {
        x: rect.x + by.x,
        y: rect.y + by.y,
        ..rect
    }
}

fn moved_point(point: Point, by: Point) -> Point {
    Point {
        x: point.x + by.x,
        y: point.y + by.y,
    }
}

fn moved_text(text: &TextAt, by: Point) -> TextAt {
    TextAt {
        text: text.text.clone(),
        at: moved_point(text.at, by),
    }
}

/// The diagram's registry entry: its parts, its connectors and its legend, moved to `origin`.
pub(super) fn build(
    id: &ElementId,
    title: &str,
    structure: &Structure,
    laid: &Laid,
    origin: Point,
) -> StagedDiagram {
    let sections = sections(structure);
    let mut entries: Vec<Entry> = structure
        .parts
        .iter()
        .enumerate()
        .map(|(at, part)| Entry {
            id: part.id.clone(),
            parent: part.parent,
            section: sections[at],
            draw: part_draw(part, &laid.details[at], laid.boxes[at], origin),
            ends: None,
            tone: part.tone,
            subs: (0..part.sub_count())
                .filter_map(|index| sub_id(&part.id, index))
                .collect(),
        })
        .collect();
    for (edge, (path, label)) in structure.edges.iter().zip(&laid.edges) {
        let section = entries[edge.from_part].section;
        entries.push(edge_entry(edge, *path, label.as_ref(), origin, section));
    }
    let tops = structure
        .parts
        .iter()
        .filter(|part| part.parent.is_none())
        .count();
    StagedDiagram {
        id: id.clone(),
        title: title.to_owned(),
        bounds: Rect {
            x: origin.x,
            y: origin.y,
            w: laid.width,
            h: laid.height,
        },
        entries,
        legend: legend(id, structure, laid, origin),
        sections: u8::try_from(tops).unwrap_or(u8::MAX),
        part_shown: 0,
        provenance: None,
    }
}

/// Each part's top-level part, from 1, in the order the top-level parts were written.
fn sections(structure: &Structure) -> Vec<u8> {
    let tops: Vec<usize> = (0..structure.parts.len())
        .filter(|at| structure.parts[*at].parent.is_none())
        .collect();
    (0..structure.parts.len())
        .map(|mut at| {
            // Each step goes one container out; the structure has no cycles, so this ends.
            for _ in 0..structure.parts.len() {
                match structure.parts[at].parent {
                    Some(parent) => at = parent,
                    None => break,
                }
            }
            let number = tops
                .iter()
                .position(|top| *top == at)
                .map_or(1, |index| index + 1);
            u8::try_from(number).unwrap_or(u8::MAX)
        })
        .collect()
}

fn edge_entry(
    edge: &Edge,
    path: EdgePath,
    label: Option<&TextAt>,
    origin: Point,
    section: u8,
) -> Entry {
    let path = EdgePath {
        start: moved_point(path.start, origin),
        control: path.control.map(|control| moved_point(control, origin)),
        end: moved_point(path.end, origin),
    };
    Entry {
        id: edge.id.clone(),
        parent: None,
        section,
        draw: StagedPart::Edge {
            id: edge.id.clone(),
            from: edge.from.clone(),
            to: edge.to.clone(),
            path,
            label: label.map(|label| moved_text(label, origin)),
            style: edge.style,
        },
        ends: Some((edge.from_part, edge.to_part)),
        tone: None,
        subs: Vec::new(),
    }
}

fn legend(id: &ElementId, structure: &Structure, laid: &Laid, origin: Point) -> Vec<LegendEntry> {
    structure
        .categories
        .iter()
        .zip(&laid.legend)
        .enumerate()
        .filter_map(|(row, ((category, label), rect))| {
            Some(LegendEntry {
                id: ElementId::parse(&format!("{}:legend{row}", id.as_str()))?,
                category: *category,
                label: label.clone(),
                rect: moved(*rect, origin),
                drawn: false,
            })
        })
        .collect()
}

/// How a part is drawn, laid out at `rect` with its details, all moved by `origin`.
fn part_draw(part: &Part, detail: &Detail, rect: Rect, origin: Point) -> StagedPart {
    let rect = moved(rect, origin);
    let boxes =
        |inner: &[Rect]| -> Vec<Rect> { inner.iter().map(|inner| moved(*inner, origin)).collect() };
    let id = part.id.clone();
    match (&part.kind, detail) {
        (PartKind::Container { dashed, .. }, Detail::Frame { label, note }) => StagedPart::Frame {
            id,
            rect,
            label: moved_text(label, origin),
            note: note.as_ref().map(|note| moved_text(note, origin)),
            category: part.tone,
            dashed: *dashed,
        },
        (PartKind::Node { .. }, _) => node_draw(part, rect),
        (PartKind::Note { .. }, Detail::Note { lines }) => StagedPart::Note {
            id,
            rect,
            lines: lines.clone(),
        },
        (PartKind::Record { label, fields }, Detail::Record { fields: inner }) => {
            StagedPart::Record {
                label: label.clone(),
                fields: cell_boxes(&id, fields, &boxes(inner), &IndexLabels::None),
                id,
            }
        }
        (
            PartKind::Cells {
                label,
                values,
                index,
                vertical,
            },
            Detail::Cells { cells },
        ) => StagedPart::Cells {
            label: label.clone(),
            // A column in a diagram has no index labels: under each cell they would run into
            // the next one.
            cells: cell_boxes(
                &id,
                values,
                &boxes(cells),
                if *vertical { &IndexLabels::None } else { index },
            ),
            vertical: *vertical,
            id,
        },
        // The layout gives every part its own kind of detail.
        (
            PartKind::Container { .. }
            | PartKind::Note { .. }
            | PartKind::Record { .. }
            | PartKind::Cells { .. },
            _,
        ) => StagedPart::Note {
            id,
            rect,
            lines: Vec::new(),
        },
    }
}

fn node_draw(part: &Part, rect: Rect) -> StagedPart {
    let PartKind::Node {
        label,
        shape,
        note,
        step,
        dashed,
        ..
    } = &part.kind
    else {
        return StagedPart::Note {
            id: part.id.clone(),
            rect,
            lines: Vec::new(),
        };
    };
    StagedPart::Node {
        id: part.id.clone(),
        shape: *shape,
        rect,
        label: label.clone(),
        note: note.clone(),
        step: *step,
        category: part.tone,
        dashed: *dashed,
    }
}

fn cell_boxes(
    id: &ElementId,
    values: &[String],
    boxes: &[Rect],
    index: &IndexLabels,
) -> Vec<CellBox> {
    values
        .iter()
        .zip(boxes)
        .enumerate()
        .filter_map(|(at, (value, rect))| {
            Some(CellBox {
                id: sub_id(id, at)?,
                rect: *rect,
                value: value.clone(),
                index: match index {
                    IndexLabels::None => None,
                    IndexLabels::From(first) => {
                        Some((u64::from(*first) + u64::try_from(at).ok()?).to_string())
                    }
                    IndexLabels::Named(names) => names.get(at).cloned(),
                },
            })
        })
        .collect()
}

fn planned(
    id: &ElementId,
    op: OpId,
    kind: ElementKind,
    label: &str,
    geometry: Geometry,
) -> Element {
    Element {
        id: id.clone(),
        op: Some(op),
        kind,
        label: label.to_owned(),
        geometry,
        state: ElementState::Planned,
    }
}

fn node_kind(shape: NodeShape) -> ElementKind {
    ElementKind::Node {
        shape,
        stack: 1,
        group: None,
        span: None,
        fill: None,
        parent: None,
    }
}

/// A record's or a row's element, then one per field or cell (`id.i`).
fn strip(
    id: &ElementId,
    op: OpId,
    label: &str,
    cells: &[CellBox],
    vertical: bool,
    extent: Rect,
) -> Vec<Element> {
    let boxes = cells
        .iter()
        .map(|cell| cell.rect)
        .reduce(Rect::union)
        .unwrap_or(extent);
    let kind = ElementKind::Cells {
        vertical,
        span: None,
        extent,
    };
    let mut elements = vec![planned(id, op, kind, label, Geometry::Box(boxes))];
    elements.extend(cells.iter().enumerate().map(|(index, cell)| {
        let kind = ElementKind::Cell {
            strip: id.clone(),
            index: u8::try_from(index).unwrap_or(u8::MAX),
            index_label: cell.index.clone(),
        };
        planned(&cell.id, op, kind, &cell.value, Geometry::Box(cell.rect))
    }));
    elements
}

/// The planned elements an entry adds: the part, then its fields or cells.
pub(super) fn elements_of(entry: &Entry, op: OpId) -> Vec<Element> {
    match &entry.draw {
        StagedPart::Frame {
            id, rect, label, ..
        } => vec![planned(
            id,
            op,
            node_kind(NodeShape::Group),
            &label.text,
            Geometry::Box(*rect),
        )],
        StagedPart::Node {
            id,
            shape,
            rect,
            label,
            ..
        } => vec![planned(
            id,
            op,
            node_kind(*shape),
            label,
            Geometry::Box(*rect),
        )],
        StagedPart::Note { id, rect, lines } => vec![planned(
            id,
            op,
            node_kind(NodeShape::Note),
            &lines.join(" "),
            Geometry::Box(*rect),
        )],
        StagedPart::Record { id, label, fields } => {
            strip(id, op, label, fields, false, part_area(&entry.draw))
        }
        StagedPart::Cells {
            id,
            label,
            cells,
            vertical,
        } => strip(id, op, label, cells, *vertical, cells_area(cells, label)),
        StagedPart::Edge {
            id,
            from,
            to,
            path,
            label,
            style,
        } => {
            let kind = ElementKind::Edge {
                from: from.clone(),
                to: to.clone(),
                style: *style,
            };
            let label = label.as_ref().map_or("", |label| label.text.as_str());
            vec![planned(id, op, kind, label, Geometry::Path(*path))]
        }
        // Legend entries are the diagram's own, never elements.
        StagedPart::Legend { .. } => Vec::new(),
    }
}
