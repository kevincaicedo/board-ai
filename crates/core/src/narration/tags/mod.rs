//! Tags to typed ops (LLD §2.3). A missing required attribute or a bad required value drops the
//! tag; attributes the grammar doesn't define are ignored, so a model adding `color="red"` still
//! draws. An optional attribute with a value the grammar doesn't know (`kind="database"`,
//! `style="dull"`) falls back to its default after a table of common synonyms, so a near miss
//! never costs the drawing (live runs with small models lost whole nodes to these).

use crate::ids::ElementId;
use crate::limits::{
    CELL_VALUE_MAX_BYTES, CELLS_PER_STRIP_MAX, COMPARE_AXIS_MAX, LABEL_MAX_BYTES, NODE_STACK_MAX,
    PLAN_STEPS_MAX, RECAP_BULLETS_MAX, TITLE_MAX_BYTES, TRACE_HOPS_MAX,
};
use crate::narration::ops::{
    AxisRef, CellsSpec, CloseTarget, Direction, IndexLabels, LineRange, NarrationOp, Placement,
    Side, StepRef,
};
use crate::narration::parser::RawTag;
use crate::protocol::{CellMark, Fill, LineStyle, Mark, NodeShape};

/// Every tag the grammar defines, in the order the system prompt documents them.
pub const TAG_NAMES: &[&str] = &[
    "plan",
    "step",
    "node",
    "edge",
    "annotate",
    "strike",
    "focus",
    "detour",
    "resume",
    "code",
    "line",
    "trace",
    "compare",
    "cell",
    "close",
    "recap",
    "participant",
    "message",
    "cells",
    "write",
];

/// Why a well-formed tag was dropped. For the log and golden files only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadTag {
    pub reason: &'static str,
}

fn bad(reason: &'static str) -> BadTag {
    BadTag { reason }
}

/// Node kinds, as the talker writes them, then the synonyms models use for them. An unknown
/// kind draws a service.
const NODE_KINDS: &[(&str, NodeShape)] = &[
    ("service", NodeShape::Service),
    ("db", NodeShape::Db),
    ("queue", NodeShape::Queue),
    ("actor", NodeShape::Actor),
    ("group", NodeShape::Group),
    ("note", NodeShape::Note),
    ("database", NodeShape::Db),
    ("cache", NodeShape::Db),
    ("store", NodeShape::Db),
    ("storage", NodeShape::Db),
    ("datastore", NodeShape::Db),
    ("topic", NodeShape::Queue),
    ("stream", NodeShape::Queue),
    ("user", NodeShape::Actor),
    ("person", NodeShape::Actor),
    ("client", NodeShape::Actor),
    ("browser", NodeShape::Actor),
    ("boundary", NodeShape::Group),
    ("cluster", NodeShape::Group),
    ("vpc", NodeShape::Group),
    ("text", NodeShape::Note),
    ("circle", NodeShape::Circle),
    ("round", NodeShape::Circle),
    ("ellipse", NodeShape::Circle),
    ("oval", NodeShape::Circle),
    ("state", NodeShape::Circle),
];

/// A tree child's side, then synonyms. An unknown side lines children up instead.
const SIDES: &[(&str, Side)] = &[
    ("left", Side::Left),
    ("right", Side::Right),
    ("l", Side::Left),
    ("r", Side::Right),
];

/// Directions, then synonyms. An unknown direction is right.
const DIRECTIONS: &[(&str, Direction)] = &[
    ("right", Direction::Right),
    ("left", Direction::Left),
    ("above", Direction::Above),
    ("below", Direction::Below),
    ("up", Direction::Above),
    ("down", Direction::Below),
    ("top", Direction::Above),
    ("bottom", Direction::Below),
];

const MARKS: &[(&str, Mark)] = &[
    ("circle", Mark::Circle),
    ("underline", Mark::Underline),
    ("arrow", Mark::Arrow),
    ("bracket", Mark::Bracket),
    ("highlight", Mark::Highlight),
    ("fill", Mark::Highlight),
    ("shade", Mark::Highlight),
    ("hatch", Mark::Highlight),
];

