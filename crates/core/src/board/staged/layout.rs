//! A staged diagram's global layout (D-187): every part placed at once, before the first stroke,
//! so nothing drawn ever has to move (I6).
//!
//! Containers are sized from their content, innermost first; then everything is placed from the
//! outside in. Inside a container the parts stand in lines: a run of boxes left to right
//! (wrapping at the container's width), containers side by side as sections (canvas 15's four
//! allocators, as tall as the tallest), notes and rows of cells each on their own line, and the
//! records along the container's bottom. The legend stands at the diagram's top right. Connectors
//! are routed last, around the boxes and containers they don't belong to.
//!
//! Deterministic and bounded: each pass is a loop over the parts, there is no recursion, and
//! nothing depends on anything but the spec.

use crate::board::metrics::text_width;
use crate::board::staged::metrics::{
    CELL_HEIGHT, CELLS_INDEX_SPACE, CELLS_LABEL_SPACE, CONTENT_WIDTH_MAX, CONTENT_WIDTH_MIN,
    DB_EXTRA_HEIGHT, EDGE_LABEL_FONT_SIZE, FIELD_HEIGHT, FRAME_HEADER_ONE_LINE,
    FRAME_HEADER_TWO_LINES, FRAME_LABEL_CENTER, FRAME_LABEL_FONT_SIZE, FRAME_NOTE_GAP,
    FRAME_NOTE_LINE_CENTER, FRAME_PADDING, INLINE_GAP, LABEL_FONT_SIZE, LEGEND_FONT_SIZE,
    LEGEND_GAP, LEGEND_MARGIN, LEGEND_ROW_HEIGHT, LEGEND_SWATCH_WIDTH, NOTE_FONT_SIZE,
    NOTE_LINE_HEIGHT, RECORD_LABEL_SPACE, ROW_WIDTH_MAX, SECTION_GAP, STACK_GAP, field_width,
    node_size,
};
use crate::board::staged::structure::{Part, PartKind, Structure};
use crate::board::staged::text::wrap;
use crate::geometry::{Point, Rect};
use crate::limits::DIAGRAM_NESTING_MAX;
use crate::narration::diagram::Flow;
use crate::narration::ops::IndexLabels;
use crate::protocol::{EdgePath, NodeShape, TextAt};

/// The space a labelled connector between two boxes side by side keeps beside its label.
const LABEL_GAP_PADDING: u32 = 16;

/// Every part's geometry, relative to the diagram's top-left corner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Laid {
    /// Each part's box, in `Structure::parts` order.
    pub boxes: Vec<Rect>,
    pub details: Vec<Detail>,
    /// Each connector's path and label, in `Structure::edges` order.
    pub edges: Vec<(EdgePath, Option<TextAt>)>,
    /// Each legend entry's box, in `Structure::categories` order.
    pub legend: Vec<Rect>,
    pub width: u32,
    pub height: u32,
}

/// What a part needs besides its box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Detail {
    Frame {
        label: TextAt,
        note: Option<TextAt>,
    },
    Box,
    Note {
        lines: Vec<String>,
    },
    /// The field boxes, left to right.
    Record {
        fields: Vec<Rect>,
    },
    /// The cell boxes, from the first.
    Cells {
        cells: Vec<Rect>,
    },
}

/// One container's content, as offsets from its content's top-left corner.
#[derive(Clone, Debug, Default)]
struct Content {
    width: u32,
    height: u32,
    placed: Vec<(usize, Point)>,
    /// The records along the bottom, as offsets from the bottom row's top-left corner.
    bottom: Vec<(usize, Point)>,
    bottom_height: u32,
}

/// What a frame's header takes: its height and where its words go, from the frame's corner.
#[derive(Clone, Debug)]
struct Header {
    height: i32,
    label: Point,
    note: Option<Point>,
}

