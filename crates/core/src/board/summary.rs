//! The compact text form of the board sent to the talker before a request (LLD §2.5):
//! `main: client(actor) "Browser" → cdn "CDN" → lb "Load balancer" → api×3`.
//!
//! Each visible layer (main and every open detour) is one line. Nodes follow their edges as
//! chains in the order they were drawn; edges that don't continue a chain are listed after. A row
//! of cells shows its values (`arr[3|1|_|4]`, cell `arr.2` empty), and a sequence diagram its
//! participants and messages in order. Shapes the user drew come last on their layer's line, as
//! `drawn by the user: u1 "Client" → u2 "API"` (artboard 13).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::board::{Board, Element, ElementKind, ElementState, Layer};
use crate::ids::ElementId;
use crate::limits::BOARD_SUMMARY_MAX_BYTES;
use crate::narration::tags::bounded;
use crate::protocol::{LineStyle, Mark, NodeShape};

#[must_use]
pub fn summary(board: &Board) -> String {
    let mut lines = Vec::new();
    let mut detour = 0;
    for layer in board.layers().iter().filter(|layer| layer.is_open()) {
        let body = layer_summary(layer);
        if layer.id().0 == 0 {
            lines.push(format!("main: {body}"));
        } else {
            detour += 1;
            lines.push(format!("detour {detour} \"{}\": {body}", layer.title()));
        }
    }
    bounded(&lines.join("\n"), BOARD_SUMMARY_MAX_BYTES)
}

fn visible(element: &Element) -> bool {
    element.state != ElementState::Pending
}

struct EdgeRef<'a> {
    from: &'a ElementId,
    to: &'a ElementId,
    label: &'a str,
    style: LineStyle,
}

fn layer_summary(layer: &Layer) -> String {
    let nodes: Vec<&Element> = layer
        .elements()
        .iter()
        .filter(|element| {
            visible(element)
                && matches!(
                    element.kind,
                    ElementKind::Node { .. } | ElementKind::Cells { .. }
                )
        })
        .collect();
    let edges: Vec<EdgeRef<'_>> = layer
        .elements()
        .iter()
        .filter(|element| visible(element))
        .filter_map(|element| match &element.kind {
            ElementKind::Edge { from, to, style } => Some(EdgeRef {
                from,
                to,
                label: &element.label,
                style: *style,
            }),
            ElementKind::Node { .. }
            | ElementKind::Annotation { .. }
            | ElementKind::Trace
            | ElementKind::Participant { .. }
            | ElementKind::Message { .. }
            | ElementKind::Cells { .. }
            | ElementKind::Cell { .. }
            | ElementKind::User { .. } => None,
        })
        .collect();
    let sequence = sequence(layer);
    // Your shapes come last, as their own description (artboard 13).
    let yours = super::user::describe(layer).map(|drawing| format!("drawn by the user: {drawing}"));
    if nodes.is_empty() && edges.is_empty() && sequence.is_none() && yours.is_none() {
        return "empty".to_owned();
    }
    let mut used = vec![false; edges.len()];
    let mut visited: BTreeSet<&ElementId> = BTreeSet::new();
    let mut parts = Vec::new();
    for node in &nodes {
        if visited.insert(&node.id) {
            parts.push(chain_from(layer, node, &edges, &mut used, &mut visited));
        }
    }
    for (edge, _) in edges.iter().zip(&used).filter(|(_, used)| !**used) {
        parts.push(format!(
            "{} {} {}",
            edge.from.as_str(),
            arrow(edge),
            edge.to.as_str()
        ));
    }
    parts.extend(sequence);
    let marks = marks(layer);
    if !marks.is_empty() {
        parts.push(format!("marks: {}", marks.join(", ")));
    }
    parts.extend(yours);
    parts.join("; ")
}

/// `sequence client, server: client → server "SYN", server ⇢ client "SYN-ACK"`.
fn sequence(layer: &Layer) -> Option<String> {
    let mut participants = Vec::new();
    let mut messages = Vec::new();
    for element in layer.elements().iter().filter(|element| visible(element)) {
        match &element.kind {
            ElementKind::Participant { .. } => participants.push(node_token(element)),
            ElementKind::Message {
                from, to, style, ..
            } => messages.push(format!(
                "{} {} {}",
                from.as_str(),
                arrow(&EdgeRef {
                    from,
                    to,
                    label: &element.label,
                    style: *style,
                }),
                to.as_str()
            )),
            ElementKind::Node { .. }
            | ElementKind::Edge { .. }
            | ElementKind::Annotation { .. }
            | ElementKind::Trace
            | ElementKind::Cells { .. }
            | ElementKind::Cell { .. }
            | ElementKind::User { .. } => {}
        }
    }
    if participants.is_empty() {
        return None;
    }
    let mut text = format!("sequence {}", participants.join(", "));
    if !messages.is_empty() {
        let _ = write!(text, ": {}", messages.join(", "));
    }
    Some(text)
}

