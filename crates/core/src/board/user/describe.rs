//! Your drawing as the talker reads it, in the turn you finish it and in the board summary:
//! `u1 "Client" → u2 "API" → u3 "Postgres" (source of truth); u4 "cache" under u2, not connected
//! yet`.
//!
//! Chains follow your arrows from your shapes, starting with those no arrow of yours points
//! into, in the order you drew them, the way the board summary follows edges. An arrow that
//! continues no chain is listed after, and a shape no arrow touches says where it sits beside
//! the nearest shape, yours or BoardAI's.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::board::{Element, ElementKind, ElementState, Geometry, Layer, UserMark};
use crate::geometry::Point;
use crate::ids::ElementId;
use crate::limits::DRAWING_DESCRIPTION_MAX_BYTES;
use crate::narration::tags::bounded;
use crate::protocol::NodeShape;

/// One arrow you drew, with what its ends connect to.
struct Arrow<'a> {
    element: &'a Element,
    from: Option<&'a ElementId>,
    to: Option<&'a ElementId>,
}

/// Your drawing on `layer`, or `None` when you drew nothing there.
#[must_use]
pub fn describe(layer: &Layer) -> Option<String> {
    let shapes = shapes_of(layer);
    let arrows = arrows_of(layer);
    if shapes.is_empty() && arrows.is_empty() {
        return None;
    }
    let touched = |id: &ElementId| {
        arrows
            .iter()
            .any(|arrow| arrow.from == Some(id) || arrow.to == Some(id))
    };
    let pointed_into = |id: &ElementId| {
        arrows
            .iter()
            .any(|arrow| arrow.to == Some(id) && arrow.from.is_some_and(|from| from != id))
    };
    let mut used = vec![false; arrows.len()];
    let mut visited: BTreeSet<&ElementId> = BTreeSet::new();
    let mut parts = Vec::new();
    let roots = shapes.iter().filter(|shape| !pointed_into(&shape.id));
    let rest = shapes.iter().filter(|shape| pointed_into(&shape.id));
    for shape in roots.chain(rest) {
        if touched(&shape.id) && visited.insert(&shape.id) {
            parts.push(chain_from(layer, shape, &arrows, &mut used, &mut visited));
        }
    }
    for (arrow, _) in arrows.iter().zip(&used).filter(|(_, used)| !**used) {
        parts.push(loose_arrow(arrow));
    }
    for shape in shapes.iter().filter(|shape| !touched(&shape.id)) {
        let mut part = token(shape);
        if let Some(place) = place(layer, shape) {
            let _ = write!(part, " {place}");
        }
        part.push_str(", not connected yet");
        parts.push(part);
    }
    Some(bounded(&parts.join("; "), DRAWING_DESCRIPTION_MAX_BYTES))
}

/// Your boxes and pen shapes on `layer`, in the order you drew them.
fn shapes_of(layer: &Layer) -> Vec<&Element> {
    layer
        .elements()
        .iter()
        .filter(|element| element.state != ElementState::Pending)
        .filter(|element| {
            matches!(
                element.kind,
                ElementKind::User {
                    mark: UserMark::Box | UserMark::Ink { .. },
                    ..
                }
            )
        })
        .collect()
}

/// Your arrows on `layer`, in the order you drew them.
fn arrows_of(layer: &Layer) -> Vec<Arrow<'_>> {
    layer
        .elements()
        .iter()
        .filter(|element| element.state != ElementState::Pending)
        .filter_map(|element| match &element.kind {
            ElementKind::User {
                mark: UserMark::Arrow { from, to },
                ..
            } => Some(Arrow {
                element,
                from: from.as_ref(),
                to: to.as_ref(),
            }),
            ElementKind::User { .. }
            | ElementKind::Node { .. }
            | ElementKind::Edge { .. }
            | ElementKind::Annotation { .. }
            | ElementKind::Trace
            | ElementKind::Participant { .. }
            | ElementKind::Message { .. }
            | ElementKind::Cells { .. }
            | ElementKind::Cell { .. } => None,
        })
        .collect()
}

/// Follows the first unused arrow out of each shape until the chain reaches a visited one.
fn chain_from<'a>(
    layer: &'a Layer,
    start: &'a Element,
    arrows: &[Arrow<'a>],
    used: &mut [bool],
    visited: &mut BTreeSet<&'a ElementId>,
) -> String {
    let mut chain = token(start);
    let mut current = &start.id;
    // Each step marks an arrow used, so the walk is bounded by the arrow count.
    for _ in 0..arrows.len() {
        let next = arrows.iter().enumerate().find(|(index, arrow)| {
            !used[*index]
                && arrow.from == Some(current)
                && arrow.to.is_some_and(|to| !visited.contains(to))
        });
        let Some((index, arrow)) = next else {
            break;
        };
        let Some(to) = arrow.to else {
            break;
        };
        used[index] = true;
        visited.insert(to);
        let _ = write!(chain, " {} ", head(arrow.element));
        match layer.element(to) {
            Some(element) => chain.push_str(&token(element)),
            None => chain.push_str(to.as_str()),
        }
        current = to;
    }
    chain
}

