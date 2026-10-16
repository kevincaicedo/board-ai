//! Export of the board as `.excalidraw` files (LLD §2.5, D-007): version 2 JSON with
//! rectangles, ellipses, arrows, lines and text, hand-drawn (`roughness` 1) in Excalifont
//! (`fontFamily` 5), with the board's hachure tones. One layer exports alone, or the whole
//! session exports as one document with a frame per layer, side by side. Seeds come from element
//! ids, so the same board always exports the same file, and nothing here reads a clock or a
//! random source.

use serde_json::{Value, json};

use crate::board::metrics::{LABEL_FONT_SIZE, LABEL_LINE_HEIGHT, text_width};
use crate::board::{Board, Element, ElementKind, ElementState, Geometry, Layer, layout};
use crate::geometry::{Point, Rect};
use crate::protocol::{EdgePath, Fill, LineStyle, Mark, NodeShape};

const INK: &str = "#1e1e1e";
/// A group's frame is light grey and solid, its label above it (D-032).
const GROUP_STROKE: &str = "#c5c8cf";
/// How far above a group's frame its label sits (the canvas's `LABEL_ABOVE`).
const GROUP_LABEL_ABOVE: i32 = 21;
/// Room between a layer's drawing and its frame in the session export.
const FRAME_PADDING: i32 = 80;
/// Space between neighbouring layer frames in the session export.
const FRAME_GAP: u32 = 160;
const FRAME_STROKE: &str = "#bbbbbb";
/// Annotations are orange (PRODUCT §7.1).
const PEN: &str = "#e8590c";
const TRACE: &str = "#d6336c";
const EXCALIFONT: u32 = 5;
/// Offset between the copies of a stacked node.
const STACK_OFFSET: i32 = 6;

/// The sketch fills' colors (the canvas's `--sketch-*` tokens).
fn fill_color(fill: Fill) -> &'static str {
    match fill {
        Fill::Lilac => "#d9c8f5",
        Fill::Sky => "#c4def6",
        Fill::Mint => "#cfefd9",
        Fill::Blush => "#f7d3e0",
    }
}

/// The layer as a complete `.excalidraw` document.
#[must_use]
pub fn export_layer(layer: &Layer) -> String {
    document(layer_elements(layer))
}

/// The whole board as one document: a frame per layer that has any drawing (the main board,
/// then each detour in the order it opened), side by side with their tops aligned. Each
/// element names its frame, so Excalidraw moves and exports them together.
#[must_use]
pub fn export_board(board: &Board) -> String {
    let mut elements = Vec::new();
    let mut left = 0_i32;
    for layer in board.layers() {
        let Some(bounds) = layer.bounds() else {
            continue;
        };
        // A group's label sits above its frame, outside the bounds.
        let area = layout::inflate(bounds, FRAME_PADDING, GROUP_LABEL_ABOVE);
        let (dx, dy) = (left - area.x, -area.y);
        let frame_id = format!("frame:{}", layer.id().0);
        for mut element in layer_elements(layer) {
            shift(&mut element, dx, dy);
            if let Value::Object(fields) = &mut element {
                fields.insert("frameId".into(), json!(frame_id));
            }
            elements.push(element);
        }
        let name = if layer.id().0 == 0 {
            "Main board".to_owned()
        } else {
            format!("Detour: {}", layer.title())
        };
        elements.push(frame(
            &frame_id,
            &name,
            Rect {
                x: left,
                y: 0,
                ..area
            },
        ));
        left = left.saturating_add_unsigned(area.w.saturating_add(FRAME_GAP));
    }
    document(elements)
}

fn document(elements: Vec<Value>) -> String {
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

/// Moves an element; line and arrow points are relative to its origin, so they follow.
fn shift(element: &mut Value, dx: i32, dy: i32) {
    for (key, delta) in [("x", dx), ("y", dy)] {
        if let Some(value) = element.get(key).and_then(Value::as_i64) {
            element[key] = json!(value + i64::from(delta));
        }
    }
}

/// An Excalidraw frame named after its layer; children come before it, as Excalidraw orders them.
fn frame(id: &str, name: &str, rect: Rect) -> Value {
    let mut value = base(id, "frame", rect, FRAME_STROKE, LineStyle::Solid);
    if let Value::Object(fields) = &mut value {
        fields.insert("name".into(), json!(name));
        fields.insert("roughness".into(), json!(0));
        fields.insert("fillStyle".into(), json!("solid"));
    }
    value
}

/// A layer's drawn elements as Excalidraw elements, in world coordinates.
fn layer_elements(layer: &Layer) -> Vec<Value> {
    let prefix = format!("l{}", layer.id().0);
    let mut elements = Vec::new();
    for element in layer.elements() {
        if element.state == ElementState::Pending {
            continue;
        }
        let id = format!("{prefix}:{}", element.id.as_str());
        match (&element.kind, &element.geometry) {
            (
                ElementKind::Node {
                    shape, stack, fill, ..
                },
                Geometry::Box(rect),
            ) => {
                let look = NodeLook {
                    shape: *shape,
                    stack: *stack,
                    fill: *fill,
                };
                node(&mut elements, &id, element, look, *rect);
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
    elements
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

/// How a node is drawn: its shape, its stacked copies and its tone.
#[derive(Clone, Copy)]
struct NodeLook {
    shape: NodeShape,
    stack: u8,
    fill: Option<Fill>,
}

fn node(out: &mut Vec<Value>, id: &str, element: &Element, look: NodeLook, rect: Rect) {
    let NodeLook { shape, stack, fill } = look;
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
        out.push(base(id, kind, rect, GROUP_STROKE, LineStyle::Solid));
        let above = Point {
            x: rect.center().x,
            y: rect.y - GROUP_LABEL_ABOVE,
        };
        out.push(text(&label_id, &element.label, above, None, INK));
        return;
    }
    let mut container = base(id, kind, rect, INK, LineStyle::Solid);
    if let (Some(fill), Value::Object(fields)) = (fill, &mut container) {
        fields.insert("backgroundColor".into(), json!(fill_color(fill)));
    }
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
