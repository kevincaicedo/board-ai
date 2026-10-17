//! Rows and columns of cells (D-120): arrays, memory pages, hash buckets, a linked list's boxes.
//! One `cells` op draws them all; each cell is its own element (`arr.3`, from 0), so the talker
//! can point at it, mark it, highlight it, write into it and strike it, and an edge can start at
//! it. A row's label is written left of it and its index labels under each cell; a column's label
//! sits above it and its index labels left of each cell. The whole of it, labels included, is
//! placed like a node, and next to a cell it is lined up with that cell.

use crate::board::layout::{self, CellSpan};
use crate::board::place::{Spot, spot, strike};
use crate::board::{Board, Element, ElementKind, ElementState, Geometry, Layer, Placed, Rejected};
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::ELEMENTS_PER_LAYER_MAX;
use crate::narration::ops::{CellsSpec, IndexLabels};
use crate::protocol::{CellBox, DrawOp};

impl Board {
    pub fn place_cells(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        spec: &CellsSpec,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        if let Some(existing) = layer.element(&spec.id) {
            // A row is changed a cell at a time (`write`, `strike`), never redrawn whole.
            return Ok(match existing.state {
                ElementState::Frozen { .. } => Placed::ResumeFrozen,
                ElementState::Pending
                | ElementState::Drawing { .. }
                | ElementState::Drawn
                | ElementState::Struck => Placed::Unchanged,
            });
        }
        let mut cells = cells_at_origin(spec)?;
        if let Some(taken) = cells.iter().find(|cell| layer.element(&cell.id).is_some()) {
            return Err(Rejected::IdTaken(taken.id.clone()));
        }
        if layer.elements().len() + 1 + cells.len() > ELEMENTS_PER_LAYER_MAX {
            return Err(Rejected::LayerFull);
        }
        let span = place_row(layer, spec, &mut cells);
        let strip = strip_of(&cells);
        let extent = layout::cells_extent(&cells, &spec.label, spec.vertical);
        match span {
            Some((span, rows)) => {
                for row in 0..rows {
                    let cell = layout::Cell {
                        row: span.cell.row + row,
                        ..span.cell
                    };
                    layer.grid.occupy(CellSpan { cell, ..span }, &spec.id);
                }
            }
            None => layer.grid.occupy_rect(extent, &spec.id),
        }
        layer.insert(Element {
            id: spec.id.clone(),
            op,
            kind: ElementKind::Cells {
                vertical: spec.vertical,
                span: span.map(|(span, _)| span),
                extent,
            },
            label: spec.label.clone(),
            geometry: Geometry::Box(strip),
            state: ElementState::Pending,
        })?;
        for (index, cell) in (0_u8..).zip(&cells) {
            layer.insert(Element {
                id: cell.id.clone(),
                op,
                kind: ElementKind::Cell {
                    strip: spec.id.clone(),
                    index,
                    index_label: cell.index.clone(),
                },
                label: cell.value.clone(),
                geometry: Geometry::Box(cell.rect),
                state: ElementState::Pending,
            })?;
        }
        Ok(Placed::New {
            id: spec.id.clone(),
            draw: DrawOp::Cells {
                id: spec.id.clone(),
                op,
                label: spec.label.clone(),
                cells,
                vertical: spec.vertical,
            },
        })
    }

    /// `write target text`: an element with no label (an empty cell) gets it written in; one
    /// with a label is struck and rewritten.
    pub fn place_write(
        &mut self,
        layer_id: LayerId,
        op: OpId,
        target: &ElementId,
        text: &str,
    ) -> Result<Placed, Rejected> {
        let layer = self.layer_mut(layer_id)?;
        let existing = layer
            .element(target)
            .filter(|element| element.state != ElementState::Pending)
            .ok_or_else(|| Rejected::UnknownElement(target.clone()))?;
        if !existing.label.trim().is_empty() {
            return Ok(if existing.label == text {
                Placed::Unchanged
            } else {
                strike(existing, op, text)
            });
        }
        Ok(Placed::Write {
            target: target.clone(),
            draw: DrawOp::Write {
                op,
                target: target.clone(),
                label: text.to_owned(),
                rect: existing.geometry.bounds(),
            },
        })
    }

    /// A written label is now the element's; it was never struck.
    pub fn apply_write(&mut self, layer_id: LayerId, target: &ElementId, label: &str) {
        if let Some(element) = self
            .layer_mut(layer_id)
            .ok()
            .and_then(|layer| layer.element_mut(target))
        {
            label.clone_into(&mut element.label);
        }
    }
}

/// Moves the cells, laid out from the origin, to where the row goes, labels included; returns its
/// grid cells and rows when it went on the grid.
fn place_row(layer: &Layer, spec: &CellsSpec, cells: &mut [CellBox]) -> Option<(CellSpan, i32)> {
    let strip = strip_of(cells);
    let extent = layout::cells_extent(cells, &spec.label, spec.vertical);
    let (left, top) = (strip.x - extent.x, strip.y - extent.y);
    let (span, footprint) = match spot(layer, &spec.placement, extent.w, extent.h) {
        Spot::Grid { cell, cols, .. } => {
            let rows = layout::rows_for(extent.h);
            let rect = layout::rect_in_block(cell, cols, rows, extent.w, extent.h);
            (Some((CellSpan { cell, cols }, rows)), rect)
        }
        Spot::Free { rect } => {
            // Line the cells up with what they were placed beside, not their labels.
            let (whole, middle) = (extent.center(), strip.center());
            let rect = Rect {
                x: rect.x + whole.x - middle.x,
                y: rect.y + whole.y - middle.y,
                ..rect
            };
            (None, rect)
        }
    };
    let (dx, dy) = (footprint.x + left - strip.x, footprint.y + top - strip.y);
    for cell in cells {
        cell.rect.x += dx;
        cell.rect.y += dy;
    }
    span
}

/// The cells of `spec` laid out from the origin, before the row is placed.
fn cells_at_origin(spec: &CellsSpec) -> Result<Vec<CellBox>, Rejected> {
    let count = usize::from(spec.count.max(1));
    let mut values = spec.values.clone();
    values.resize(count, String::new());
    let width = layout::cell_width(&values);
    let height = layout::CELL_BOX_HEIGHT;
    let (step_x, step_y) = if spec.vertical {
        (0, i32::try_from(height).unwrap_or(0))
    } else {
        (i32::try_from(width).unwrap_or(0), 0)
    };
    let mut cells = Vec::with_capacity(count);
    for (position, value) in (0_i32..).zip(values) {
        let index = usize::try_from(position).unwrap_or(0);
        let id = ElementId::parse(&format!("{}.{position}", spec.id.as_str()))
            .ok_or_else(|| Rejected::IdTaken(spec.id.clone()))?;
        let label = match &spec.index {
            IndexLabels::From(first) => u32::try_from(position)
                .ok()
                .map(|position| (first + position).to_string()),
            IndexLabels::Named(names) => names.get(index).filter(|name| !name.is_empty()).cloned(),
            IndexLabels::None => None,
        };
        cells.push(CellBox {
            id,
            rect: Rect {
                x: position * step_x,
                y: position * step_y,
                w: width,
                h: height,
            },
            value,
            index: label,
        });
    }
    Ok(cells)
}

fn strip_of(cells: &[CellBox]) -> Rect {
    cells
        .iter()
        .map(|cell| cell.rect)
        .reduce(Rect::union)
        .unwrap_or_default()
}