/// A row of cells as its values, `_` for an empty cell.
fn cells_token(layer: &Layer, element: &Element, vertical: bool) -> String {
    let values: Vec<&str> = layer
        .elements()
        .iter()
        .filter(
            |cell| matches!(&cell.kind, ElementKind::Cell { strip, .. } if *strip == element.id),
        )
        .map(|cell| {
            if cell.label.is_empty() {
                "_"
            } else {
                cell.label.as_str()
            }
        })
        .collect();
    let mut token = format!("{}[{}]", element.id.as_str(), values.join("|"));
    if vertical {
        token.push_str("(column)");
    }
    if !element.label.is_empty() {
        let _ = write!(token, " \"{}\"", element.label);
    }
    token
}

/// Follows the first unused edge out of each node until the chain reaches a visited node.
fn chain_from<'a>(
    layer: &'a Layer,
    start: &'a Element,
    edges: &[EdgeRef<'a>],
    used: &mut [bool],
    visited: &mut BTreeSet<&'a ElementId>,
) -> String {
    let mut chain = match start.kind {
        ElementKind::Cells { vertical, .. } => cells_token(layer, start, vertical),
        ElementKind::Node { .. }
        | ElementKind::Edge { .. }
        | ElementKind::Annotation { .. }
        | ElementKind::Trace
        | ElementKind::Participant { .. }
        | ElementKind::Message { .. }
        | ElementKind::Cell { .. }
        | ElementKind::User { .. } => node_token(start),
    };
    let mut current = &start.id;
    // Each step marks an edge used, so the walk is bounded by the edge count.
    for _ in 0..edges.len() {
        let next = edges.iter().enumerate().find(|(index, edge)| {
            !used[*index] && edge.from == current && !visited.contains(edge.to)
        });
        let Some((index, edge)) = next else {
            break;
        };
        used[index] = true;
        visited.insert(edge.to);
        let _ = write!(chain, " {} ", arrow(edge));
        match layer.element(edge.to) {
            Some(element) => chain.push_str(&node_token(element)),
            None => chain.push_str(edge.to.as_str()),
        }
        current = edge.to;
    }
    chain
}

fn arrow(edge: &EdgeRef<'_>) -> String {
    let head = match edge.style {
        LineStyle::Solid => "→",
        LineStyle::Dashed => "⇢",
    };
    if edge.label.is_empty() {
        head.to_owned()
    } else {
        format!("-[{}]{head}", edge.label)
    }
}

fn node_token(element: &Element) -> String {
    let mut token = element.id.as_str().to_owned();
    if let ElementKind::Node {
        shape,
        stack,
        group,
        ..
    } = &element.kind
    {
        if *shape != NodeShape::Service {
            let _ = write!(token, "({})", shape_name(*shape));
        }
        if *stack > 1 {
            let _ = write!(token, "×{stack}");
        }
        if !element.label.eq_ignore_ascii_case(element.id.as_str()) {
            let _ = write!(token, " \"{}\"", element.label);
        }
        if let Some(group) = group {
            let _ = write!(token, " in {}", group.as_str());
        }
    }
    if matches!(element.kind, ElementKind::Participant { .. })
        && !element.label.eq_ignore_ascii_case(element.id.as_str())
    {
        let _ = write!(token, " \"{}\"", element.label);
    }
    match element.state {
        ElementState::Frozen {
            progress_percent, ..
        } => {
            let _ = write!(token, " [frozen at {progress_percent}%]");
        }
        ElementState::Struck => token.push_str(" [corrected]"),
        ElementState::Pending | ElementState::Drawing { .. } | ElementState::Drawn => {}
    }
    token
}

fn marks(layer: &Layer) -> Vec<String> {
    layer
        .elements()
        .iter()
        .filter(|element| visible(element))
        .filter_map(|element| match &element.kind {
            ElementKind::Annotation { target, mark, .. } => {
                let mut text = format!("{} on {}", mark_name(*mark), target.as_str());
                if !element.label.is_empty() {
                    let _ = write!(text, " \"{}\"", element.label);
                }
                Some(text)
            }
            ElementKind::Node { .. }
            | ElementKind::Edge { .. }
            | ElementKind::Trace
            | ElementKind::Participant { .. }
            | ElementKind::Message { .. }
            | ElementKind::Cells { .. }
            | ElementKind::Cell { .. }
            | ElementKind::User { .. } => None,
        })
        .collect()
}

#[must_use]
pub fn shape_name(shape: NodeShape) -> &'static str {
    match shape {
        NodeShape::Service => "service",
        NodeShape::Db => "db",
        NodeShape::Queue => "queue",
        NodeShape::Actor => "actor",
        NodeShape::Group => "group",
        NodeShape::Note => "note",
        NodeShape::Circle => "circle",
    }
}

fn mark_name(mark: Mark) -> &'static str {
    match mark {
        Mark::Circle => "circle",
        Mark::Underline => "underline",
        Mark::Arrow => "arrow",
        Mark::Bracket => "bracket",
        Mark::Highlight => "highlight",
    }
}