/// Line styles, then a synonym. An unknown style is solid.
const STYLES: &[(&str, LineStyle)] = &[
    ("solid", LineStyle::Solid),
    ("dashed", LineStyle::Dashed),
    ("dotted", LineStyle::Dashed),
];

/// Message styles: an edge's, then what a sequence diagram calls them. An unknown one is a call.
const MESSAGE_STYLES: &[(&str, LineStyle)] = &[
    ("solid", LineStyle::Solid),
    ("dashed", LineStyle::Dashed),
    ("dotted", LineStyle::Dashed),
    ("call", LineStyle::Solid),
    ("request", LineStyle::Solid),
    ("sync", LineStyle::Solid),
    ("return", LineStyle::Dashed),
    ("reply", LineStyle::Dashed),
    ("response", LineStyle::Dashed),
];

/// Values that turn a boolean attribute on (`vertical="true"`); anything else is off.
const TRUE_VALUES: &[&str] = &["true", "yes", "1", "on", "column", "vertical", "down"];

/// `index` values that hide the index labels.
const NO_INDEX: &[&str] = &["none", "no", "off", "false", "hide", "hidden"];

/// The sketch tones a node may name.
const FILLS: &[(&str, Fill)] = &[
    ("lilac", Fill::Lilac),
    ("sky", Fill::Sky),
    ("mint", Fill::Mint),
    ("blush", Fill::Blush),
];

const CLOSE_TARGETS: &[(&str, CloseTarget)] = &[
    ("code", CloseTarget::Code),
    ("compare", CloseTarget::Compare),
    ("recap", CloseTarget::Recap),
    ("all", CloseTarget::All),
];

/// Converts a parsed tag into its op.
pub fn to_op(tag: &RawTag) -> Result<NarrationOp, BadTag> {
    let attributes = Attributes(&tag.attributes);
    match tag.name.as_str() {
        "plan" => plan(&attributes),
        "step" => step(&attributes),
        "node" => node(&attributes),
        "edge" => edge(&attributes),
        "annotate" => annotate(&attributes),
        "strike" => Ok(NarrationOp::Strike {
            target: attributes.id("target")?,
            text: bounded(attributes.required("text")?, LABEL_MAX_BYTES),
        }),
        "focus" => Ok(NarrationOp::Focus {
            target: attributes.id("target")?,
        }),
        "detour" => Ok(NarrationOp::Detour {
            title: bounded(attributes.get("title").unwrap_or("Aside"), TITLE_MAX_BYTES),
        }),
        "resume" => Ok(NarrationOp::Resume),
        "code" => code(&attributes),
        "line" => Ok(NarrationOp::Line {
            line: attributes.number("n")?,
        }),
        "trace" => trace(&attributes),
        "compare" => compare(&attributes),
        "cell" => cell(&attributes),
        "close" => Ok(NarrationOp::Close {
            surface: match attributes.get("surface") {
                None => CloseTarget::Latest,
                Some(value) => lookup(CLOSE_TARGETS, value, "unknown surface")?,
            },
        }),
        "recap" => Ok(NarrationOp::Recap {
            bullets: list(
                attributes.required("bullets")?,
                RECAP_BULLETS_MAX,
                LABEL_MAX_BYTES,
            )?,
        }),
        "participant" => {
            let id = attributes.id("id")?;
            Ok(NarrationOp::Participant {
                label: bounded(
                    attributes.get("label").unwrap_or(id.as_str()),
                    LABEL_MAX_BYTES,
                ),
                id,
            })
        }
        "message" => Ok(NarrationOp::Message {
            from: attributes.id("from")?,
            to: attributes.id("to")?,
            label: attributes
                .get("label")
                .map(|label| bounded(label, LABEL_MAX_BYTES)),
            style: lenient(MESSAGE_STYLES, attributes.get("style"), LineStyle::Solid),
        }),
        "cells" => cells(&attributes),
        "write" => Ok(NarrationOp::Write {
            target: attributes.id("target")?,
            text: bounded(
                attributes
                    .get("text")
                    .or_else(|| attributes.get("value"))
                    .ok_or(bad("a required attribute is missing"))?,
                LABEL_MAX_BYTES,
            ),
        }),
        _ => Err(bad("unknown tag")),
    }
}

