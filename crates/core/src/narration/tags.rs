//! Tags to typed ops (LLD §2.3). A missing required attribute or a bad required value drops the
//! tag; attributes the grammar doesn't define are ignored, so a model adding `color="red"` still
//! draws. An optional attribute with a value the grammar doesn't know (`kind="database"`,
//! `style="dull"`) falls back to its default after a table of common synonyms, so a near miss
//! never costs the drawing (live runs with small models lost whole nodes to these).

use crate::ids::ElementId;
use crate::limits::{
    COMPARE_AXIS_MAX, LABEL_MAX_BYTES, NODE_STACK_MAX, PLAN_STEPS_MAX, RECAP_BULLETS_MAX,
    TITLE_MAX_BYTES, TRACE_HOPS_MAX,
};
use crate::narration::ops::{
    AxisRef, CloseTarget, Direction, LineRange, NarrationOp, Placement, StepRef,
};
use crate::narration::parser::RawTag;
use crate::protocol::{CellMark, Fill, LineStyle, Mark, NodeShape};

/// Every tag the grammar defines, in the order the system prompt documents them.
pub const TAG_NAMES: &[&str] = &[
    "plan", "step", "node", "edge", "annotate", "strike", "focus", "detour", "resume", "code",
    "line", "trace", "compare", "cell", "close", "recap",
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
];

/// Line styles, then a synonym. An unknown style is solid.
const STYLES: &[(&str, LineStyle)] = &[
    ("solid", LineStyle::Solid),
    ("dashed", LineStyle::Dashed),
    ("dotted", LineStyle::Dashed),
];

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
    let shape = lenient(NODE_KINDS, attributes.get("kind"), NodeShape::Service);
    let direction = lenient(DIRECTIONS, attributes.get("dir"), Direction::Right);
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
            Mark::Underline | Mark::Arrow | Mark::Bracket => Mark::Underline,
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
    Ok(NarrationOp::Annotate {
        target: attributes.id("target")?,
        mark: lenient(MARKS, attributes.get("mark"), Mark::Circle),
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
mod tests {
    use super::{TAG_NAMES, bounded, to_op};
    use crate::narration::ops::{AxisRef, Direction, NarrationOp, Placement};
    use crate::narration::parser::RawTag;
    use crate::protocol::{CellMark, Fill, LineStyle, Mark, NodeShape};

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
            (
                "cell",
                &[("option", "0"), ("criterion", "Speed"), ("mark", "yes")],
            ),
            ("close", &[("surface", "code")]),
            (
                "recap",
                &[("bullets", "Reads try Redis first|A miss reads Postgres")],
            ),
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
            ("edge", &[("from", "a")]),
            ("edge", &[("from", "a"), ("to", "a")]),
            ("step", &[("n", "0")]),
            ("line", &[("n", "x")]),
            ("code", &[("path", "a.go"), ("lines", "9-4")]),
            ("trace", &[("path", "only")]),
            ("cell", &[("row", "0"), ("col", "0")]),
            ("cell", &[("option", "Redis"), ("mark", "yes")]),
            ("close", &[("surface", "window")]),
            ("plan", &[("steps", " | ")]),
            ("annotate", &[("line", "0"), ("code", "x")]),
            ("recap", &[("bullets", "|")]),
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
                fill: None,
            })
        );
    }

    /// A near miss in an optional value keeps the drawing: synonyms map, unknown values default.
    #[test]
    fn unknown_optional_values_fall_back_to_their_defaults() {
        let shape = |kind: &str| match to_op(&tag("node", &[("id", "a"), ("kind", kind)])) {
            Ok(NarrationOp::Node { shape, .. }) => Some(shape),
            Ok(_) | Err(_) => None,
        };
        assert_eq!(shape("Database"), Some(NodeShape::Db));
        assert_eq!(shape("cache"), Some(NodeShape::Db));
        assert_eq!(shape("topic"), Some(NodeShape::Queue));
        assert_eq!(shape("browser"), Some(NodeShape::Actor));
        assert_eq!(shape("cluster"), Some(NodeShape::Group));
        assert_eq!(shape("spaceship"), Some(NodeShape::Service));
        let placed = to_op(&tag(
            "node",
            &[("id", "a"), ("near", "b"), ("dir", "diagonal")],
        ));
        assert!(matches!(
            placed,
            Ok(NarrationOp::Node {
                placement: Placement::Near {
                    direction: Direction::Right,
                    ..
                },
                ..
            })
        ));
        let edge = to_op(&tag(
            "edge",
            &[("from", "a"), ("to", "b"), ("style", "dull")],
        ));
        assert!(matches!(
            edge,
            Ok(NarrationOp::Edge {
                style: LineStyle::Solid,
                ..
            })
        ));
        let annotate = to_op(&tag("annotate", &[("target", "a"), ("mark", "zigzag")]));
        assert!(matches!(
            annotate,
            Ok(NarrationOp::Annotate {
                mark: Mark::Circle,
                ..
            })
        ));
        let code = to_op(&tag("annotate", &[("line", "4"), ("mark", "zigzag")]));
        assert!(matches!(
            code,
            Ok(NarrationOp::MarkCode {
                mark: Mark::Underline,
                ..
            })
        ));
    }

    #[test]
    fn node_tones_compare_titles_cells_and_code_marks() {
        let node = to_op(&tag(
            "node",
            &[("id", "q"), ("kind", "queue"), ("fill", "Blush")],
        ));
        assert!(matches!(
            node,
            Ok(NarrationOp::Node {
                fill: Some(Fill::Blush),
                ..
            })
        ));
        let unknown = to_op(&tag("node", &[("id", "q"), ("fill", "plaid")]));
        assert!(matches!(unknown, Ok(NarrationOp::Node { fill: None, .. })));
        let compare = to_op(&tag(
            "compare",
            &[
                ("title", "Raft vs Paxos"),
                ("options", "Raft|Paxos"),
                ("criteria", "Easy"),
            ],
        ));
        assert!(matches!(
            compare,
            Ok(NarrationOp::Compare { title: Some(ref title), .. }) if title == "Raft vs Paxos"
        ));
        let by_row = to_op(&tag(
            "cell",
            &[("row", "Easy"), ("col", "Raft"), ("mark", "no")],
        ));
        assert_eq!(
            by_row,
            Ok(NarrationOp::Cell {
                criterion: AxisRef::Name("Easy".into()),
                option: AxisRef::Name("Raft".into()),
                mark: CellMark::No,
            })
        );
        let underline = to_op(&tag(
            "annotate",
            &[("line", "43"), ("code", "s.cache.Get(")],
        ));
        assert_eq!(
            underline,
            Ok(NarrationOp::MarkCode {
                line: 43,
                code: Some("s.cache.Get(".into()),
                mark: Mark::Underline,
            })
        );
        let circle = to_op(&tag("annotate", &[("line", "44"), ("mark", "circle")]));
        assert_eq!(
            circle,
            Ok(NarrationOp::MarkCode {
                line: 44,
                code: None,
                mark: Mark::Circle,
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