struct Ctx<'a> {
    structure: &'a Structure,
    children: Vec<Vec<usize>>,
    sizes: Vec<(u32, u32)>,
    lines: Vec<Vec<String>>,
    contents: Vec<Content>,
    headers: Vec<Option<Header>>,
}

pub(super) fn lay_out(structure: &Structure, flow: Option<Flow>) -> Laid {
    let count = structure.parts.len();
    let mut children = vec![Vec::new(); count + 1];
    for (at, part) in structure.parts.iter().enumerate() {
        children[part.parent.unwrap_or(count)].push(at);
    }
    let mut ctx = Ctx {
        structure,
        children,
        sizes: structure.parts.iter().map(leaf_size).collect(),
        lines: vec![Vec::new(); count],
        contents: vec![Content::default(); count + 1],
        headers: vec![None; count],
    };
    for container in by_depth(structure, true) {
        size_container(&mut ctx, container);
    }
    let root = content(&mut ctx, count, flow);
    let (root_width, root_height) = (root.width, root.height);
    ctx.contents[count] = root;
    // The legend is a row above the diagram's top-left corner: drawn with the first part, it
    // stays in view while the first section is explained.
    let legend_space = if structure.categories.is_empty() {
        0
    } else {
        LEGEND_ROW_HEIGHT + u32::try_from(LEGEND_MARGIN).unwrap_or(0)
    };
    let top = Point {
        x: 0,
        y: i32::try_from(legend_space).unwrap_or(0),
    };
    let origins = place(&ctx, root_height, top);
    finish(&ctx, &origins, root_width, root_height + legend_space)
}

/// Containers, innermost first (`deepest_first`) or outermost first.
fn by_depth(structure: &Structure, deepest_first: bool) -> Vec<usize> {
    let depth = |at: usize| {
        let mut levels = 0;
        let mut cursor = Some(at);
        while let Some(part) = cursor {
            levels += 1;
            if levels > DIAGRAM_NESTING_MAX + 1 {
                break;
            }
            cursor = structure.parts[part].parent;
        }
        levels
    };
    let mut containers: Vec<(usize, usize)> = structure
        .parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.is_container())
        .map(|(at, _)| (depth(at), at))
        .collect();
    containers.sort_by_key(|(depth, at)| {
        if deepest_first {
            (usize::MAX - depth, *at)
        } else {
            (*depth, *at)
        }
    });
    containers.into_iter().map(|(_, at)| at).collect()
}

/// A part's size before its container is laid out; a container's and a note's come later.
fn leaf_size(part: &Part) -> (u32, u32) {
    match &part.kind {
        PartKind::Container { .. } | PartKind::Note { .. } => (0, 0),
        PartKind::Node {
            label,
            shape,
            note,
            step,
            ..
        } => {
            let (width, height) =
                node_size(label, note.as_deref(), *step, *shape == NodeShape::Pill);
            match shape {
                NodeShape::Db => (width, height + DB_EXTRA_HEIGHT),
                NodeShape::Circle => (width.max(height), height.max(width.min(height + 12))),
                NodeShape::Actor => (width.max(40), height + 24),
                NodeShape::Service
                | NodeShape::Queue
                | NodeShape::Group
                | NodeShape::Note
                | NodeShape::Pill => (width, height),
            }
        }
        PartKind::Record { label, fields } => {
            let strip: u32 = fields.iter().map(|field| field_width(field)).sum();
            (
                strip.max(text_width(label, LABEL_FONT_SIZE)),
                FIELD_HEIGHT + RECORD_LABEL_SPACE,
            )
        }
        PartKind::Cells {
            label,
            values,
            index,
            vertical,
        } => cells_size(label, values, index, *vertical),
    }
}

