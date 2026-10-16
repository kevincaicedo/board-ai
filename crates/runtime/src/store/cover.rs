//! Cover art for the library (PRODUCT §7.6: "a blurred pastel render of the session's
//! diagram"): the main board's node boxes, read from the session's `board.excalidraw` (one frame
//! per layer; detours sit beside the main board and are left out). The UI draws them soft and
//! blurred; only where the shapes are matters here.

use boardai_core::geometry::Rect;
use boardai_core::protocol::NodeShape;
use boardai_core::views::CoverShape;
use serde_json::Value;

use crate::limits::COVER_SHAPES_MAX;

/// The node boxes of an `.excalidraw` document, in drawing order, at most
/// [`COVER_SHAPES_MAX`]. Labels, the offset copies of stacked nodes, arrows and lines are left
/// out; anything unreadable gives no shapes rather than an error (the cover is decoration).
#[must_use]
pub(crate) fn cover_shapes(excalidraw: &str) -> Vec<CoverShape> {
    let Ok(document) = serde_json::from_str::<Value>(excalidraw) else {
        return Vec::new();
    };
    let Some(elements) = document.get("elements").and_then(Value::as_array) else {
        return Vec::new();
    };
    elements
        .iter()
        .filter_map(shape)
        .take(COVER_SHAPES_MAX)
        .collect()
}

fn shape(element: &Value) -> Option<CoverShape> {
    let id = element.get("id")?.as_str()?;
    // `l0:api#label` is a label and `l0:api#copy1` a stacked copy: the node itself is `l0:api`.
    if id.contains('#')
        || on_a_detour(id)
        || element.get("isDeleted").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let dashed = element.get("strokeStyle").and_then(Value::as_str) == Some("dashed");
    let shape = match element.get("type")?.as_str()? {
        "ellipse" => NodeShape::Db,
        "rectangle" if dashed => NodeShape::Group,
        "rectangle" => NodeShape::Service,
        _ => return None,
    };
    let number = |key: &str| element.get(key).and_then(Value::as_f64);
    let rect = Rect {
        x: to_i32(number("x")?)?,
        y: to_i32(number("y")?)?,
        w: to_u32(number("width")?)?,
        h: to_u32(number("height")?)?,
    };
    (rect.w > 0 && rect.h > 0).then_some(CoverShape { rect, shape })
}

/// `l2:redis` is on detour layer 2; `l0:` is the main board, and other ids (a hand-made
/// board) count as main.
fn on_a_detour(id: &str) -> bool {
    id.split_once(':').is_some_and(|(layer, _)| {
        layer.strip_prefix('l').is_some_and(|number| {
            !number.is_empty() && number != "0" && number.bytes().all(|b| b.is_ascii_digit())
        })
    })
}

fn to_i32(value: f64) -> Option<i32> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= f64::from(i32::MIN) && rounded <= f64::from(i32::MAX)).then(
        || {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the range was checked above"
            )]
            let value = rounded as i32;
            value
        },
    )
}

fn to_u32(value: f64) -> Option<u32> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(u32::MAX)).then(|| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the range was checked above"
        )]
        let value = rounded as u32;
        value
    })
}

#[cfg(test)]
mod tests {
    use boardai_core::protocol::NodeShape;
    use serde_json::json;

    use super::cover_shapes;
    use crate::limits::COVER_SHAPES_MAX;

    fn element(id: &str, kind: &str, style: &str) -> serde_json::Value {
        json!({ "id": id, "type": kind, "x": 10.4, "y": -20, "width": 160, "height": 64,
                "strokeStyle": style, "isDeleted": false })
    }

    #[test]
    fn nodes_become_cover_shapes_and_the_rest_is_left_out() {
        let document = json!({ "elements": [
            element("l0:api", "rectangle", "solid"),
            element("l0:api#copy1", "rectangle", "solid"),
            element("l0:api#label", "text", "solid"),
            element("l0:db", "ellipse", "solid"),
            element("l0:vpc", "rectangle", "dashed"),
            element("l0:api>db", "arrow", "solid"),
            element("l1:redis", "rectangle", "solid"),
            element("frame:0", "frame", "solid"),
        ]});
        let shapes = cover_shapes(&document.to_string());
        let kinds: Vec<NodeShape> = shapes.iter().map(|shape| shape.shape).collect();
        assert_eq!(
            kinds,
            vec![NodeShape::Service, NodeShape::Db, NodeShape::Group]
        );
        assert_eq!((shapes[0].rect.x, shapes[0].rect.w), (10, 160));
    }

    #[test]
    fn junk_gives_no_cover_and_big_boards_are_bounded() {
        assert!(cover_shapes("not json").is_empty());
        assert!(cover_shapes(r#"{"elements": 3}"#).is_empty());
        let many: Vec<serde_json::Value> = (0..COVER_SHAPES_MAX + 5)
            .map(|index| element(&format!("l0:n{index}"), "rectangle", "solid"))
            .collect();
        let document = json!({ "elements": many });
        assert_eq!(cover_shapes(&document.to_string()).len(), COVER_SHAPES_MAX);
        let bad = json!({ "elements": [{ "id": "l0:x", "type": "rectangle", "x": 1e300,
                          "y": 0, "width": 10, "height": 10 }] });
        assert!(cover_shapes(&bad.to_string()).is_empty());
    }
}
