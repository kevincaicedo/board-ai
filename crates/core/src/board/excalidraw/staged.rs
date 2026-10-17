//! A staged diagram's drawn parts in the `.excalidraw` export (D-187), in the canvas's diagram
//! sizes and categories: frames with their category's stroke (dashed when drawn dashed), labels
//! and notes where the board wrote them, boxes filled with their category's soft colour, records'
//! fields in the code font, connectors bound to their ends, and the legend entries drawn. Parts
//! still planned were never drawn, so they are not exported.

use serde_json::{Value, json};

use super::{INK, INK2, base, bind_text, line};
use crate::board::Layer;
use crate::board::metrics::text_width;
use crate::board::staged::metrics::{
    CELLS_INDEX_BELOW, CELLS_LABEL_ABOVE, EDGE_LABEL_FONT_SIZE, FIELD_FONT_SIZE,
    FRAME_LABEL_FONT_SIZE, LABEL_FONT_SIZE, LEGEND_FONT_SIZE, LEGEND_GAP, LEGEND_SWATCH_HEIGHT,
    LEGEND_SWATCH_WIDTH, NODE_LABEL_CENTER_WITH_NOTE, NODE_NOTE_CENTER, NOTE_FONT_SIZE,
    NOTE_LINE_HEIGHT, mono_width, record_label_box,
};
use crate::geometry::{Point, Rect};
use crate::protocol::{Category, CellBox, LineStyle, NodeShape, StagedPart};

/// Excalidraw's code font.
const CODE_FONT: u32 = 3;
const HAND_FONT: u32 = 5;
/// A frame with no category, in the light grey.
const PLAIN_FRAME: &str = "#c5c8cf";

/// Canvas 15's category pairs: the stroke, then the soft fill.
fn colours(category: Category) -> (&'static str, &'static str) {
    match category {
        Category::Slate => ("#9aa3b2", "#eef0f4"),
        Category::Violet => ("#b565d8", "#e9ddf6"),
        Category::Blue => ("#4a8bd6", "#e2eefb"),
        Category::Green => ("#2f8f5b", "#e3f5ea"),
    }
}

/// Every drawn part of the layer's staged diagrams, ids under `prefix`.
pub(super) fn export(out: &mut Vec<Value>, layer: &Layer, prefix: &str) {
    for diagram in layer.staged.diagrams() {
        for entry in &diagram.entries {
            let drawn = layer
                .element(&entry.id)
                .is_some_and(|element| element.state.is_shown());
            if drawn {
                part(out, prefix, &entry.draw);
            }
        }
        for legend in diagram.legend.iter().filter(|legend| legend.drawn) {
            let id = format!("{prefix}:{}", legend.id.as_str());
            let swatch = Rect {
                x: legend.rect.x,
                y: legend.rect.center().y - i32::try_from(LEGEND_SWATCH_HEIGHT / 2).unwrap_or(0),
                w: LEGEND_SWATCH_WIDTH,
                h: LEGEND_SWATCH_HEIGHT,
            };
            out.push(base(
                &id,
                "rectangle",
                swatch,
                colours(legend.category).0,
                LineStyle::Dashed,
            ));
            let left = legend.rect.x + i32::try_from(LEGEND_SWATCH_WIDTH + LEGEND_GAP).unwrap_or(0);
            out.push(words(
                &format!("{id}#label"),
                &legend.label,
                Place::Left(left, legend.rect.center().y),
                Look::hand(LEGEND_FONT_SIZE, INK),
            ));
        }
    }
}

