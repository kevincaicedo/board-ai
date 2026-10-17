//! A spec's structure, resolved before layout (D-187): each part's id, the container it sits in
//! and how deep, its category, and each connector's ends. What can't be kept is dropped and
//! counted, never fatal: an id already on the layer or written twice, a container nested past
//! [`DIAGRAM_NESTING_MAX`] (what it held moves into the deepest container kept), a category past
//! the palette, a connector naming nothing.
//!
//! Every loop here is bounded by the spec's parts ([`DIAGRAM_PARTS_MAX`]) or the nesting limit;
//! there is no recursion.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::ElementId;
use crate::limits::{DIAGRAM_CATEGORIES_MAX, DIAGRAM_NESTING_MAX, DIAGRAM_PARTS_MAX};
use crate::narration::diagram::{DiagramSpec, Flow, SpecItem, SpecPart};
use crate::narration::ops::IndexLabels;
use crate::protocol::{Category, LineStyle, NodeShape};

/// The palette, in the order a diagram first uses its categories (canvas 15).
const PALETTE: [Category; DIAGRAM_CATEGORIES_MAX] = [
    Category::Slate,
    Category::Violet,
    Category::Blue,
    Category::Green,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Structure {
    /// Every part but the connectors, in the order written.
    pub parts: Vec<Part>,
    pub edges: Vec<Edge>,
    /// The categories named, in first use, with the name as first written: the legend.
    pub categories: Vec<(Category, String)>,
    pub dropped: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Part {
    pub id: ElementId,
    /// The container it sits in, an index into `parts`; `None` at the diagram's top level.
    pub parent: Option<usize>,
    pub kind: PartKind,
    /// The category it names itself, for the legend.
    pub category: Option<Category>,
    /// Its own category or the nearest container's: the colours it is drawn in.
    pub tone: Option<Category>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PartKind {
    Container {
        label: String,
        note: Option<String>,
        dashed: bool,
        flow: Option<Flow>,
    },
    Node {
        label: String,
        shape: NodeShape,
        note: Option<String>,
        step: Option<u8>,
        dashed: bool,
        new_row: bool,
    },
    Note {
        text: String,
    },
    Record {
        label: String,
        fields: Vec<String>,
    },
    Cells {
        label: String,
        values: Vec<String>,
        index: IndexLabels,
        vertical: bool,
    },
}

/// A connector between two parts, or between fields or cells of them (`arena.2`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Edge {
    pub id: ElementId,
    pub from: ElementId,
    pub to: ElementId,
    /// The parts its ends belong to.
    pub from_part: usize,
    pub to_part: usize,
    pub label: Option<String>,
    pub style: LineStyle,
}

impl Part {
    pub fn is_container(&self) -> bool {
        matches!(self.kind, PartKind::Container { .. })
    }

    /// The elements of a record's fields or a row's cells, `{id}.{i}`.
    pub fn sub_count(&self) -> usize {
        match &self.kind {
            PartKind::Record { fields, .. } => fields.len(),
            PartKind::Cells { values, .. } => values.len(),
            PartKind::Container { .. } | PartKind::Node { .. } | PartKind::Note { .. } => 0,
        }
    }
}

/// Resolves `spec`. `taken` says an id already names something on the layer; `inherited` are the
/// categories of the diagram a detail drills into, which keep their colours here (canvas 15b's
/// region stays green inside the arena).
pub(super) fn resolve(
    spec: &DiagramSpec,
    taken: impl Fn(&ElementId) -> bool,
    inherited: &[(Category, String)],
) -> Structure {
    let mut dropped = 0;
    let mut parts = Vec::new();
    let mut parents = Vec::new();
    let mut seen: BTreeSet<ElementId> = BTreeSet::new();
    seen.insert(spec.id.clone());
    let mut notes = 0;
    let mut edge_items = Vec::new();
    for item in spec.items.iter().take(DIAGRAM_PARTS_MAX) {
        if let SpecPart::Edge { .. } = item.part {
            edge_items.push(item);
            continue;
        }
        let Some(part) = part_of(spec, item, &mut notes) else {
            dropped += 1;
            continue;
        };
        let subs = (0..part.sub_count()).filter_map(|index| sub_id(&part.id, index));
        let clashes = !seen.insert(part.id.clone())
            || taken(&part.id)
            || subs.into_iter().any(|sub| taken(&sub) || !seen.insert(sub));
        if clashes {
            dropped += 1;
            continue;
        }
        parents.push(item.parent.clone());
        parts.push(part);
    }
    dropped += u32::try_from(spec.items.len().saturating_sub(DIAGRAM_PARTS_MAX)).unwrap_or(0);
    link_parents(&mut parts, &parents);
    dropped += drop_too_deep(&mut parts);
    let categories = categorize(spec, &mut parts, inherited, &mut dropped);
    let edges = resolve_edges(&edge_items, &parts, &mut dropped);
    Structure {
        parts,
        edges,
        categories,
        dropped,
    }
}

/// A field's or a cell's element id.
pub(super) fn sub_id(id: &ElementId, index: usize) -> Option<ElementId> {
    ElementId::parse(&format!("{}.{index}", id.as_str()))
}

fn part_of(spec: &DiagramSpec, item: &SpecItem, notes: &mut usize) -> Option<Part> {
    let (id, kind) = match &item.part {
        SpecPart::Note { id, text } => {
            *notes += 1;
            let id = id
                .clone()
                .or_else(|| ElementId::parse(&format!("{}:note{}", spec.id.as_str(), *notes)))?;
            (id, PartKind::Note { text: text.clone() })
        }
        part @ (SpecPart::Container { .. }
        | SpecPart::Node { .. }
        | SpecPart::Record { .. }
        | SpecPart::Cells { .. }) => (part.id()?.clone(), kind_of(part)?),
        SpecPart::Edge { .. } => return None,
    };
    // Categories are named by the talker and resolved for the whole diagram (`categorize`).
    Some(Part {
        id,
        parent: None,
        kind,
        category: None,
        tone: None,
    })
}

/// What a part with an id of its own is.
fn kind_of(part: &SpecPart) -> Option<PartKind> {
    Some(match part {
        SpecPart::Container {
            label,
            note,
            dashed,
            flow,
            ..
        } => PartKind::Container {
            label: label.clone(),
            note: note.clone(),
            dashed: *dashed,
            flow: *flow,
        },
        // A note written as a node is a note.
        SpecPart::Node {
            label,
            shape: NodeShape::Note,
            ..
        } => PartKind::Note {
            text: label.clone(),
        },
        SpecPart::Node {
            label,
            shape,
            note,
            step,
            dashed,
            new_row,
            ..
        } => PartKind::Node {
            label: label.clone(),
            // A group can't be a box; a box keeps its shape.
            shape: if *shape == NodeShape::Group {
                NodeShape::Service
            } else {
                *shape
            },
            note: note.clone(),
            step: *step,
            dashed: *dashed,
            new_row: *new_row,
        },
        SpecPart::Record { label, fields, .. } => PartKind::Record {
            label: label.clone(),
            fields: fields.clone(),
        },
        SpecPart::Cells {
            label,
            values,
            index,
            vertical,
            ..
        } => PartKind::Cells {
            label: label.clone(),
            values: values.clone(),
            index: index.clone(),
            vertical: *vertical,
        },
        SpecPart::Note { .. } | SpecPart::Edge { .. } => return None,
    })
}

/// Each part goes in the container it names. A name of a part that isn't a container puts it
/// beside that part, in that part's container (D-120's `in` on a plain node); a name of nothing
/// puts it at the top level. A container can't hold itself or one of its own containers: the
/// cycle is broken at the container that closes it.
fn link_parents(parts: &mut [Part], parents: &[Option<ElementId>]) {
    let index: BTreeMap<&ElementId, usize> = parts
        .iter()
        .enumerate()
        .map(|(at, part)| (&part.id, at))
        .collect();
    let named: Vec<Option<usize>> = parents
        .iter()
        .map(|parent| parent.as_ref().and_then(|name| index.get(name).copied()))
        .collect();
    let containers: Vec<bool> = parts.iter().map(Part::is_container).collect();
    // A part named `in` a plain box goes beside it: up the names to the nearest container (live,
    // gpt-oss nested fields in a pill in a pill).
    let resolved: Vec<Option<usize>> = (0..parts.len())
        .map(|at| {
            let mut cursor = named[at];
            for _ in 0..parts.len() {
                match cursor {
                    Some(parent) if parent == at => return None,
                    Some(parent) if containers[parent] => return Some(parent),
                    Some(parent) => cursor = named[parent],
                    None => return None,
                }
            }
            None
        })
        .collect();
    for (at, part) in parts.iter_mut().enumerate() {
        part.parent = resolved[at].filter(|_| !closes_cycle(&resolved, at));
    }
}

/// Walking up from `at` comes back to it, or doesn't end within the parts' count.
fn closes_cycle(parents: &[Option<usize>], at: usize) -> bool {
    let mut cursor = parents[at];
    for _ in 0..parents.len() {
        match cursor {
            None => return false,
            Some(next) if next == at => return true,
            Some(next) => cursor = parents.get(next).copied().flatten(),
        }
    }
    true
}

/// How many containers hold `at`, itself included when it is one, walking at most past the
/// nesting limit.
fn depth(parts: &[Part], at: usize) -> usize {
    let mut levels = usize::from(parts[at].is_container());
    let mut cursor = parts[at].parent;
    while let Some(parent) = cursor {
        levels += 1;
        if levels > DIAGRAM_NESTING_MAX + 1 {
            break;
        }
        cursor = parts[parent].parent;
    }
    levels
}

/// Drops containers nested past the limit; what they held moves up to the deepest container
/// kept. Returns how many were dropped.
fn drop_too_deep(parts: &mut Vec<Part>) -> u32 {
    let too_deep: Vec<bool> = (0..parts.len())
        .map(|at| parts[at].is_container() && depth(parts, at) > DIAGRAM_NESTING_MAX)
        .collect();
    let dropped = too_deep.iter().filter(|deep| **deep).count();
    if dropped == 0 {
        return 0;
    }
    // The nearest kept container, walking up past dropped ones (bounded by the parts' count).
    let kept_parent = |mut cursor: Option<usize>| {
        for _ in 0..parts.len() {
            match cursor {
                Some(parent) if too_deep[parent] => cursor = parts[parent].parent,
                other => return other,
            }
        }
        None
    };
    let parents: Vec<Option<usize>> = parts.iter().map(|part| kept_parent(part.parent)).collect();
    let mut renumber = vec![None; parts.len()];
    let mut next = 0;
    for (at, deep) in too_deep.iter().enumerate() {
        if !deep {
            renumber[at] = Some(next);
            next += 1;
        }
    }
    let mut kept = Vec::with_capacity(next);
    for (at, mut part) in std::mem::take(parts).into_iter().enumerate() {
        if too_deep[at] {
            continue;
        }
        part.parent = parents[at].and_then(|parent| renumber[parent]);
        kept.push(part);
    }
    *parts = kept;
    u32::try_from(dropped).unwrap_or(u32::MAX)
}

/// Gives each category named a colour of the palette in first use, and each part its tone: its
/// own category's, else its container's.
fn categorize(
    spec: &DiagramSpec,
    parts: &mut [Part],
    inherited: &[(Category, String)],
    dropped: &mut u32,
) -> Vec<(Category, String)> {
    let names: BTreeMap<&ElementId, &str> = spec
        .items
        .iter()
        .filter_map(|item| {
            let name = match &item.part {
                SpecPart::Container { category, .. } | SpecPart::Node { category, .. } => {
                    category.as_deref()?
                }
                SpecPart::Note { .. }
                | SpecPart::Record { .. }
                | SpecPart::Cells { .. }
                | SpecPart::Edge { .. } => return None,
            };
            Some((item.part.id()?, name))
        })
        .collect();
    let mut categories: Vec<(Category, String)> = Vec::new();
    for part in parts.iter_mut() {
        part.category = None;
        let Some(name) = names.get(&part.id) else {
            continue;
        };
        let same =
            |(_, known): &&(Category, String)| known.trim().eq_ignore_ascii_case(name.trim());
        let known = categories.iter().find(same).map(|(category, _)| *category);
        part.category = known.or_else(|| {
            // A name the drilled-into diagram used keeps its colour; a new one takes the first
            // colour neither uses.
            let category = inherited
                .iter()
                .find(same)
                .map(|(category, _)| *category)
                .or_else(|| {
                    PALETTE.into_iter().find(|colour| {
                        !categories
                            .iter()
                            .chain(inherited)
                            .any(|(used, _)| used == colour)
                    })
                });
            match category {
                Some(category) if categories.len() < DIAGRAM_CATEGORIES_MAX => {
                    categories.push((category, (*name).to_owned()));
                    Some(category)
                }
                Some(_) | None => {
                    *dropped += 1;
                    None
                }
            }
        });
    }
    // Containers come before what they hold only when written so; walk up instead.
    let tones: Vec<Option<Category>> = (0..parts.len())
        .map(|at| {
            let mut cursor = Some(at);
            for _ in 0..=DIAGRAM_NESTING_MAX + 1 {
                let part = &parts[cursor?];
                if part.category.is_some() {
                    return part.category;
                }
                cursor = part.parent;
            }
            None
        })
        .collect();
    for (part, tone) in parts.iter_mut().zip(tones) {
        part.tone = tone;
    }
    categories
}

/// Whether container `outer` holds `inner`, at any depth.
fn holds(parts: &[Part], outer: usize, inner: usize) -> bool {
    let mut cursor = parts[inner].parent;
    for _ in 0..parts.len() {
        match cursor {
            Some(parent) if parent == outer => return true,
            Some(parent) => cursor = parts[parent].parent,
            None => return false,
        }
    }
    false
}

/// Each connector's ends: parts of this diagram, or fields and cells of them. Anything else, or
/// a connector written twice, is dropped.
fn resolve_edges(items: &[&SpecItem], parts: &[Part], dropped: &mut u32) -> Vec<Edge> {
    let owner = |id: &ElementId| -> Option<usize> {
        parts.iter().position(|part| part.id == *id).or_else(|| {
            let (base, index) = id.as_str().rsplit_once('.')?;
            let index: usize = index.parse().ok()?;
            parts
                .iter()
                .position(|part| part.id.as_str() == base && index < part.sub_count())
        })
    };
    let mut edges: Vec<Edge> = Vec::new();
    for item in items {
        let SpecPart::Edge {
            id,
            from,
            to,
            label,
            style,
        } = &item.part
        else {
            continue;
        };
        let id = id
            .clone()
            .or_else(|| ElementId::parse(&format!("{}>{}", from.as_str(), to.as_str())));
        let ends = owner(from).zip(owner(to));
        let (Some(id), Some((from_part, to_part))) = (id, ends) else {
            *dropped += 1;
            continue;
        };
        let clashes =
            edges.iter().any(|edge| edge.id == id) || parts.iter().any(|part| part.id == id);
        // A container joined to a part inside it says what the frame already shows (live
        // models wrote "manages" arrows from a section to each of its parts).
        let containing = holds(parts, from_part, to_part) || holds(parts, to_part, from_part);
        if clashes || containing {
            *dropped += 1;
            continue;
        }
        edges.push(Edge {
            id,
            from: from.clone(),
            to: to.clone(),
            from_part,
            to_part,
            label: label.clone(),
            style: *style,
        });
    }
    edges
}
