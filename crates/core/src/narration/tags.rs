//! Tags to typed ops (LLD §2.3). A missing required attribute or a bad value drops the tag;
//! attributes the grammar doesn't define are ignored, so a model adding `color="red"` still
//! draws.

use crate::ids::ElementId;
use crate::limits::{
    COMPARE_AXIS_MAX, LABEL_MAX_BYTES, NODE_STACK_MAX, PLAN_STEPS_MAX, TITLE_MAX_BYTES,
    TRACE_HOPS_MAX,
};
use crate::narration::ops::{
    AxisRef, CloseTarget, Direction, LineRange, NarrationOp, Placement, StepRef,
};
use crate::narration::parser::RawTag;
use crate::protocol::{CellMark, LineStyle, Mark, NodeShape};

/// Every tag the grammar defines, in the order the system prompt documents them.
pub const TAG_NAMES: &[&str] = &[
    "plan", "step", "node", "edge", "annotate", "strike", "focus", "detour", "resume", "code",
    "line", "trace", "compare", "cell", "close",
];

/// Why a well-formed tag was dropped. For the log and golden files only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadTag {
    pub reason: &'static str,
}

fn bad(reason: &'static str) -> BadTag {
    BadTag { reason }
}

/// Node kinds, as the talker writes them.
const NODE_KINDS: &[(&str, NodeShape)] = &[
    ("service", NodeShape::Service),
    ("db", NodeShape::Db),
    ("queue", NodeShape::Queue),
    ("actor", NodeShape::Actor),
    ("group", NodeShape::Group),
    ("note", NodeShape::Note),
];

const DIRECTIONS: &[(&str, Direction)] = &[
    ("right", Direction::Right),
    ("left", Direction::Left),
    ("above", Direction::Above),
    ("below", Direction::Below),
];

const MARKS: &[(&str, Mark)] = &[
    ("circle", Mark::Circle),
    ("underline", Mark::Underline),
    ("arrow", Mark::Arrow),
    ("bracket", Mark::Bracket),
];

const STYLES: &[(&str, LineStyle)] = &[("solid", LineStyle::Solid), ("dashed", LineStyle::Dashed)];

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
        "compare" => Ok(NarrationOp::Compare {
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
        }),
        "cell" => cell(&attributes),
        "close" => Ok(NarrationOp::Close {
            surface: match attributes.get("surface") {
                None => CloseTarget::Latest,
                Some(value) => lookup(CLOSE_TARGETS, value, "unknown surface")?,
            },
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

    fn optional_id(&self, key: &'static str) -> Result<Option<ElementId>, BadTag> {
        self.get(key)
            .map(|value| ElementId::parse(value).ok_or(bad("invalid element id")))
            .transpose()
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
    let shape = match attributes.get("kind") {
        None => NodeShape::Service,
        Some(kind) => lookup(NODE_KINDS, kind, "unknown node kind")?,
    };
    let direction = match attributes.get("dir") {
        None => Direction::Right,
        Some(dir) => lookup(DIRECTIONS, dir, "unknown direction")?,
    };
    let near = attributes
        .optional_id("near")?
        .map(|target| (target, direction));
    let placement = match (attributes.optional_id("in")?, near) {
        (Some(group), near) => Placement::Inside { group, near },
        (None, Some((target, direction))) => Placement::Near { target, direction },
        (None, None) => Placement::Auto,
    };
    let stack = match attributes.get("stack") {
        None => 1,
        Some(value) => {
            let stack: u8 = value.parse().map_err(|_| bad("stack is not a number"))?;
            stack.clamp(1, NODE_STACK_MAX)
        }
    };
    Ok(NarrationOp::Node {
        id,
        label,
        shape,
        placement,
        stack,
    })
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
        style: match attributes.get("style") {
            None => LineStyle::Solid,
            Some(style) => lookup(STYLES, style, "unknown line style")?,
        },
    })
}

