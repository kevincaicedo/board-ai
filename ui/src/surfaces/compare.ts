// The compare card's grid (StageStates: "options are columns, criteria are rows"). A cell update
// names `row` (a criterion) and `col` (an option), and the card keys it `${row}:${col}`; this is
// the one place that turns the surface into rows of cells in reading order.

import type { Surface } from "../protocol/generated/Surface.ts";

export interface CompareCellSlot {
  readonly option: string;
  readonly col: number;
  readonly key: string;
}

export interface CompareRow {
  readonly criterion: string;
  readonly row: number;
  readonly cells: readonly CompareCellSlot[];
}

export function compareTitle(surface: Extract<Surface, { kind: "Compare" }>): string {
  return surface.title ?? surface.options.join(" vs ");
}

export function compareRows(surface: Extract<Surface, { kind: "Compare" }>): CompareRow[] {
  return surface.criteria.map((criterion, row) => ({
    criterion,
    row,
    cells: surface.options.map((option, col) => ({ option, col, key: `${row}:${col}` })),
  }));
}
