//! The tags of a staged diagram (D-187): a `<diagram>` block's opening tag, the parts inside it,
//! `<reveal>` and `<detail>`. Tolerant as every tag is (D-120, D-181): an attribute the grammar
//! doesn't know is ignored, an unknown value falls back to its default, and only a part missing
//! what it can't do without is dropped.

use super::{
    Attributes, BadTag, DIRECTIONS, NODE_KINDS, STYLES, TRUE_VALUES, bad, bounded, cells, lenient,
    lookup,
};
use crate::ids::ElementId;
use crate::limits::{
    CELL_VALUE_MAX_BYTES, DIAGRAM_NOTE_MAX_BYTES, LABEL_MAX_BYTES, RECORD_FIELDS_MAX,
    TITLE_MAX_BYTES,
};
use crate::narration::diagram::{DiagramSpec, Flow, SpecItem, SpecPart};
use crate::narration::ops::{Direction, NarrationOp};
use crate::narration::parser::RawTag;
use crate::protocol::{LineStyle, NodeShape};

/// The tags that open a block and end at their closing tag.
pub const BLOCK_TAG_NAMES: &[&str] = &["diagram", "detail"];

/// The id a diagram gets when its tag names none.
pub const DEFAULT_DIAGRAM_ID: &str = "diagram";

/// How a container's parts stand, as the talker writes it.
const FLOWS: &[(&str, Flow)] = &[
    ("row", Flow::Row),
    ("column", Flow::Column),
    ("horizontal", Flow::Row),
    ("vertical", Flow::Column),
    ("across", Flow::Row),
    ("down", Flow::Column),
    ("stack", Flow::Column),
    ("side", Flow::Row),
];

/// An empty diagram, from its opening tag: its id, title and how its top-level parts stand.
#[must_use]
pub fn open(tag: &RawTag) -> DiagramSpec {
    let attributes = Attributes(&tag.attributes);
    let id = attributes.hint("id").unwrap_or_else(default_id);
    DiagramSpec {
        title: bounded(
            attributes
                .get("title")
                .or_else(|| attributes.get("label"))
                .unwrap_or(id.as_str()),
            TITLE_MAX_BYTES,
        ),
        id,
        flow: flow(&attributes),
        items: Vec::new(),
        dropped: 0,
    }
}

#[allow(
    clippy::expect_used,
    reason = "the default id is a constant made of letters only"
)]
fn default_id() -> ElementId {
    ElementId::parse(DEFAULT_DIAGRAM_ID).expect("a valid id")
}

/// `<detail of="arena" title="…">`'s target and title, when it names a target.
pub fn detail_target(tag: &RawTag) -> Result<(ElementId, Option<String>), BadTag> {
    let attributes = Attributes(&tag.attributes);
    let of = ["of", "target", "id", "on"]
        .iter()
        .find_map(|key| attributes.hint(key))
        .ok_or(bad("a detail needs what it drills into"))?;
    let title = attributes
        .get("title")
        .or_else(|| attributes.get("label"))
        .map(|title| bounded(title, TITLE_MAX_BYTES));
    Ok((of, title))
}

/// A staged diagram's tag read on its own, outside a block: a reveal, a self-closing detail
/// (a detail layer to open with nothing staged on it yet), or a block's tag that stages nothing
/// alone (an empty `<diagram/>`, a part outside its diagram).
pub(super) fn outside_block(tag: &RawTag) -> Result<NarrationOp, BadTag> {
    match tag.name.as_str() {
        "reveal" => reveal(&Attributes(&tag.attributes)),
        "detail" => {
            let (of, title) = detail_target(tag)?;
            Ok(NarrationOp::Detail {
                of,
                title,
                spec: None,
            })
        }
        "diagram" => Err(bad("an empty diagram")),
        _ => Err(bad("a diagram's part outside a diagram")),
    }
}

/// `<reveal id="arena" deep="true"/>`.
pub(super) fn reveal(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let target = ["id", "target", "of"]
        .iter()
        .find_map(|key| attributes.get(key))
        .ok_or(bad("a required attribute is missing"))?;
    Ok(NarrationOp::Reveal {
        target: ElementId::parse(target).ok_or(bad("invalid element id"))?,
        deep: switch(attributes, &["deep", "all", "whole"]),
    })
}