/// An arrow that continues no chain: between two shapes already named, or with a loose end.
fn loose_arrow(arrow: &Arrow<'_>) -> String {
    let id = arrow.element.id.as_str();
    let head = head(arrow.element);
    match (arrow.from, arrow.to) {
        (Some(from), Some(to)) => format!("{} {head} {}", from.as_str(), to.as_str()),
        (Some(from), None) => format!(
            "{id} {head} from {} pointing {}, to nothing yet",
            from.as_str(),
            pointing(arrow.element)
        ),
        (None, Some(to)) => format!("{id} {head} into {} from nothing", to.as_str()),
        (None, None) => format!(
            "{id} {head} pointing {}, joining nothing",
            pointing(arrow.element)
        ),
    }
}

/// `→`, or `-[reads]→` for an arrow you labelled.
fn head(arrow: &Element) -> String {
    if arrow.label.is_empty() {
        "→".to_owned()
    } else {
        format!("-[{}]→", arrow.label)
    }
}

/// A shape as named in the description: yours with its label and note, BoardAI's with its id
/// and label.
fn token(element: &Element) -> String {
    let mut token = element.id.as_str().to_owned();
    match &element.kind {
        ElementKind::User { mark, note } => {
            if element.label.is_empty() {
                token.push_str(match mark {
                    UserMark::Box => " (a box, no label)",
                    UserMark::Ink { .. } => " (a sketch, no label)",
                    UserMark::Arrow { .. } => " (an arrow)",
                });
            } else {
                let _ = write!(token, " \"{}\"", element.label);
            }
            if let Some(note) = note {
                let _ = write!(token, " ({note})");
            }
        }
        ElementKind::Node { .. }
        | ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Participant { .. }
        | ElementKind::Message { .. }
        | ElementKind::Cells { .. }
        | ElementKind::Cell { .. } => {
            if !element.label.is_empty() && !element.label.eq_ignore_ascii_case(&token) {
                let _ = write!(token, " \"{}\"", element.label);
            }
        }
    }
    token
}

/// Where `shape` sits: beside the nearest other shape, yours or a node of BoardAI's.
fn place(layer: &Layer, shape: &Element) -> Option<String> {
    let center = shape.geometry.bounds().center();
    let distance = |point: Point| {
        let (dx, dy) = (
            i64::from(point.x) - i64::from(center.x),
            i64::from(point.y) - i64::from(center.y),
        );
        dx * dx + dy * dy
    };
    let nearest = layer
        .elements()
        .iter()
        .filter(|other| other.id != shape.id && other.state != ElementState::Pending)
        .filter(|other| is_landmark(other))
        .min_by_key(|other| distance(other.geometry.bounds().center()))?;
    let other = nearest.geometry.bounds().center();
    let (dx, dy) = (center.x - other.x, center.y - other.y);
    let side = if dy.abs() >= dx.abs() {
        if dy > 0 { "under" } else { "above" }
    } else if dx > 0 {
        "right of"
    } else {
        "left of"
    };
    Some(format!("{side} {}", nearest.id.as_str()))
}

/// A shape worth placing another beside: a box or pen shape of yours, or a node, a participant
/// or a row of cells of BoardAI's.
fn is_landmark(element: &Element) -> bool {
    match &element.kind {
        ElementKind::User { mark, .. } => !matches!(mark, UserMark::Arrow { .. }),
        ElementKind::Node { shape, .. } => !matches!(shape, NodeShape::Group),
        ElementKind::Participant { .. } | ElementKind::Cells { .. } => true,
        ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Message { .. }
        | ElementKind::Cell { .. } => false,
    }
}

/// Which way an arrow points, in eight directions.
fn pointing(arrow: &Element) -> &'static str {
    let Geometry::Path(path) = &arrow.geometry else {
        return "nowhere";
    };
    let (dx, dy) = (path.end.x - path.start.x, path.end.y - path.start.y);
    // Within about 22° of an axis it points along it.
    let (across, down) = (dx.abs(), dy.abs());
    let horizontal = across > down.saturating_mul(5) / 2;
    let vertical = down > across.saturating_mul(5) / 2;
    match (horizontal, vertical, dx >= 0, dy >= 0) {
        (true, _, true, _) => "right",
        (true, _, false, _) => "left",
        (false, true, _, true) => "down",
        (false, true, _, false) => "up",
        (false, false, true, true) => "down and right",
        (false, false, true, false) => "up and right",
        (false, false, false, true) => "down and left",
        (false, false, false, false) => "up and left",
    }
}
