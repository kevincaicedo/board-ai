//! Shapes the user drew, exported in their ink (artboard 13's `--ink-you`): a box as a
//! rectangle with its label inside, an arrow bound to the shapes it joins, and pen strokes as
//! Excalidraw freehand lines grouped into one shape. A note is written under its shape.

use serde_json::{Value, json};

use super::{base, bind_text, line, small_text, text};
use crate::board::metrics::text_width;
use crate::board::user::{ARROW_LABEL_ABOVE, NOTE_BELOW, NOTE_FONT_SIZE, UserMark};
use crate::board::{Element, Geometry};
use crate::geometry::{Point, Rect};
use crate::protocol::LineStyle;

/// The user's ink (the canvas's `--ink-you`).
const YOU: &str = "#5b5bd6";

/// One of the user's shapes as Excalidraw elements, ids prefixed with its layer's.
pub(super) fn export(
    out: &mut Vec<Value>,
    prefix: &str,
    element: &Element,
    mark: &UserMark,
    note: Option<&str>,
) {
    let id = format!("{prefix}:{}", element.id.as_str());
    let label_id = format!("{id}#label");
    match (mark, &element.geometry) {
        (UserMark::Box, Geometry::Box(rect)) => {
            let mut container = base(&id, "rectangle", *rect, YOU, LineStyle::Solid);
            if element.label.is_empty() {
                out.push(container);
            } else {
                bind_text(&mut container, &label_id);
                out.push(container);
                out.push(text(
                    &label_id,
                    &element.label,
                    rect.center(),
                    Some(&id),
                    YOU,
                ));
            }
        }
        (UserMark::Arrow { from, to }, Geometry::Path(path)) => {
            let mut arrow = line(&id, &[path.start, path.end], YOU, LineStyle::Solid);
            if let Value::Object(fields) = &mut arrow {
                fields.insert("type".into(), json!("arrow"));
                fields.insert("endArrowhead".into(), json!("arrow"));
                let bound = |end: &Option<crate::ids::ElementId>| {
                    end.as_ref().map_or(Value::Null, |end| {
                        json!({ "elementId": format!("{prefix}:{}", end.as_str()), "focus": 0, "gap": 4 })
                    })
                };
                fields.insert("startBinding".into(), bound(from));
                fields.insert("endBinding".into(), bound(to));
            }
            out.push(arrow);
            if !element.label.is_empty() {
                let above = Point {
                    x: (path.start.x + path.end.x) / 2,
                    y: (path.start.y + path.end.y) / 2 - ARROW_LABEL_ABOVE,
                };
                out.push(colored(text(&label_id, &element.label, above, None, YOU)));
            }
        }
        (UserMark::Ink { strokes }, Geometry::Box(bounds)) => {
            for (index, stroke) in strokes.iter().enumerate() {
                out.push(freehand(&format!("{id}#{index}"), &id, stroke));
            }
            if !element.label.is_empty() {
                out.push(text(&label_id, &element.label, bounds.center(), None, YOU));
            }
        }
        (UserMark::Box | UserMark::Arrow { .. } | UserMark::Ink { .. }, _) => {
            debug_assert!(false, "a shape's mark and geometry always match");
        }
    }
    if let Some(note) = note {
        out.push(note_text(&format!("{id}#note"), note, element));
    }
}

/// A pen stroke as an Excalidraw freehand line, grouped with the shape's other strokes.
fn freehand(id: &str, group: &str, points: &[Point]) -> Value {
    let mut value = line(id, points, YOU, LineStyle::Solid);
    if let Value::Object(fields) = &mut value {
        fields.insert("type".into(), json!("freedraw"));
        fields.insert("pressures".into(), json!([]));
        fields.insert("simulatePressure".into(), json!(true));
        fields.insert("groupIds".into(), json!([group]));
        fields.remove("startArrowhead");
        fields.remove("endArrowhead");
        fields.remove("startBinding");
        fields.remove("endBinding");
        fields.remove("elbowed");
    }
    value
}

/// The note under a shape: left-aligned with a box or pen shape, centred under an arrow's middle,
/// where the core measured it (`user::area`).
fn note_text(id: &str, note: &str, element: &Element) -> Value {
    let width = i32::try_from(text_width(note, NOTE_FONT_SIZE)).unwrap_or(0);
    let center = match &element.geometry {
        Geometry::Path(path) => Point {
            x: (path.start.x + path.end.x) / 2,
            y: (path.start.y + path.end.y) / 2 + NOTE_BELOW,
        },
        Geometry::Box(_) | Geometry::Points(_) => {
            let bounds: Rect = element.geometry.bounds();
            Point {
                x: bounds.x + width / 2,
                y: bounds.bottom() + NOTE_BELOW,
            }
        }
    };
    colored(small_text(id, note, center, NOTE_FONT_SIZE))
}

/// Free text in the user's ink.
fn colored(mut value: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.insert("strokeColor".into(), json!(YOU));
    }
    value
}