/// One tag inside a block, as a part of the diagram.
pub fn part(tag: &RawTag) -> Result<SpecItem, BadTag> {
    let attributes = Attributes(&tag.attributes);
    let parent = ["in", "parent", "inside"]
        .iter()
        .find_map(|key| attributes.hint(key));
    let part = match tag.name.as_str() {
        "node" => node(&attributes)?,
        "edge" => edge(&attributes)?,
        "cells" => strip(tag)?,
        "record" => record(&attributes)?,
        "note" => note(&attributes)?,
        _ => return Err(bad("not a part of a diagram")),
    };
    Ok(SpecItem { parent, part })
}

fn node(attributes: &Attributes<'_>) -> Result<SpecPart, BadTag> {
    let id = attributes.id("id")?;
    let label = bounded(
        attributes.get("label").unwrap_or(id.as_str()),
        LABEL_MAX_BYTES,
    );
    let note = attributes
        .get("note")
        .map(|note| bounded(note, LABEL_MAX_BYTES));
    let category = ["cat", "category", "kind_of", "tier"]
        .iter()
        .find_map(|key| attributes.get(key))
        .map(|name| bounded(name, LABEL_MAX_BYTES));
    let dashed = dashed(attributes);
    let shape = lenient(NODE_KINDS, attributes.get("kind"), NodeShape::Service);
    if shape == NodeShape::Group {
        return Ok(SpecPart::Container {
            id,
            label,
            note,
            category,
            dashed,
            flow: flow(attributes),
        });
    }
    let step = ["n", "step", "number"]
        .iter()
        .filter_map(|key| attributes.get(key))
        .find_map(|value| value.trim_end_matches('.').parse::<u8>().ok())
        .filter(|number| *number > 0);
    let new_row = attributes
        .get("dir")
        .is_some_and(|dir| lookup(DIRECTIONS, dir, "") == Ok(Direction::Below));
    Ok(SpecPart::Node {
        id,
        label,
        shape,
        note,
        step,
        category,
        dashed,
        new_row,
    })
}

fn edge(attributes: &Attributes<'_>) -> Result<SpecPart, BadTag> {
    let from = attributes.id("from")?;
    let to = attributes.id("to")?;
    if from == to {
        return Err(bad("an edge needs two different ends"));
    }
    Ok(SpecPart::Edge {
        id: attributes.hint("id"),
        from,
        to,
        label: attributes
            .get("label")
            .map(|label| bounded(label, LABEL_MAX_BYTES)),
        style: if dashed(attributes) {
            LineStyle::Dashed
        } else {
            lenient(STYLES, attributes.get("style"), LineStyle::Solid)
        },
    })
}

/// A row of cells, read as `cells` reads one outside a diagram, its empty cells made explicit.
fn strip(tag: &RawTag) -> Result<SpecPart, BadTag> {
    let NarrationOp::Cells(spec) = cells(&Attributes(&tag.attributes))? else {
        return Err(bad("not a row of cells"));
    };
    let mut values = spec.values;
    values.resize(usize::from(spec.count), String::new());
    Ok(SpecPart::Cells {
        id: spec.id,
        label: spec.label,
        values,
        index: spec.index,
        vertical: spec.vertical,
    })
}

fn record(attributes: &Attributes<'_>) -> Result<SpecPart, BadTag> {
    let id = attributes.id("id")?;
    let fields: Vec<String> = ["fields", "values", "members"]
        .iter()
        .find_map(|key| attributes.get(key))
        .ok_or(bad("a record needs its fields"))?
        .split('|')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .take(RECORD_FIELDS_MAX)
        .map(|field| bounded(field, CELL_VALUE_MAX_BYTES))
        .collect();
    if fields.is_empty() {
        return Err(bad("a record needs its fields"));
    }
    Ok(SpecPart::Record {
        label: bounded(
            attributes.get("label").unwrap_or(id.as_str()),
            LABEL_MAX_BYTES,
        ),
        id,
        fields,
    })
}

fn note(attributes: &Attributes<'_>) -> Result<SpecPart, BadTag> {
    let text = ["text", "label", "note"]
        .iter()
        .find_map(|key| attributes.get(key))
        .ok_or(bad("a note needs its text"))?;
    Ok(SpecPart::Note {
        id: attributes.hint("id"),
        text: bounded(text, DIAGRAM_NOTE_MAX_BYTES),
    })
}

/// `style="dashed"` (or dotted), or `dashed="true"`.
fn dashed(attributes: &Attributes<'_>) -> bool {
    lenient(STYLES, attributes.get("style"), LineStyle::Solid) == LineStyle::Dashed
        || switch(attributes, &["dashed", "dotted"])
}

fn flow(attributes: &Attributes<'_>) -> Option<Flow> {
    ["layout", "flow", "arrange"]
        .iter()
        .filter_map(|key| attributes.get(key))
        .find_map(|value| lookup(FLOWS, value, "unknown flow").ok())
}

