//! One layer of the board: its elements in the order they were laid out, an index by id, its
//! grid and its sequence diagram.

use std::collections::BTreeMap;

use super::layout::{CellSpan, Grid};
use super::sequence::Sequence;
use super::staged::StagedDiagrams;
use crate::board::{Element, ElementKind, ElementState, Geometry, Rejected, UserMark};
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::ELEMENTS_PER_LAYER_MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub(super) id: LayerId,
    pub(super) title: String,
    /// The page it belongs to, from 1: the main layer's page, or the page a detour opened on.
    pub(super) page: u16,
    pub(super) open: bool,
    pub(super) elements: Vec<Element>,
    pub(super) index: BTreeMap<ElementId, usize>,
    pub(super) grid: Grid,
    pub(super) sequence: Sequence,
    /// The diagrams staged on it (D-187).
    pub(super) staged: StagedDiagrams,
}

impl Layer {
    pub(super) fn new(id: LayerId, title: String, page: u16) -> Self {
        Self {
            id,
            title,
            page,
            open: true,
            elements: Vec::new(),
            index: BTreeMap::new(),
            grid: Grid::default(),
            sequence: Sequence::default(),
            staged: StagedDiagrams::default(),
        }
    }

    #[must_use]
    pub fn id(&self) -> LayerId {
        self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Elements in the order they were laid out.
    #[must_use]
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    #[must_use]
    pub fn element(&self, id: &ElementId) -> Option<&Element> {
        self.index
            .get(id)
            .and_then(|&index| self.elements.get(index))
    }

    pub(super) fn element_mut(&mut self, id: &ElementId) -> Option<&mut Element> {
        let index = *self.index.get(id)?;
        self.elements.get_mut(index)
    }

    pub(super) fn insert(&mut self, element: Element) -> Result<(), Rejected> {
        if self.elements.len() >= ELEMENTS_PER_LAYER_MAX {
            return Err(Rejected::LayerFull);
        }
        if self.index.contains_key(&element.id) {
            return Err(Rejected::IdTaken(element.id));
        }
        debug_assert_eq!(
            element.op.is_none(),
            matches!(element.kind, ElementKind::User { .. }),
            "only your shapes have no op"
        );
        self.index.insert(element.id.clone(), self.elements.len());
        self.elements.push(element);
        Ok(())
    }

    /// The box of something drawn that others can be placed beside or joined to, and its grid
    /// cells when it sits on the grid (`None` places its neighbours off the grid).
    pub(super) fn node_rect(&self, id: &ElementId) -> Result<(Rect, Option<CellSpan>), Rejected> {
        let element = self
            .element(id)
            .filter(|element| element.state.is_shown())
            .ok_or_else(|| Rejected::UnknownElement(id.clone()))?;
        match (&element.kind, &element.geometry) {
            (
                ElementKind::Node { span, .. } | ElementKind::Cells { span, .. },
                Geometry::Box(rect),
            ) => Ok((*rect, *span)),
            (
                ElementKind::Cell { .. }
                | ElementKind::Participant { .. }
                | ElementKind::User {
                    mark: UserMark::Box | UserMark::Ink { .. },
                    ..
                },
                Geometry::Box(rect),
            ) => Ok((*rect, None)),
            (
                ElementKind::Node { .. }
                | ElementKind::Cells { .. }
                | ElementKind::Cell { .. }
                | ElementKind::Participant { .. }
                | ElementKind::User { .. },
                Geometry::Path(_) | Geometry::Points(_),
            )
            | (
                ElementKind::User {
                    mark: UserMark::Arrow { .. },
                    ..
                },
                Geometry::Box(_),
            )
            | (
                ElementKind::Edge { .. }
                | ElementKind::Annotation { .. }
                | ElementKind::Trace
                | ElementKind::Message { .. },
                _,
            ) => Err(Rejected::UnknownElement(id.clone())),
        }
    }

    /// The last node or row of cells laid out, where the flow continues for one with no
    /// placement.
    pub(super) fn last_node(&self) -> Option<&Element> {
        self.elements.iter().rev().find(|element| {
            matches!(
                element.kind,
                ElementKind::Node { group: None, .. } | ElementKind::Cells { .. }
            )
        })
    }

    /// Bounds of everything the user can see on this layer.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.elements
            .iter()
            .filter(|element| element.state.is_shown())
            .map(Element::area)
            .reduce(Rect::union)
    }

    /// The elements one op made, in the order it made them.
    pub(super) fn made_by(&self, op: OpId) -> impl Iterator<Item = &Element> {
        self.elements
            .iter()
            .filter(move |element| element.op == Some(op))
    }

    /// The frozen element, if a stroke on this layer was stopped mid-way.
    #[must_use]
    pub fn frozen(&self) -> Option<&Element> {
        self.elements
            .iter()
            .find(|element| matches!(element.state, ElementState::Frozen { .. }))
    }
}