fn annotate(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    Ok(NarrationOp::Annotate {
        target: attributes.id("target")?,
        mark: match attributes.get("mark") {
            None => Mark::Circle,
            Some(mark) => lookup(MARKS, mark, "unknown mark")?,
        },
        text: attributes
            .get("text")
            .map(|text| bounded(text, LABEL_MAX_BYTES)),
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

fn cell(attributes: &Attributes<'_>) -> Result<NarrationOp, BadTag> {
    let row = axis(attributes.required("row")?);
    let col = axis(attributes.required("col")?);
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
    Ok(NarrationOp::Cell { row, col, mark })
}

#[cfg(test)]
mod tests {
    use super::{TAG_NAMES, bounded, to_op};
    use crate::narration::ops::{Direction, NarrationOp, Placement};
    use crate::narration::parser::RawTag;
    use crate::protocol::NodeShape;

    fn tag(name: &str, attributes: &[(&str, &str)]) -> RawTag {
        RawTag {
            name: name.to_owned(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            written: String::new(),
        }
    }

    #[test]
    fn a_minimal_valid_form_of_every_tag_parses() {
        let minimal: &[(&str, &[(&str, &str)])] = &[
            ("plan", &[("steps", "Client|Edge|LB")]),
            ("step", &[("n", "2")]),
            ("node", &[("id", "lb")]),
            ("edge", &[("from", "cdn"), ("to", "lb")]),
            ("annotate", &[("target", "lb")]),
            ("strike", &[("target", "sqs"), ("text", "Kafka")]),
            ("focus", &[("target", "lb")]),
            ("detour", &[("title", "cache-aside")]),
            ("resume", &[]),
            ("code", &[("path", "src/lb.rs"), ("lines", "4-9")]),
            ("line", &[("n", "42")]),
            ("trace", &[("path", "api>redis>db")]),
            (
                "compare",
                &[("options", "Redis|Memcached"), ("criteria", "Speed")],
            ),
            ("cell", &[("row", "0"), ("col", "Speed"), ("mark", "yes")]),
            ("close", &[("surface", "code")]),
        ];
        assert_eq!(minimal.len(), TAG_NAMES.len());
        for ((name, attributes), expected) in minimal.iter().zip(TAG_NAMES) {
            assert_eq!(name, expected, "the table follows TAG_NAMES");
            assert!(to_op(&tag(name, attributes)).is_ok(), "{name}");
        }
    }

    #[test]
    fn bad_values_and_missing_attributes_drop_the_tag() {
        let bad: &[(&str, &[(&str, &str)])] = &[
            ("node", &[("id", "has space")]),
            ("node", &[("id", "a"), ("kind", "spaceship")]),
            ("node", &[("id", "a"), ("near", "b"), ("dir", "diagonal")]),
            ("edge", &[("from", "a")]),
            ("edge", &[("from", "a"), ("to", "a")]),
            ("step", &[("n", "0")]),
            ("line", &[("n", "x")]),
            ("code", &[("path", "a.go"), ("lines", "9-4")]),
            ("trace", &[("path", "only")]),
            ("cell", &[("row", "0"), ("col", "0")]),
            ("close", &[("surface", "window")]),
            ("plan", &[("steps", " | ")]),
        ];
        for (name, attributes) in bad {
            assert!(
                to_op(&tag(name, attributes)).is_err(),
                "{name} {attributes:?}"
            );
        }
    }

    #[test]
    fn node_placement_and_defaults() {
        let op = to_op(&tag(
            "node",
            &[
                ("id", "api"),
                ("near", "lb"),
                ("stack", "3"),
                ("color", "red"),
            ],
        ));
        assert_eq!(
            op,
            Ok(NarrationOp::Node {
                id: crate::ids::ElementId::parse("api").expect("valid"),
                label: "api".to_owned(),
                shape: NodeShape::Service,
                placement: Placement::Near {
                    target: crate::ids::ElementId::parse("lb").expect("valid"),
                    direction: Direction::Right,
                },
                stack: 3,
            })
        );
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        let cut = bounded(&"é".repeat(50), 11);
        assert!(cut.len() <= 11, "{cut}");
        assert!(cut.ends_with('…'));
    }
}