fn part(out: &mut Vec<Value>, prefix: &str, draw: &StagedPart) {
    let id = format!("{prefix}:{}", draw.id().as_str());
    match draw {
        StagedPart::Frame { .. } => frame(out, &id, draw),
        StagedPart::Node { .. } => node(out, &id, draw),
        StagedPart::Note { rect, lines, .. } => {
            for (number, text) in lines.iter().enumerate() {
                let row = i32::try_from(number).unwrap_or(0);
                let center = rect.y + row * line_height() + line_height() / 2;
                let at = Place::Left(rect.x, center);
                out.push(words(
                    &format!("{id}#{number}"),
                    text,
                    at,
                    Look::hand(NOTE_FONT_SIZE, INK2),
                ));
            }
        }
        StagedPart::Record { label, fields, .. } => {
            cells(out, &id, fields);
            if let Some(name) = record_label_box(fields, label) {
                let at = Place::Left(name.x, name.center().y);
                out.push(words(
                    &format!("{id}#label"),
                    label,
                    at,
                    Look::hand(LABEL_FONT_SIZE, INK),
                ));
            }
        }
        StagedPart::Cells {
            label,
            cells: boxes,
            ..
        } => {
            cells(out, &id, boxes);
            if let Some(first) = boxes.first()
                && !label.is_empty()
            {
                let at = Place::Left(first.rect.x, first.rect.y - CELLS_LABEL_ABOVE);
                out.push(words(
                    &format!("{id}#label"),
                    label,
                    at,
                    Look::hand(NOTE_FONT_SIZE, INK2),
                ));
            }
        }
        StagedPart::Edge { .. } => edge(out, prefix, &id, draw),
        StagedPart::Legend { .. } => {}
    }
}

fn frame(out: &mut Vec<Value>, id: &str, draw: &StagedPart) {
    let StagedPart::Frame {
        rect,
        label,
        note,
        category,
        dashed,
        ..
    } = draw
    else {
        return;
    };
    let stroke = category.map_or(PLAIN_FRAME, |category| colours(category).0);
    out.push(rounded(base(
        id,
        "rectangle",
        *rect,
        stroke,
        style(*dashed),
    )));
    let at = Place::Left(label.at.x, label.at.y);
    let look = Look::hand(FRAME_LABEL_FONT_SIZE, INK);
    out.push(words(&format!("{id}#label"), &label.text, at, look));
    if let Some(note) = note {
        let at = Place::Left(note.at.x, note.at.y);
        out.push(words(
            &format!("{id}#note"),
            &note.text,
            at,
            Look::hand(NOTE_FONT_SIZE, INK2),
        ));
    }
}

fn edge(out: &mut Vec<Value>, prefix: &str, id: &str, draw: &StagedPart) {
    let StagedPart::Edge {
        from,
        to,
        path,
        label,
        style: line_style,
        ..
    } = draw
    else {
        return;
    };
    let points: Vec<Point> = [Some(path.start), path.control, Some(path.end)]
        .into_iter()
        .flatten()
        .collect();
    let mut arrow = line(id, &points, INK, *line_style);
    if let Value::Object(fields) = &mut arrow {
        fields.insert("type".into(), json!("arrow"));
        fields.insert("endArrowhead".into(), json!("arrow"));
        fields.insert("strokeWidth".into(), json!(1));
        for (key, end) in [("startBinding", from), ("endBinding", to)] {
            let end = format!("{prefix}:{}", end.as_str());
            fields.insert(
                key.into(),
                json!({ "elementId": end, "focus": 0, "gap": 4 }),
            );
        }
    }
    out.push(arrow);
    if let Some(label) = label {
        let at = Place::Center(label.at);
        out.push(words(
            &format!("{id}#label"),
            &label.text,
            at,
            Look::hand(EDGE_LABEL_FONT_SIZE, INK2),
        ));
    }
}

fn node(out: &mut Vec<Value>, id: &str, draw: &StagedPart) {
    let StagedPart::Node {
        shape,
        rect,
        label,
        note,
        step,
        category,
        dashed,
        ..
    } = draw
    else {
        return;
    };
    let kind = match shape {
        NodeShape::Db | NodeShape::Actor | NodeShape::Circle => "ellipse",
        NodeShape::Service
        | NodeShape::Queue
        | NodeShape::Group
        | NodeShape::Note
        | NodeShape::Pill => "rectangle",
    };
    let mut container = rounded(base(id, kind, *rect, INK, style(*dashed)));
    if let (Some(category), Value::Object(fields)) = (category, &mut container) {
        fields.insert("backgroundColor".into(), json!(colours(*category).1));
        fields.insert("fillStyle".into(), json!("solid"));
        fields.insert("strokeWidth".into(), json!(1));
    }
    let label_id = format!("{id}#label");
    bind_text(&mut container, &label_id);
    out.push(container);
    let text = step.map_or_else(|| label.clone(), |number| format!("{number}. {label}"));
    let (label_center, note_center) = match note {
        Some(_) => (
            rect.y + NODE_LABEL_CENTER_WITH_NOTE,
            rect.y + NODE_NOTE_CENTER,
        ),
        None => (rect.center().y, rect.center().y),
    };
    let mut written = words(
        &label_id,
        &text,
        Place::Center(Point {
            x: rect.center().x,
            y: label_center,
        }),
        Look::hand(LABEL_FONT_SIZE, INK),
    );
    if let Value::Object(fields) = &mut written {
        fields.insert("containerId".into(), json!(id));
    }
    out.push(written);
    if let Some(note) = note {
        let center = Point {
            x: rect.center().x,
            y: note_center,
        };
        out.push(words(
            &format!("{id}#note"),
            note,
            Place::Center(center),
            Look::hand(NOTE_FONT_SIZE, INK2),
        ));
    }
}