fn cells_size(label: &str, values: &[String], index: &IndexLabels, vertical: bool) -> (u32, u32) {
    let cell = values
        .iter()
        .map(|value| field_width(value))
        .max()
        .unwrap_or(0);
    let count = u32::try_from(values.len()).unwrap_or(0);
    let label_space = if label.is_empty() {
        0
    } else {
        CELLS_LABEL_SPACE
    };
    let label_width = text_width(label, NOTE_FONT_SIZE);
    if vertical {
        return (cell.max(label_width), label_space + count * CELL_HEIGHT);
    }
    let index_space = match index {
        IndexLabels::None => 0,
        IndexLabels::From(_) | IndexLabels::Named(_) => CELLS_INDEX_SPACE,
    };
    (
        (cell * count).max(label_width),
        label_space + CELL_HEIGHT + index_space,
    )
}

/// Sizes a container from its content, and its header.
fn size_container(ctx: &mut Ctx<'_>, at: usize) {
    let PartKind::Container {
        label, note, flow, ..
    } = &ctx.structure.parts[at].kind
    else {
        return;
    };
    let content = content(ctx, at, *flow);
    let padding = u32::try_from(FRAME_PADDING * 2).unwrap_or(0);
    let label_width = text_width(label, FRAME_LABEL_FONT_SIZE);
    let mut width = content.width.max(label_width) + padding;
    let header = match note {
        None => Header {
            height: FRAME_HEADER_ONE_LINE,
            label: Point {
                x: FRAME_PADDING,
                y: FRAME_LABEL_CENTER,
            },
            note: None,
        },
        Some(note) => {
            let note_width = text_width(note, NOTE_FONT_SIZE);
            let gap = u32::try_from(FRAME_NOTE_GAP).unwrap_or(0);
            let inline = label_width + gap + note_width;
            if inline <= content.width.max(CONTENT_WIDTH_MIN) {
                width = width.max(inline + padding);
                let after =
                    FRAME_PADDING + i32::try_from(label_width).unwrap_or(0) + FRAME_NOTE_GAP;
                Header {
                    height: FRAME_HEADER_ONE_LINE,
                    label: Point {
                        x: FRAME_PADDING,
                        y: FRAME_LABEL_CENTER,
                    },
                    note: Some(Point {
                        x: after,
                        y: FRAME_LABEL_CENTER,
                    }),
                }
            } else {
                width = width.max(note_width.min(CONTENT_WIDTH_MAX) + padding);
                Header {
                    height: FRAME_HEADER_TWO_LINES,
                    label: Point {
                        x: FRAME_PADDING,
                        y: FRAME_LABEL_CENTER,
                    },
                    note: Some(Point {
                        x: FRAME_PADDING,
                        y: FRAME_NOTE_LINE_CENTER,
                    }),
                }
            }
        }
    };
    let content_height = content.height + u32::try_from(FRAME_PADDING).unwrap_or(0);
    let height = u32::try_from(header.height).unwrap_or(0) + content_height;
    ctx.sizes[at] = (width, height);
    ctx.headers[at] = Some(header);
    ctx.contents[at] = content;
}

/// How a line of parts packs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pack {
    /// Boxes left to right, centred on the line, wrapping at the container's width.
    Run,
    /// Containers or anything else side by side, top-aligned, wrapping at the row limit; the
    /// containers stretch to the line's height.
    Row,
    /// One part alone.
    Single,
}

