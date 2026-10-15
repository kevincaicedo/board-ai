//! Export of a layer as an `.excalidraw` file (LLD §2.5, D-007): version 2 JSON with
//! rectangles, ellipses, arrows, lines and text, hand-drawn (`roughness` 1) in Excalifont
//! (`fontFamily` 5). Seeds come from element ids, so the same board always exports the same
//! file, and nothing here reads a clock or a random source.

use serde_json::{Value, json};

use crate::board::metrics::{LABEL_FONT_SIZE, LABEL_LINE_HEIGHT, text_width};
use crate::board::{Element, ElementKind, ElementState, Geometry, Layer};
use crate::geometry::{Point, Rect};
use crate::protocol::{EdgePath, LineStyle, Mark, NodeShape};

const INK: &str = "#1e1e1e";
/// Annotations are orange (PRODUCT §7.1).
const PEN: &str = "#e8590c";
const TRACE: &str = "#d6336c";
const EXCALIFONT: u32 = 5;
/// Offset between the copies of a stacked node.
const STACK_OFFSET: i32 = 6;

/// The layer as a complete `.excalidraw` document.
#[must_use]
pub fn export_layer(layer: &Layer) -> String {
    let prefix = format!("l{}", layer.id().0);
    let mut elements = Vec::new();
    for element in layer.elements() {
        if element.state == ElementState::Pending {
            continue;
        }
        let id = format!("{prefix}:{}", element.id.as_str());
        match (&element.kind, &element.geometry) {
            (ElementKind::Node { shape, stack, .. }, Geometry::Box(rect)) => {
                node(&mut elements, &id, element, *shape, *stack, *rect);
            }
            (ElementKind::Edge { from, to, style }, Geometry::Path(path)) => {
                let ends = (
                    format!("{prefix}:{}", from.as_str()),
                    format!("{prefix}:{}", to.as_str()),
                );
                edge(&mut elements, &id, element, *path, *style, &ends);
            }
            (ElementKind::Annotation { mark, .. }, Geometry::Box(rect)) => {
                annotation(&mut elements, &id, element, *mark, *rect);
            }
            (ElementKind::Trace, Geometry::Points(points)) => {
                elements.push(line(&id, points, TRACE, LineStyle::Dashed));
            }
            (
                ElementKind::Node { .. }
                | ElementKind::Edge { .. }
                | ElementKind::Annotation { .. }
                | ElementKind::Trace,
                _,
            ) => debug_assert!(false, "kind and geometry always match"),
        }
    }
    json!({
        "type": "excalidraw",
        "version": 2,
        "source": "boardai",
        "elements": elements,
        "appState": { "viewBackgroundColor": "#ffffff", "gridSize": null },
        "files": {},
    })
    .to_string()
}

/// FNV-1a: a stable, dependency-free hash for seeds.
fn seed(text: &str) -> u32 {
    let hash = text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    (hash & 0x7fff_ffff).max(1)
}

fn base(id: &str, kind: &str, rect: Rect, stroke: &str, style: LineStyle) -> Value {
    json!({
        "id": id,
        "type": kind,
        "x": rect.x,
        "y": rect.y,
        "width": rect.w,
        "height": rect.h,
        "angle": 0,
        "strokeColor": stroke,
        "backgroundColor": "transparent",
        "fillStyle": "hachure",
        "strokeWidth": 2,
        "strokeStyle": match style { LineStyle::Solid => "solid", LineStyle::Dashed => "dashed" },
        "roughness": 1,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "roundness": null,
        "seed": seed(id),
        "version": 1,
        "versionNonce": seed(&format!("{id}#nonce")),
        "isDeleted": false,
        "boundElements": [],
        "updated": 1,
        "link": null,
        "locked": false,
    })
}

fn text(id: &str, content: &str, center: Point, container: Option<&str>, color: &str) -> Value {
    let width = text_width(content, LABEL_FONT_SIZE);
    let rect = Rect {
        x: center.x - i32::try_from(width / 2).unwrap_or(0),
        y: center.y - i32::try_from(LABEL_LINE_HEIGHT / 2).unwrap_or(0),
        w: width,
        h: LABEL_LINE_HEIGHT,
    };
    let mut value = base(id, "text", rect, color, LineStyle::Solid);
    if let Value::Object(fields) = &mut value {
        fields.insert("text".into(), json!(content));
        fields.insert("originalText".into(), json!(content));
        fields.insert("fontSize".into(), json!(LABEL_FONT_SIZE));
        fields.insert("fontFamily".into(), json!(EXCALIFONT));
        fields.insert("textAlign".into(), json!("center"));
        fields.insert("verticalAlign".into(), json!("middle"));
        fields.insert("containerId".into(), json!(container));
        fields.insert("autoResize".into(), json!(true));
        fields.insert("lineHeight".into(), json!(1.25));
    }
    value
}

/// Binds a text element to its container, both ways, as Excalidraw expects.
fn bind_text(container: &mut Value, text_id: &str) {
    if let Some(Value::Array(bound)) = container.get_mut("boundElements") {
        bound.push(json!({ "type": "text", "id": text_id }));
    }
}