/// Fields or cells: a box each with its value in the code font, and its index label under it.
fn cells(out: &mut Vec<Value>, id: &str, boxes: &[CellBox]) {
    for cell in boxes {
        let cell_id = format!("{id}:{}", cell.id.as_str());
        out.push(base(
            &cell_id,
            "rectangle",
            cell.rect,
            INK,
            LineStyle::Solid,
        ));
        if !cell.value.is_empty() {
            out.push(words(
                &format!("{cell_id}#value"),
                &cell.value,
                Place::Center(cell.rect.center()),
                Look::code(FIELD_FONT_SIZE, INK),
            ));
        }
        if let Some(index) = &cell.index {
            let center = Point {
                x: cell.rect.center().x,
                y: cell.rect.bottom() + CELLS_INDEX_BELOW,
            };
            out.push(words(
                &format!("{cell_id}#index"),
                index,
                Place::Center(center),
                Look::code(FIELD_FONT_SIZE, INK2),
            ));
        }
    }
}

fn line_height() -> i32 {
    i32::try_from(NOTE_LINE_HEIGHT).unwrap_or(0)
}

fn style(dashed: bool) -> LineStyle {
    if dashed {
        LineStyle::Dashed
    } else {
        LineStyle::Solid
    }
}

/// Excalidraw's rounded corners, as the canvas's diagram boxes have.
fn rounded(mut value: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.insert("roundness".into(), json!({ "type": 3 }));
    }
    value
}

/// Where a line of text goes: its left end, or its middle, on its centre line.
enum Place {
    Left(i32, i32),
    Center(Point),
}

/// How a line of text is written.
struct Look {
    size: u32,
    family: u32,
    color: &'static str,
}

impl Look {
    fn hand(size: u32, color: &'static str) -> Self {
        Self {
            size,
            family: HAND_FONT,
            color,
        }
    }

    fn code(size: u32, color: &'static str) -> Self {
        Self {
            size,
            family: CODE_FONT,
            color,
        }
    }
}

fn words(id: &str, content: &str, place: Place, look: Look) -> Value {
    let width = if look.family == CODE_FONT {
        mono_width(content, look.size)
    } else {
        text_width(content, look.size)
    };
    let half = i32::try_from(width / 2).unwrap_or(0);
    let (left, center_y) = match place {
        Place::Left(x, y) => (x, y),
        Place::Center(center) => (center.x - half, center.y),
    };
    let height = look.size + look.size / 2;
    let rect = Rect {
        x: left,
        y: center_y - i32::try_from(height / 2).unwrap_or(0),
        w: width,
        h: height,
    };
    let mut value = base(id, "text", rect, look.color, LineStyle::Solid);
    if let Value::Object(fields) = &mut value {
        fields.insert("text".into(), json!(content));
        fields.insert("originalText".into(), json!(content));
        fields.insert("fontSize".into(), json!(look.size));
        fields.insert("fontFamily".into(), json!(look.family));
        fields.insert("textAlign".into(), json!("left"));
        fields.insert("verticalAlign".into(), json!("middle"));
        fields.insert("containerId".into(), Value::Null);
        fields.insert("autoResize".into(), json!(true));
        fields.insert("lineHeight".into(), json!(1.25));
    }
    value
}