struct Attributes<'a>(&'a [(String, String)]);

impl Attributes<'_> {
    /// The first value of `key`, trimmed; an empty value counts as missing.
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    }

    fn required(&self, key: &'static str) -> Result<&str, BadTag> {
        self.get(key).ok_or(bad("a required attribute is missing"))
    }

    fn id(&self, key: &'static str) -> Result<ElementId, BadTag> {
        ElementId::parse(self.required(key)?).ok_or(bad("invalid element id"))
    }

    /// A placement hint's id. A value that can't be an id (`near="crates/app"`) names nothing,
    /// so like any hint naming nothing it continues the flow instead of losing the drawing.
    fn hint(&self, key: &'static str) -> Option<ElementId> {
        self.get(key).and_then(ElementId::parse)
    }

    fn number(&self, key: &'static str) -> Result<u32, BadTag> {
        let number: u32 = self
            .required(key)?
            .parse()
            .map_err(|_| bad("not a number"))?;
        if number == 0 {
            return Err(bad("numbers start at 1"));
        }
        Ok(number)
    }
}

/// An optional attribute's value from its table, or `default` when it is absent or unknown.
fn lenient<T: Copy>(table: &[(&str, T)], value: Option<&str>, default: T) -> T {
    value
        .and_then(|value| lookup(table, value, "unknown value").ok())
        .unwrap_or(default)
}

fn lookup<T: Copy>(table: &[(&str, T)], value: &str, reason: &'static str) -> Result<T, BadTag> {
    table
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
        .map(|(_, item)| *item)
        .ok_or(bad(reason))
}