fn node(
    out: &mut Vec<Value>,
    id: &str,
    element: &Element,
    shape: NodeShape,
    stack: u8,
    rect: Rect,
) {
    let label_id = format!("{id}#label");
    let kind = match shape {
        NodeShape::Db | NodeShape::Actor => "ellipse",
        NodeShape::Service | NodeShape::Queue | NodeShape::Group | NodeShape::Note => "rectangle",
    };
    if shape == NodeShape::Note {
        out.push(text(id, &element.label, rect.center(), None, INK));
        return;
    }
    for copy in (1..i32::from(stack)).rev() {
        let behind = Rect {
            x: rect.x + STACK_OFFSET * copy,
            y: rect.y - STACK_OFFSET * copy,
            ..rect
        };
        out.push(base(
            &format!("{id}#copy{copy}"),
            kind,
            behind,
            INK,
            LineStyle::Solid,
        ));
    }
    if shape == NodeShape::Group {
        out.push(base(id, kind, rect, INK, LineStyle::Dashed));
        let corner = Point {
            x: rect.x
                + i32::try_from(text_width(&element.label, LABEL_FONT_SIZE) / 2).unwrap_or(0)
                + 8,
            y: rect.y + i32::try_from(LABEL_LINE_HEIGHT / 2).unwrap_or(0) + 4,
        };
        out.push(text(&label_id, &element.label, corner, None, INK));
        return;
    }
    let mut container = base(id, kind, rect, INK, LineStyle::Solid);
    bind_text(&mut container, &label_id);
    out.push(container);
    out.push(text(
        &label_id,
        &element.label,
        rect.center(),
        Some(id),
        INK,
    ));
}

fn edge(
    out: &mut Vec<Value>,
    id: &str,
    element: &Element,
    path: EdgePath,
    style: LineStyle,
    (from, to): &(String, String),
) {
    let points: Vec<Point> = [Some(path.start), path.control, Some(path.end)]
        .into_iter()
        .flatten()
        .collect();
    let mut arrow = line(id, &points, INK, style);
    if let Value::Object(fields) = &mut arrow {
        fields.insert("type".into(), json!("arrow"));
        fields.insert("endArrowhead".into(), json!("arrow"));
        fields.insert(
            "startBinding".into(),
            json!({ "elementId": from, "focus": 0, "gap": 4 }),
        );
        fields.insert(
            "endBinding".into(),
            json!({ "elementId": to, "focus": 0, "gap": 4 }),
        );
    }
    if element.label.is_empty() {
        out.push(arrow);
        return;
    }
    let label_id = format!("{id}#label");
    bind_text(&mut arrow, &label_id);
    let middle = layout_middle(path);
    out.push(arrow);
    out.push(text(&label_id, &element.label, middle, Some(id), INK));
}

fn layout_middle(path: EdgePath) -> Point {
    let through = path.control.unwrap_or(Point {
        x: (path.start.x + path.end.x) / 2,
        y: (path.start.y + path.end.y) / 2,
    });
    Point {
        x: (path.start.x + 2 * through.x + path.end.x) / 4,
        y: (path.start.y + 2 * through.y + path.end.y) / 4,
    }
}

/// A line through `points`; Excalidraw stores points relative to the element's origin.
fn line(id: &str, points: &[Point], color: &str, style: LineStyle) -> Value {
    let origin = points.first().copied().unwrap_or_default();
    let bounds = points
        .iter()
        .map(|point| Rect {
            x: point.x,
            y: point.y,
            w: 0,
            h: 0,
        })
        .reduce(Rect::union)
        .unwrap_or_default();
    let mut value = base(
        id,
        "line",
        Rect {
            x: origin.x,
            y: origin.y,
            ..bounds
        },
        color,
        style,
    );
    let relative: Vec<[i32; 2]> = points
        .iter()
        .map(|point| [point.x - origin.x, point.y - origin.y])
        .collect();
    if let Value::Object(fields) = &mut value {
        fields.insert("points".into(), json!(relative));
        fields.insert("lastCommittedPoint".into(), Value::Null);
        fields.insert("startBinding".into(), Value::Null);
        fields.insert("endBinding".into(), Value::Null);
        fields.insert("startArrowhead".into(), Value::Null);
        fields.insert("endArrowhead".into(), Value::Null);
        fields.insert("elbowed".into(), json!(false));
    }
    value
}

fn annotation(out: &mut Vec<Value>, id: &str, element: &Element, mark: Mark, rect: Rect) {
    let left_middle = Point {
        x: rect.x,
        y: rect.center().y,
    };
    match mark {
        Mark::Circle => out.push(base(id, "ellipse", rect, PEN, LineStyle::Solid)),
        Mark::Underline => {
            let y = rect.bottom();
            let points = [Point { x: rect.x, y }, Point { x: rect.right(), y }];
            out.push(line(id, &points, PEN, LineStyle::Solid));
        }
        Mark::Arrow => {
            let from = Point {
                x: rect.x - 80,
                y: left_middle.y - 40,
            };
            let mut arrow = line(id, &[from, left_middle], PEN, LineStyle::Solid);
            if let Value::Object(fields) = &mut arrow {
                fields.insert("type".into(), json!("arrow"));
                fields.insert("endArrowhead".into(), json!("arrow"));
            }
            out.push(arrow);
        }
        Mark::Bracket => {
            let points = [
                Point {
                    x: rect.x + 8,
                    y: rect.y,
                },
                Point {
                    x: rect.x,
                    y: rect.y,
                },
                Point {
                    x: rect.x,
                    y: rect.bottom(),
                },
                Point {
                    x: rect.x + 8,
                    y: rect.bottom(),
                },
            ];
            out.push(line(id, &points, PEN, LineStyle::Solid));
        }
    }
    if !element.label.is_empty() {
        let below = Point {
            x: rect.center().x,
            y: rect.bottom() + i32::try_from(LABEL_LINE_HEIGHT).unwrap_or(0),
        };
        out.push(text(
            &format!("{id}#note"),
            &element.label,
            below,
            None,
            PEN,
        ));
    }
}