/// Lays out what container `at` holds (`at` = the parts' count for the diagram itself).
fn content(ctx: &mut Ctx<'_>, at: usize, flow: Option<Flow>) -> Content {
    let kids = ctx.children[at].clone();
    // The records written last stand along the container's bottom (canvas 15's structs); a
    // record written among other parts stays where it was written.
    let trailing = kids
        .iter()
        .rev()
        .take_while(|&&kid| matches!(ctx.structure.parts[kid].kind, PartKind::Record { .. }))
        .count();
    let (rest, records) = kids.split_at(kids.len() - trailing);
    let (rest, records) = (rest.to_vec(), records.to_vec());
    let widest = kids
        .iter()
        .filter(|&&kid| !matches!(ctx.structure.parts[kid].kind, PartKind::Note { .. }))
        .map(|&kid| ctx.sizes[kid].0)
        .max()
        .unwrap_or(0);
    let wrap_width = widest.clamp(CONTENT_WIDTH_MIN, CONTENT_WIDTH_MAX);
    for &kid in &rest {
        if let PartKind::Note { text } = &ctx.structure.parts[kid].kind {
            let lines = wrap(text, wrap_width, NOTE_FONT_SIZE);
            let width = lines
                .iter()
                .map(|line| text_width(line, NOTE_FONT_SIZE))
                .max();
            let height = NOTE_LINE_HEIGHT * u32::try_from(lines.len()).unwrap_or(0);
            ctx.sizes[kid] = (width.unwrap_or(0), height);
            ctx.lines[kid] = lines;
        }
    }
    let mut placed = Vec::new();
    let mut y = 0_u32;
    let mut width = 0_u32;
    for (pack, group) in groups(ctx, &rest, flow) {
        // A run of boxes is a flow read left to right: it wraps only past the widest content.
        let limit = match pack {
            Pack::Row => ROW_WIDTH_MAX,
            Pack::Run => CONTENT_WIDTH_MAX,
            Pack::Single => wrap_width,
        };
        if !placed.is_empty() {
            y += u32::try_from(STACK_GAP).unwrap_or(0);
        }
        let (used_width, used_height) = pack_lines(ctx, &group, pack, limit, y, &mut placed);
        width = width.max(used_width);
        y += used_height;
    }
    let mut bottom = Vec::new();
    let (bottom_width, bottom_height) =
        pack_lines(ctx, &records, Pack::Row, wrap_width, 0, &mut bottom);
    let height = match (placed.is_empty(), bottom.is_empty()) {
        (_, true) => y,
        (true, false) => bottom_height,
        (false, false) => y + u32::try_from(STACK_GAP).unwrap_or(0) + bottom_height,
    };
    Content {
        width: width.max(bottom_width),
        height,
        placed,
        bottom,
        bottom_height,
    }
}

/// The container's parts in lines: runs of boxes, rows of containers, and the rest alone; a
/// `row` flow puts everything side by side, a `column` flow everything alone.
fn groups(ctx: &Ctx<'_>, kids: &[usize], flow: Option<Flow>) -> Vec<(Pack, Vec<usize>)> {
    if flow == Some(Flow::Row) {
        return if kids.is_empty() {
            Vec::new()
        } else {
            vec![(Pack::Row, kids.to_vec())]
        };
    }
    let mut groups: Vec<(Pack, Vec<usize>)> = Vec::new();
    for &kid in kids {
        let pack = match (&ctx.structure.parts[kid].kind, flow) {
            (_, Some(Flow::Column)) => Pack::Single,
            (PartKind::Node { .. }, _) => Pack::Run,
            (PartKind::Container { .. }, _) => Pack::Row,
            (PartKind::Note { .. } | PartKind::Record { .. } | PartKind::Cells { .. }, _) => {
                Pack::Single
            }
        };
        let new_row = matches!(
            ctx.structure.parts[kid].kind,
            PartKind::Node { new_row: true, .. }
        );
        match groups.last_mut() {
            Some((last, group)) if *last == pack && pack != Pack::Single && !new_row => {
                group.push(kid);
            }
            Some(_) | None => groups.push((pack, vec![kid])),
        }
    }
    groups
}