/// Cuts text to `max_bytes` on a char boundary, ending with "…" when it was cut.
#[must_use]
pub fn bounded(text: &str, max_bytes: usize) -> String {
    let text = text.trim();
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let ellipsis = '…';
    let mut end = max_bytes.saturating_sub(ellipsis.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut cut = text[..end].trim_end().to_owned();
    cut.push(ellipsis);
    cut
}

fn list(value: &str, max_items: usize, max_bytes: usize) -> Result<Vec<String>, BadTag> {
    let items: Vec<String> = value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .take(max_items)
        .map(|item| bounded(item, max_bytes))
        .collect();
    if items.is_empty() {
        return Err(bad("an empty list"));
    }
    Ok(items)
}

fn plan(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    Ok(NarrationOp::Plan {
        steps: list(
            attributes.required("steps")?,
            PLAN_STEPS_MAX,
            TITLE_MAX_BYTES,
        )?,
    })
}

fn step(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let value = attributes.required("n")?;
    let step = match value.parse::<u32>() {
        Ok(0) => return Err(bad("steps start at 1")),
        Ok(number) => StepRef::Number(u8::try_from(number).map_err(|_| bad("step too large"))?),
        Err(_) => StepRef::Name(bounded(value, TITLE_MAX_BYTES)),
    };
    Ok(NarrationOp::Step { step })
}

fn node(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let id = attributes.id("id")?;
    let label = bounded(
        attributes.get("label").unwrap_or(id.as_str()),
        LABEL_MAX_BYTES,
    );
    let shape = attributes
        .get("kind")
        .map(|kind| lenient(NODE_KINDS, Some(kind), NodeShape::Service));
    let placement = match attributes.hint("parent") {
        Some(parent) => Placement::Child {
            parent,
            side: attributes
                .get("side")
                .and_then(|side| lookup(SIDES, side, "unknown side").ok()),
        },
        None => placement(attributes)?,
    };
    let stack = match attributes.get("stack") {
        None => 1,
        Some(value) => {
            let stack: u8 = value.parse().map_err(|_| bad("stack is not a number"))?;
            stack.clamp(1, NODE_STACK_MAX)
        }
    };
    // An unknown tone is ignored rather than dropping the node: the board picks one.
    let fill = attributes
        .get("fill")
        .and_then(|fill| lookup(FILLS, fill, "unknown fill").ok());
    Ok(NarrationOp::Node {
        id,
        label,
        shape,
        placement,
        stack,
        fill,
    })
}

/// `in`, `near` and `dir`, as a node or a row of cells take them.
fn placement(attributes: &Attributes<'_>) -> Result<Placement, BadTag> {
    let direction = lenient(DIRECTIONS, attributes.get("dir"), Direction::Right);
    let near = attributes.hint("near").map(|target| (target, direction));
    Ok(match (attributes.hint("in"), near) {
        (Some(group), near) => Placement::Inside { group, near },
        (None, Some((target, direction))) => Placement::Near { target, direction },
        (None, None) => Placement::Auto,
    })
}

/// `values="3|1||4"` keeps empty cells; `count` adds empty ones; one of them is required.
fn cells(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let id = attributes.id("id")?;
    let values: Vec<String> = attributes
        .get("values")
        .map(|values| {
            values
                .split('|')
                .take(CELLS_PER_STRIP_MAX)
                .map(|value| bounded(value, CELL_VALUE_MAX_BYTES))
                .collect()
        })
        .unwrap_or_default();
    let count = match attributes.get("count") {
        Some(count) => {
            let count: usize = count.parse().map_err(|_| bad("count is not a number"))?;
            count.clamp(1, CELLS_PER_STRIP_MAX)
        }
        None if values.is_empty() => return Err(bad("cells need values or a count")),
        None => values.len(),
    };
    let count = u8::try_from(count.max(values.len())).map_err(|_| bad("too many cells"))?;
    let index = match attributes.get("index") {
        None => IndexLabels::From(0),
        Some(value) if NO_INDEX.iter().any(|no| no.eq_ignore_ascii_case(value)) => {
            IndexLabels::None
        }
        Some(value) => match value.parse::<u32>() {
            Ok(first) => IndexLabels::From(first),
            Err(_) => IndexLabels::Named(
                value
                    .split('|')
                    .take(CELLS_PER_STRIP_MAX)
                    .map(|label| bounded(label, CELL_VALUE_MAX_BYTES))
                    .collect(),
            ),
        },
    };
    let vertical = ["vertical", "layout", "orient"]
        .iter()
        .filter_map(|key| attributes.get(key))
        .any(|value| TRUE_VALUES.iter().any(|on| on.eq_ignore_ascii_case(value)));
    Ok(NarrationOp::Cells(CellsSpec {
        label: bounded(attributes.get("label").unwrap_or_default(), LABEL_MAX_BYTES),
        id,
        values,
        count,
        index,
        vertical,
        placement: placement(attributes)?,
    }))
}

fn edge(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let from = attributes.id("from")?;
    let to = attributes.id("to")?;
    if from == to {
        return Err(bad("an edge needs two different ends"));
    }
    Ok(NarrationOp::Edge {
        from,
        to,
        label: attributes
            .get("label")
            .map(|label| bounded(label, LABEL_MAX_BYTES)),
        style: lenient(STYLES, attributes.get("style"), LineStyle::Solid),
    })
}

/// `target` marks a board element; `line` (with `code`, the text to mark) marks the open code
/// card instead, underlined unless `mark` says circle.
fn annotate(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    if attributes.get("line").is_some() && attributes.get("target").is_none() {
        let mark = match lenient(MARKS, attributes.get("mark"), Mark::Underline) {
            Mark::Circle => Mark::Circle,
            // Only a ring or an underline suit a line of code; anything else underlines.
            Mark::Underline | Mark::Arrow | Mark::Bracket | Mark::Highlight => Mark::Underline,
        };
        return Ok(NarrationOp::MarkCode {
            line: attributes.number("line")?,
            code: attributes
                .get("code")
                .or_else(|| attributes.get("text"))
                .map(str::to_owned),
            mark,
        });
    }
    let mark = lenient(MARKS, attributes.get("mark"), Mark::Circle);
    // Only a highlight has a tone; a pen mark is always the annotation orange.
    let fill = match mark {
        Mark::Highlight => attributes
            .get("fill")
            .and_then(|fill| lookup(FILLS, fill, "unknown fill").ok()),
        Mark::Circle | Mark::Underline | Mark::Arrow | Mark::Bracket => None,
    };
    Ok(NarrationOp::Annotate {
        target: attributes.id("target")?,
        mark,
        text: attributes
            .get("text")
            .map(|text| bounded(text, LABEL_MAX_BYTES)),
        fill,
    })
}

fn code(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let path = attributes.required("path")?;
    let path = path.strip_prefix("./").unwrap_or(path).to_owned();
    let lines = attributes.get("lines").map(parse_lines).transpose()?;
    Ok(NarrationOp::Code { path, lines })
}

/// "42-52", "42–52" or "42".
fn parse_lines(value: &str) -> Result<LineRange, BadTag> {
    let mut parts = value.split(['-', '–']).map(str::trim);
    let first: u32 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or(bad("bad line range"))?;
    let last: u32 = match parts.next() {
        None => first,
        Some(part) => part.parse().map_err(|_| bad("bad line range"))?,
    };
    if parts.next().is_some() {
        return Err(bad("bad line range"));
    }
    LineRange::new(first, last).ok_or(bad("bad line range"))
}

fn trace(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let hops = attributes
        .required("path")?
        .split('>')
        .map(|hop| ElementId::parse(hop.trim()).ok_or(bad("invalid element id")))
        .collect::<Result<Vec<_>, _>>()?;
    if hops.len() < 2 || hops.len() > TRACE_HOPS_MAX {
        return Err(bad("a trace needs 2 to 16 hops"));
    }
    Ok(NarrationOp::Trace { hops })
}

fn axis(value: &str) -> AxisRef {
    match value.parse::<u8>() {
        Ok(index) => AxisRef::Index(index),
        Err(_) => AxisRef::Name(bounded(value, LABEL_MAX_BYTES)),
    }
}

fn compare(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    Ok(NarrationOp::Compare {
        options: list(
            attributes.required("options")?,
            COMPARE_AXIS_MAX,
            LABEL_MAX_BYTES,
        )?,
        criteria: list(
            attributes.required("criteria")?,
            COMPARE_AXIS_MAX,
            LABEL_MAX_BYTES,
        )?,
        title: attributes
            .get("title")
            .map(|title| bounded(title, TITLE_MAX_BYTES)),
    })
}

/// `option` and `criterion` name the cell; `col` and `row` are accepted for them, since rows are
/// criteria and columns are options on the card.
fn cell(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let missing = || bad("a required attribute is missing");
    let criterion = axis(
        attributes
            .get("criterion")
            .or_else(|| attributes.get("row"))
            .ok_or_else(missing)?,
    );
    let option = axis(
        attributes
            .get("option")
            .or_else(|| attributes.get("col"))
            .ok_or_else(missing)?,
    );
    let text = attributes.get("text");
    let mark = match (attributes.get("mark").map(str::to_ascii_lowercase), text) {
        (Some(mark), _) if mark == "yes" => CellMark::Yes,
        (Some(mark), _) if mark == "no" => CellMark::No,
        (Some(mark), _) if mark == "depends" => CellMark::Depends,
        (_, Some(text)) => CellMark::Text {
            text: bounded(text, LABEL_MAX_BYTES),
        },
        (Some(_) | None, None) => return Err(bad("a cell needs a mark or text")),
    };
    Ok(NarrationOp::Cell {
        criterion,
        option,
        mark,
    })
}

#[cfg(test)]
mod tests;
