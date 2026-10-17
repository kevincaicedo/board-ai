//! A staged diagram in the talker's board summary (D-187): what is drawn and what is still
//! planned, so the talker reveals the rest by id and never speaks of a planned part as drawn:
//!
//! `diagram alloc "inf-alloc · memory allocation", staged (reveal its parts with <reveal id>):
//! arena "Arena" drawn; pool "AlignedPool" partly drawn, planned: io, give; slots "SlotState"
//! planned: free, leased, staged`.
//!
//! A diagram's notes, connectors, fields and cells are left out: they come with what they
//! belong to.

use std::fmt::Write as _;

use crate::board::Layer;
use crate::board::staged::StagedDiagram;
use crate::ids::ElementId;
use crate::protocol::StagedPart;

/// Whether `id` is a part (or a field or cell of one) of a diagram staged on `layer`.
#[must_use]
pub fn is_part(layer: &Layer, id: &ElementId) -> bool {
    layer
        .staged
        .diagrams()
        .iter()
        .any(|diagram| diagram.entry_of(id).is_some())
}

/// One line per staged diagram of the layer.
#[must_use]
pub fn describe(layer: &Layer) -> Vec<String> {
    layer
        .staged
        .diagrams()
        .iter()
        .map(|diagram| diagram_line(layer, diagram))
        .collect()
}

fn diagram_line(layer: &Layer, diagram: &StagedDiagram) -> String {
    let mut line = format!(
        "diagram {} \"{}\", staged (reveal its parts with <reveal id>):",
        diagram.id.as_str(),
        diagram.title
    );
    let tops = diagram
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.parent.is_none() && entry.ends.is_none());
    let mut first = true;
    for (top, entry) in tops {
        let _ = write!(line, "{} {}", if first { "" } else { ";" }, token(entry));
        first = false;
        let (drawn, planned) = inside(layer, diagram, top);
        let shown = drawn_now(layer, &entry.id);
        let _ = match (shown, drawn, planned.is_empty()) {
            (true, _, true) => write!(line, " drawn"),
            (false, 0, false) => write!(line, " planned: {}", planned.join(", ")),
            (_, _, false) => write!(line, " partly drawn, planned: {}", planned.join(", ")),
            (false, _, true) => write!(line, " planned"),
        };
    }
    line
}

/// `id "label"`, the label left out when it is the id.
fn token(entry: &crate::board::staged::Entry) -> String {
    let label = entry.draw.label().unwrap_or_default();
    if label.is_empty() || label.eq_ignore_ascii_case(entry.id.as_str()) {
        entry.id.as_str().to_owned()
    } else {
        format!("{} \"{label}\"", entry.id.as_str())
    }
}

fn drawn_now(layer: &Layer, id: &ElementId) -> bool {
    layer
        .element(id)
        .is_some_and(|element| element.state.is_shown())
}

/// What is inside top-level part `top`: how many of its parts are drawn, and the ids of those
/// still planned (notes left out; they come with their container).
fn inside(layer: &Layer, diagram: &StagedDiagram, top: usize) -> (usize, Vec<String>) {
    let mut drawn = 0;
    let mut planned = Vec::new();
    for (at, entry) in diagram.entries.iter().enumerate() {
        let note = matches!(entry.draw, StagedPart::Note { .. });
        if at == top || entry.ends.is_some() || note {
            continue;
        }
        if !diagram.ancestors(at).contains(&top) {
            continue;
        }
        if drawn_now(layer, &entry.id) {
            drawn += 1;
        } else {
            planned.push(entry.id.as_str().to_owned());
        }
    }
    (drawn, planned)
}