/// Packs `kids` into lines from `top`, wrapping past `limit`; returns the width and height
/// used.
fn pack_lines(
    ctx: &mut Ctx<'_>,
    kids: &[usize],
    pack: Pack,
    limit: u32,
    top: u32,
    placed: &mut Vec<(usize, Point)>,
) -> (u32, u32) {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut line_width = 0_u32;
    for (index, &kid) in kids.iter().enumerate() {
        let gap = kids
            .get(index.wrapping_sub(1))
            .map_or(0, |&previous| gap_between(ctx, previous, kid, pack));
        let width = ctx.sizes[kid].0;
        match lines.last_mut() {
            Some(line) if line_width + gap + width <= limit => {
                line.push(kid);
                line_width += gap + width;
            }
            Some(_) | None => {
                lines.push(vec![kid]);
                line_width = width;
            }
        }
    }
    let (mut y, mut widest) = (top, 0_u32);
    for (number, line) in lines.iter().enumerate() {
        if number > 0 {
            y += u32::try_from(STACK_GAP).unwrap_or(0);
        }
        let height = line.iter().map(|&kid| ctx.sizes[kid].1).max().unwrap_or(0);
        let mut x = 0_u32;
        for (index, &kid) in line.iter().enumerate() {
            if index > 0 {
                x += gap_between(ctx, line[index - 1], kid, pack);
            }
            let (width, own_height) = ctx.sizes[kid];
            let dy = match pack {
                Pack::Run | Pack::Single => (height - own_height) / 2,
                Pack::Row => 0,
            };
            if pack == Pack::Row && ctx.structure.parts[kid].is_container() {
                ctx.sizes[kid].1 = height;
            }
            placed.push((kid, point(x, y + dy)));
            x += width;
        }
        widest = widest.max(x);
        y += height;
    }
    (widest, y - top)
}

/// The space between two parts side by side: room for a connector between boxes, and for its
/// label when it has one; the section gap between containers.
fn gap_between(ctx: &Ctx<'_>, left: usize, right: usize, pack: Pack) -> u32 {
    let both_containers =
        ctx.structure.parts[left].is_container() && ctx.structure.parts[right].is_container();
    if pack == Pack::Row && both_containers {
        return u32::try_from(SECTION_GAP).unwrap_or(0);
    }
    let label = ctx
        .structure
        .edges
        .iter()
        .filter(|edge| {
            (edge.from_part, edge.to_part) == (left, right)
                || (edge.from_part, edge.to_part) == (right, left)
        })
        .filter_map(|edge| edge.label.as_deref())
        .map(|label| text_width(label, EDGE_LABEL_FONT_SIZE) + LABEL_GAP_PADDING)
        .max()
        .unwrap_or(0);
    label.max(u32::try_from(INLINE_GAP).unwrap_or(0))
}

fn point(x: u32, y: u32) -> Point {
    Point {
        x: i32::try_from(x).unwrap_or(i32::MAX),
        y: i32::try_from(y).unwrap_or(i32::MAX),
    }
}

fn offset(origin: Point, by: Point) -> Point {
    Point {
        x: origin.x + by.x,
        y: origin.y + by.y,
    }
}

/// Every part's top-left corner, from the outside in.
fn place(ctx: &Ctx<'_>, root_height: u32, top: Point) -> Vec<Point> {
    let count = ctx.structure.parts.len();
    let mut origins = vec![Point::default(); count];
    let root = &ctx.contents[count];
    let root_bottom = i32::try_from(root_height.saturating_sub(root.bottom_height)).unwrap_or(0);
    place_content(&mut origins, root, top, top.y + root_bottom);
    for container in by_depth(ctx.structure, false) {
        let Some(header) = &ctx.headers[container] else {
            continue;
        };
        let corner = origins[container];
        let content = &ctx.contents[container];
        let inner = offset(
            corner,
            Point {
                x: FRAME_PADDING,
                y: header.height,
            },
        );
        let height = i32::try_from(ctx.sizes[container].1).unwrap_or(0);
        let bottom_height = i32::try_from(content.bottom_height).unwrap_or(0);
        let bottom_top = corner.y + height - FRAME_PADDING - bottom_height;
        place_content(&mut origins, content, inner, bottom_top);
    }
    origins
}

