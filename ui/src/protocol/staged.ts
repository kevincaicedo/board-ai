// The protocol of staged diagrams (D-187, D-188, canvas 15 and 15b): a reveal's parts, a detail
// layer's anchor and the diagram header, parsed at the boundary. A reveal with one malformed part
// is dropped whole, as any malformed op is, and counted by the caller.

import { cellBox, edgePath, LINE_STYLES, NODE_SHAPES } from "./draw.ts";
import {
  elementId,
  fields,
  flag,
  list,
  nullable,
  oneOf,
  point,
  rect,
  type Tagged,
  tagged,
  text,
  u8,
} from "./fields.ts";
import type { Category } from "./generated/Category.ts";
import type { DetailAnchor } from "./generated/DetailAnchor.ts";
import type { StagedPart } from "./generated/StagedPart.ts";
import type { TextAt } from "./generated/TextAt.ts";

/**
 * Parts one reveal may carry: the core's elements per layer (`ELEMENTS_PER_LAYER_MAX`), since a
 * reveal draws parts of one layer and each part is an element. More is malformed.
 */
export const REVEAL_PARTS_MAX = 500;
/** Fields of a record or cells of a row: the core's `CELLS_PER_STRIP_MAX`. */
export const STAGED_STRIP_MAX = 32;
/** Lines of a note: generous over the core's bound, so only a malformed note trips it. */
export const STAGED_NOTE_LINES_MAX = 32;

export const CATEGORIES: readonly Category[] = ["Slate", "Violet", "Blue", "Green"];

const category = (value: unknown): Category | null => oneOf(value, CATEGORIES);

function bounded<T>(value: unknown, max: number, parse: (value: unknown) => T | null): T[] | null {
  return Array.isArray(value) && value.length <= max ? list(value, parse) : null;
}

function textAt(value: unknown): TextAt | null {
  const f = fields(value);
  const words = text(f?.text);
  const at = point(f?.at);
  return words === null || at === null ? null : { text: words, at };
}

const PARTS: Tagged<StagedPart, "part"> = {
  Frame: (f) => {
    const id = elementId(f.id);
    const r = rect(f.rect);
    const label = textAt(f.label);
    const note = nullable(f.note, textAt);
    const tone = nullable(f.category, category);
    const dashed = flag(f.dashed);
    return id === null ||
      r === null ||
      label === null ||
      note === undefined ||
      tone === undefined ||
      dashed === null
      ? null
      : { part: "Frame", id, rect: r, label, note, category: tone, dashed };
  },
  Node: (f) => {
    const id = elementId(f.id);
    const shape = oneOf(f.shape, NODE_SHAPES);
    const r = rect(f.rect);
    const label = text(f.label);
    const note = nullable(f.note, text);
    const step = nullable(f.step, u8);
    const tone = nullable(f.category, category);
    const dashed = flag(f.dashed);
    return id === null ||
      shape === null ||
      r === null ||
      label === null ||
      note === undefined ||
      step === undefined ||
      tone === undefined ||
      dashed === null
      ? null
      : { part: "Node", id, shape, rect: r, label, note, step, category: tone, dashed };
  },
  Note: (f) => {
    const id = elementId(f.id);
    const r = rect(f.rect);
    const lines = bounded(f.lines, STAGED_NOTE_LINES_MAX, text);
    return id === null || r === null || lines === null
      ? null
      : { part: "Note", id, rect: r, lines };
  },
  Record: (f) => {
    const id = elementId(f.id);
    const label = text(f.label);
    const cells = bounded(f.fields, STAGED_STRIP_MAX, cellBox);
    return id === null || label === null || cells === null || cells.length === 0
      ? null
      : { part: "Record", id, label, fields: cells };
  },
  Cells: (f) => {
    const id = elementId(f.id);
    const label = text(f.label);
    const cells = bounded(f.cells, STAGED_STRIP_MAX, cellBox);
    const vertical = flag(f.vertical);
    return id === null ||
      label === null ||
      cells === null ||
      cells.length === 0 ||
      vertical === null
      ? null
      : { part: "Cells", id, label, cells, vertical };
  },
  Edge: (f) => {
    const id = elementId(f.id);
    const from = elementId(f.from);
    const to = elementId(f.to);
    const path = edgePath(f.path);
    const label = nullable(f.label, textAt);
    const style = oneOf(f.style, LINE_STYLES);
    return id === null ||
      from === null ||
      to === null ||
      path === null ||
      label === undefined ||
      style === null
      ? null
      : { part: "Edge", id, from, to, path, label, style };
  },
  Legend: (f) => {
    const id = elementId(f.id);
    const r = rect(f.rect);
    const tone = category(f.category);
    const label = text(f.label);
    return id === null || r === null || tone === null || label === null
      ? null
      : { part: "Legend", id, rect: r, category: tone, label };
  },
};

export function stagedPart(value: unknown): StagedPart | null {
  return tagged<StagedPart>(PARTS, "part", value);
}

/** A reveal's parts: one to `REVEAL_PARTS_MAX`, every one well formed. */
export function stagedParts(value: unknown): StagedPart[] | null {
  const parts = bounded(value, REVEAL_PARTS_MAX, stagedPart);
  return parts === null || parts.length === 0 ? null : parts;
}

export function detailAnchor(value: unknown): DetailAnchor | null {
  const f = fields(value);
  const layer = u8(f?.layer);
  const element = elementId(f?.element);
  const r = rect(f?.rect);
  const tone = nullable(f?.category, category);
  return layer === null || element === null || r === null || tone === undefined
    ? null
    : { layer, element, rect: r, category: tone };
}