/// A boolean attribute under any of `keys`, on when it is a true value.
fn switch(attributes: &Attributes<'_>, keys: &[&str]) -> bool {
    keys.iter()
        .filter_map(|key| attributes.get(key))
        .any(|value| TRUE_VALUES.iter().any(|on| on.eq_ignore_ascii_case(value)))
}

#[cfg(test)]
mod tests {
    use super::{detail_target, open, part, reveal};
    use crate::narration::diagram::{Flow, SpecPart};
    use crate::narration::ops::NarrationOp;
    use crate::narration::parser::RawTag;
    use crate::narration::tags::Attributes;
    use crate::protocol::{LineStyle, NodeShape};

    fn tag(name: &str, attributes: &[(&str, &str)]) -> RawTag {
        RawTag {
            name: name.to_owned(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            written: String::new(),
            self_closing: true,
        }
    }

    #[test]
    fn a_group_is_a_container_and_unknown_attributes_are_ignored() {
        let item = part(&tag(
            "node",
            &[
                ("id", "arena"),
                ("kind", "group"),
                ("label", "Arena"),
                ("cat", "RAM records"),
                ("style", "dashed"),
                ("color", "red"),
                ("layout", "column"),
            ],
        ))
        .expect("a container");
        assert_eq!(
            item.part,
            SpecPart::Container {
                id: crate::ids::ElementId::parse("arena").expect("id"),
                label: "Arena".into(),
                note: None,
                category: Some("RAM records".into()),
                dashed: true,
                flow: Some(Flow::Column),
            }
        );
    }

    #[test]
    fn steps_pills_records_notes_and_rows_parse_and_bad_parts_drop() {
        let step = part(&tag(
            "node",
            &[("id", "s1"), ("label", "round up"), ("n", "1.")],
        ));
        assert!(matches!(
            step.map(|item| item.part),
            Ok(SpecPart::Node {
                step: Some(1),
                shape: NodeShape::Service,
                ..
            })
        ));
        let pill = part(&tag(
            "node",
            &[("id", "free"), ("kind", "state"), ("in", "slots")],
        ));
        let pill = pill.expect("a pill");
        assert_eq!(pill.parent.as_ref().map(|id| id.as_str()), Some("slots"));
        assert!(matches!(
            pill.part,
            SpecPart::Node {
                shape: NodeShape::Pill,
                ..
            }
        ));
        let record = part(&tag(
            "record",
            &[("id", "a"), ("fields", "cfg| |free|live")],
        ));
        assert!(matches!(
            record.map(|item| item.part),
            Ok(SpecPart::Record { fields, .. }) if fields == ["cfg", "free", "live"]
        ));
        let row = part(&tag(
            "cells",
            &[("id", "c"), ("values", "64K|128K"), ("count", "4")],
        ));
        assert!(matches!(
            row.map(|item| item.part),
            Ok(SpecPart::Cells { values, .. }) if values.len() == 4
        ));
        assert!(
            part(&tag("note", &[("in", "arena")])).is_err(),
            "a note needs text"
        );
        assert!(
            part(&tag("record", &[("id", "a")])).is_err(),
            "a record needs fields"
        );
        assert!(part(&tag("message", &[("from", "a"), ("to", "b")])).is_err());
        let edge = part(&tag(
            "edge",
            &[("from", "a"), ("to", "b"), ("dashed", "yes")],
        ));
        assert!(matches!(
            edge.map(|item| item.part),
            Ok(SpecPart::Edge {
                style: LineStyle::Dashed,
                ..
            })
        ));
    }

    #[test]
    fn the_block_tags_read_their_ids_titles_and_switches() {
        let spec = open(&tag(
            "diagram",
            &[("title", "inf-alloc"), ("layout", "row")],
        ));
        assert_eq!(spec.id.as_str(), "diagram");
        assert_eq!(spec.title, "inf-alloc");
        assert_eq!(spec.flow, Some(Flow::Row));
        let (of, title) = detail_target(&tag("detail", &[("of", "arena")])).expect("a target");
        assert_eq!((of.as_str(), title), ("arena", None));
        assert!(detail_target(&tag("detail", &[("title", "x")])).is_err());
        let deep = reveal(&Attributes(&[
            ("id".to_owned(), "arena".to_owned()),
            ("deep".to_owned(), "true".to_owned()),
        ]));
        assert!(matches!(deep, Ok(NarrationOp::Reveal { deep: true, .. })));
    }
}