fn place_content(origins: &mut [Point], content: &Content, inner: Point, bottom_top: i32) {
    for (kid, at) in &content.placed {
        origins[*kid] = offset(inner, *at);
    }
    let bottom = Point {
        x: inner.x,
        y: bottom_top,
    };
    for (kid, at) in &content.bottom {
        origins[*kid] = offset(bottom, *at);
    }
}

/// The parts' boxes and details, the connectors, and the legend.
fn finish(ctx: &Ctx<'_>, origins: &[Point], width: u32, height: u32) -> Laid {
    let parts = &ctx.structure.parts;
    let boxes: Vec<Rect> = (0..parts.len())
        .map(|at| Rect {
            x: origins[at].x,
            y: origins[at].y,
            w: ctx.sizes[at].0,
            h: ctx.sizes[at].1,
        })
        .collect();
    let details: Vec<Detail> = (0..parts.len())
        .map(|at| detail(ctx, at, boxes[at]))
        .collect();
    let edges = super::connect::route_edges(ctx.structure, &boxes, &details);
    let mut left = 0;
    let legend: Vec<Rect> = ctx
        .structure
        .categories
        .iter()
        .map(|(_, name)| {
            let width = LEGEND_SWATCH_WIDTH + LEGEND_GAP + text_width(name, LEGEND_FONT_SIZE);
            let entry = Rect {
                x: left,
                y: 0,
                w: width,
                h: LEGEND_ROW_HEIGHT,
            };
            left += i32::try_from(width).unwrap_or(0) + LEGEND_MARGIN;
            entry
        })
        .collect();
    let all = legend.iter().copied().fold(
        Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        Rect::union,
    );
    Laid {
        boxes,
        details,
        edges,
        legend,
        width: all.w,
        height: all.h,
    }
}

fn detail(ctx: &Ctx<'_>, at: usize, rect: Rect) -> Detail {
    let corner = Point {
        x: rect.x,
        y: rect.y,
    };
    match &ctx.structure.parts[at].kind {
        PartKind::Container { label, note, .. } => {
            let Some(header) = &ctx.headers[at] else {
                return Detail::Box;
            };
            Detail::Frame {
                label: TextAt {
                    text: label.clone(),
                    at: offset(corner, header.label),
                },
                note: note.clone().zip(header.note).map(|(text, at)| TextAt {
                    text,
                    at: offset(corner, at),
                }),
            }
        }
        PartKind::Node { .. } => Detail::Box,
        PartKind::Note { .. } => Detail::Note {
            lines: ctx.lines[at].clone(),
        },
        PartKind::Record { fields, .. } => {
            let mut x = rect.x;
            Detail::Record {
                fields: fields
                    .iter()
                    .map(|field| {
                        let width = field_width(field);
                        let field = Rect {
                            x,
                            y: rect.y,
                            w: width,
                            h: FIELD_HEIGHT,
                        };
                        x += i32::try_from(width).unwrap_or(0);
                        field
                    })
                    .collect(),
            }
        }
        PartKind::Cells {
            label,
            values,
            vertical,
            ..
        } => Detail::Cells {
            cells: cell_boxes(rect, label, values, *vertical),
        },
    }
}

fn cell_boxes(rect: Rect, label: &str, values: &[String], vertical: bool) -> Vec<Rect> {
    let width = values
        .iter()
        .map(|value| field_width(value))
        .max()
        .unwrap_or(0);
    let top = rect.y
        + if label.is_empty() {
            0
        } else {
            i32::try_from(CELLS_LABEL_SPACE).unwrap_or(0)
        };
    (0..values.len())
        .map(|index| {
            let index = i32::try_from(index).unwrap_or(0);
            let (x, y) = if vertical {
                (
                    rect.x,
                    top + index * i32::try_from(CELL_HEIGHT).unwrap_or(0),
                )
            } else {
                (rect.x + index * i32::try_from(width).unwrap_or(0), top)
            };
            Rect {
                x,
                y,
                w: width,
                h: CELL_HEIGHT,
            }
        })
        .collect()
}
