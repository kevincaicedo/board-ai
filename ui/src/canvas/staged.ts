// A staged diagram's reveal (15 · a detailed diagram revealed as it is explained, D-187): every
// part the narration named, planned in the order the core sent them, each seeded by its own id,
// into one item list, so one run draws them as a hand would within its budget: a frame and its
// label before what is inside, both ends before what joins them. The core laid every part out in
// the canvas's small diagram sizes (`board::staged::metrics`); the text sizes and offsets here
// are the same numbers, read from the `--diagram-*` tokens.
//
// Each part is a plan target of its own (its area, and its label for a strike to cross); a
// record's fields and a row's cells are targets too (`rec.2`), as a sketch's cells are.

import type { Category } from "../protocol/generated/Category.ts";
import type { CellBox } from "../protocol/generated/CellBox.ts";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { StagedPart } from "../protocol/generated/StagedPart.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { PlanTarget } from "./diagrams.ts";
import {
  arrowHead,
  ellipsePolygon,
  roughCurve,
  roughEllipse,
  roughLine,
  roughRoundedRect,
  roundedRectPolygon,
  type Vec,
} from "./geometry.ts";
import { type Random, randomFor } from "./rng.ts";
import { GROUP_PACE } from "./sketch.ts";
import { type Item, makeFill, makeStroke, makeText, type TextMark, type Tone } from "./stroke.ts";

export type StagedTokens = Pick<
  Tokens,
  | "diagramFrameLabel"
  | "diagramNote"
  | "diagramLabel"
  | "diagramField"
  | "diagramEdge"
  | "diagramLegend"
  | "diagramNoteLine"
  | "diagramNodeLabelCenter"
  | "diagramNodeNoteCenter"
  | "diagramRecordLabelCenter"
  | "diagramCellsLabelAbove"
  | "diagramCellsIndexBelow"
  | "diagramLegendSwatchWidth"
  | "diagramLegendSwatchHeight"
  | "diagramLegendGap"
  | "diagramFrameRadius"
  | "diagramNodeRadius"
  | "diagramFieldRadius"
  | "diagramFrameWidth"
  | "diagramNodeWidth"
  | "diagramFieldWidth"
  | "diagramEdgeWidth"
  | "diagramArrowHead"
  | "diagramFrameDash"
  | "diagramFrameDashGap"
  | "diagramSwatchDash"
  | "diagramSwatchDashGap"
>;

/** A category's stroke tone (frames, swatches) and its soft fill (boxes), canvas 15's pairs. */
export function categoryStroke(category: Category): Tone {
  switch (category) {
    case "Slate":
      return "slate";
    case "Violet":
      return "violet";
    case "Blue":
      return "blue";
    case "Green":
      return "green";
  }
}

export function categoryFill(category: Category): Tone {
  switch (category) {
    case "Slate":
      return "slate-soft";
    case "Violet":
      return "violet-soft";
    case "Blue":
      return "blue-soft";
    case "Green":
      return "green-soft";
  }
}

/** A queue's message slots: the right two fifths of its box, as the sketch's queue reads. */
const QUEUE_SLOTS = 4;
const QUEUE_SLOTS_SHARE = 0.4;

export interface StagedPlan {
  readonly items: Item[];
  readonly targets: PlanTarget[];
}

type Reveal = Extract<DrawOp, { kind: "Reveal" }>;

/** Every part of a reveal, in order, and every part (and field or cell) as a target. */
export function revealItems(op: Reveal, tokens: StagedTokens): StagedPlan {
  const items: Item[] = [];
  const targets: PlanTarget[] = [];
  for (const part of op.parts) {
    planPart(part, randomFor(part.id), tokens, items, targets);
  }
  return { items, targets };
}

function planPart(
  part: StagedPart,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
  targets: PlanTarget[],
): void {
  switch (part.part) {
    case "Frame":
      targets.push({
        id: part.id,
        rect: part.rect,
        label: frameItems(part, random, tokens, items),
      });
      break;
    case "Node":
      targets.push({ id: part.id, rect: part.rect, label: nodeItems(part, random, tokens, items) });
      break;
    case "Note": {
      let first: TextMark | null = null;
      for (let index = 0; index < part.lines.length; index += 1) {
        const mark = makeText(
          part.rect.x,
          part.rect.y + tokens.diagramNoteLine / 2 + index * tokens.diagramNoteLine,
          part.lines[index] ?? "",
          tokens.diagramNote,
          "ink2",
          "left",
        );
        first ??= mark;
        items.push(mark);
      }
      targets.push({ id: part.id, rect: part.rect, label: first });
      break;
    }
    case "Record":
      recordItems(part, random, tokens, items, targets);
      break;
    case "Cells":
      cellsItems(part, random, tokens, items, targets);
      break;
    case "Edge":
      targets.push({
        id: part.id,
        rect: pathBounds(part.path.start, part.path.control, part.path.end),
        label: edgeItems(part, random, tokens, items),
      });
      break;
    case "Legend": {
      const { rect } = part;
      const swatchW = tokens.diagramLegendSwatchWidth;
      const swatchH = tokens.diagramLegendSwatchHeight;
      const cy = rect.y + rect.h / 2;
      const swatch = roughRoundedRect(random, rect.x, cy - swatchH / 2, swatchW, swatchH, 2);
      items.push(
        makeStroke(swatch.points, categoryStroke(part.category), tokens.diagramNodeWidth, {
          legs: swatch.legs,
          dashed: true,
          dash: [tokens.diagramSwatchDash, tokens.diagramSwatchDashGap],
        }),
      );
      const label = makeText(
        rect.x + swatchW + tokens.diagramLegendGap,
        cy,
        part.label,
        tokens.diagramLegend,
        "ink",
        "left",
      );
      items.push(label);
      targets.push({ id: part.id, rect, label });
      break;
    }
  }
}

/** A container: its frame, then its label and its note. Returns the label, for a strike. */
function frameItems(
  part: Extract<StagedPart, { part: "Frame" }>,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
): TextMark {
  const { x, y, w, h } = part.rect;
  const outline = roughRoundedRect(random, x, y, w, h, tokens.diagramFrameRadius);
  items.push(
    makeStroke(
      outline.points,
      part.category === null ? "grey" : categoryStroke(part.category),
      tokens.diagramFrameWidth,
      // A frame is scaffolding, drawn quicker than what it holds, as a group's is.
      {
        legs: outline.legs,
        dashed: part.dashed,
        dash: [tokens.diagramFrameDash, tokens.diagramFrameDashGap],
        pace: GROUP_PACE,
      },
    ),
  );
  const label = makeText(
    part.label.at.x,
    part.label.at.y,
    part.label.text,
    tokens.diagramFrameLabel,
    "ink",
    "left",
  );
  items.push(label);
  if (part.note !== null) {
    items.push(
      makeText(part.note.at.x, part.note.at.y, part.note.text, tokens.diagramNote, "ink2", "left"),
    );
  }
  return label;
}

/**
 * A box: its outline, its label (numbered for a step) and its note, then its category's soft
 * fill under the ink. Returns the label, for a strike.
 */
function nodeItems(
  part: Extract<StagedPart, { part: "Node" }>,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
): TextMark | null {
  const { x, y, w, h } = part.rect;
  const fill = part.category === null ? null : categoryFill(part.category);
  const ink = tokens.diagramNodeWidth;
  let polygon: Vec[];
  switch (part.shape) {
    case "Circle": {
      const outline = roughEllipse(random, x + w / 2, y + h / 2, w / 2, h / 2, { grow: 0.03 });
      items.push(makeStroke(outline, "ink", ink, { kind: "ellipse", dashed: part.dashed }));
      polygon = ellipsePolygon(x + w / 2, y + h / 2, w / 2, h / 2, 0);
      break;
    }
    case "Db": {
      polygon = cylinderItems(random, part.rect, ink, items);
      break;
    }
    case "Note":
      polygon = [];
      break;
    case "Service":
    case "Pill":
    case "Queue":
    case "Actor":
    case "Group": {
      const radius = part.shape === "Pill" ? h / 2 : tokens.diagramNodeRadius;
      const outline = roughRoundedRect(random, x, y, w, h, radius);
      items.push(
        makeStroke(outline.points, part.shape === "Group" ? "grey" : "ink", ink, {
          legs: outline.legs,
          dashed: part.dashed,
        }),
      );
      if (part.shape === "Queue") {
        const slot = (w * QUEUE_SLOTS_SHARE) / QUEUE_SLOTS;
        for (let divider = 0; divider < QUEUE_SLOTS; divider += 1) {
          const dx = x + w - slot * (divider + 1);
          items.push(makeStroke(roughLine(random, dx, y, dx, y + h, { overshoot: 1 }), "ink", ink));
        }
      }
      polygon = roundedRectPolygon(x, y, w, h, radius);
      break;
    }
  }
  const words = part.step === null ? part.label : `${part.step}. ${part.label}`;
  let label: TextMark | null = null;
  if (words !== "") {
    const queueLeft = part.shape === "Queue" ? w * QUEUE_SLOTS_SHARE : 0;
    const cx = x + (w - queueLeft) / 2;
    // A cylinder's label sits in its body, below the top's front arc, as a sketch's does.
    const body = part.shape === "Db" ? cylinderLip(part.rect) / 2 : 0;
    const cy = part.note === null ? y + h / 2 + body : y + tokens.diagramNodeLabelCenter + body;
    label = makeText(cx, cy, words, tokens.diagramLabel, part.shape === "Note" ? "ink2" : "ink");
    items.push(label);
  }
  if (part.note !== null) {
    items.push(
      makeText(x + w / 2, y + tokens.diagramNodeNoteCenter, part.note, tokens.diagramNote, "ink2"),
    );
  }
  if (fill !== null && polygon.length > 2) {
    items.push(makeFill(polygon, fill));
  }
  return label;
}

/** How deep a cylinder's top and base curve: the design's ~9% of its width. */
function cylinderLip(rect: Rect): number {
  return Math.max(4, Math.round(rect.w * 0.09));
}

/** A cylinder at a diagram's size: its top, its sides and its base. Returns its body, to fill. */
function cylinderItems(random: Random, rect: Rect, ink: number, items: Item[]): Vec[] {
  const cx = rect.x + rect.w / 2;
  const rx = rect.w / 2;
  const ry = cylinderLip(rect);
  const top = rect.y + ry;
  const bottom = rect.y + rect.h - ry;
  items.push(
    makeStroke(roughEllipse(random, cx, top, rx, ry, { grow: 0.02 }), "ink", ink, {
      kind: "ellipse",
    }),
    makeStroke(roughLine(random, cx - rx, top, cx - rx, bottom, { overshoot: 1 }), "ink", ink),
    makeStroke(
      roughEllipse(random, cx, bottom, rx, ry, { start: Math.PI, span: -Math.PI, grow: 0 }),
      "ink",
      ink,
      { kind: "ellipse" },
    ),
    makeStroke(roughLine(random, cx + rx, bottom, cx + rx, top, { overshoot: 1 }), "ink", ink),
  );
  return roundedRectPolygon(rect.x, rect.y, rect.w, rect.h, Math.min(rx, ry));
}

/** One field's or cell's box, rounded as canvas 15's fields are. */
function fieldBox(random: Random, cell: CellBox, tokens: StagedTokens): Item {
  const { x, y, w, h } = cell.rect;
  const outline = roughRoundedRect(random, x, y, w, h, tokens.diagramFieldRadius);
  return makeStroke(outline.points, "ink", tokens.diagramFieldWidth, { legs: outline.legs });
}

/** A field's or cell's value in the code font, a target with its box. */
function fieldValue(cell: CellBox, tokens: StagedTokens): TextMark | null {
  if (cell.value === "") {
    return null;
  }
  const { x, y, w, h } = cell.rect;
  return makeText(x + w / 2, y + h / 2, cell.value, tokens.diagramField, "ink", "center", "code");
}

/** A struct: each field's box, then the field names, then the struct's name under them. */
function recordItems(
  part: Extract<StagedPart, { part: "Record" }>,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
  targets: PlanTarget[],
): void {
  let strip: Rect | null = null;
  for (const field of part.fields) {
    items.push(fieldBox(random, field, tokens));
    strip = strip === null ? field.rect : union(strip, field.rect);
  }
  const fieldTargets: PlanTarget[] = [];
  for (const field of part.fields) {
    const value = fieldValue(field, tokens);
    if (value !== null) {
      items.push(value);
    }
    fieldTargets.push({ id: field.id, rect: field.rect, label: value });
  }
  if (strip === null) {
    return;
  }
  let name: TextMark | null = null;
  if (part.label !== "") {
    name = makeText(
      strip.x,
      strip.y + strip.h + tokens.diagramRecordLabelCenter,
      part.label,
      tokens.diagramLabel,
      "ink",
      "left",
    );
    items.push(name);
  }
  // The record before its fields: a field's target sits over the record's, so pointing at a
  // field names the field.
  targets.push({
    id: part.id,
    rect: name === null ? strip : union(strip, textBox(name)),
    label: name,
  });
  targets.push(...fieldTargets);
}

/** A row (or column) of cells: its name above, the boxes, the values, then the index labels. */
function cellsItems(
  part: Extract<StagedPart, { part: "Cells" }>,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
  targets: PlanTarget[],
): void {
  const first = part.cells[0];
  if (first === undefined) {
    return;
  }
  let strip = first.rect;
  for (const cell of part.cells) {
    strip = union(strip, cell.rect);
  }
  if (part.label !== "") {
    items.push(
      makeText(
        strip.x,
        strip.y - tokens.diagramCellsLabelAbove,
        part.label,
        tokens.diagramNote,
        "ink2",
        "left",
      ),
    );
  }
  for (const cell of part.cells) {
    items.push(fieldBox(random, cell, tokens));
  }
  const cellTargets: PlanTarget[] = [];
  for (const cell of part.cells) {
    const value = fieldValue(cell, tokens);
    if (value !== null) {
      items.push(value);
    }
    cellTargets.push({ id: cell.id, rect: cell.rect, label: value });
  }
  for (const cell of part.cells) {
    if (cell.index === null) {
      continue;
    }
    const { x, y, w, h } = cell.rect;
    items.push(
      makeText(
        x + w / 2,
        y + h + tokens.diagramCellsIndexBelow,
        cell.index,
        tokens.diagramField,
        "ink2",
        "center",
        "code",
      ),
    );
  }
  targets.push({ id: part.id, rect: strip, label: null });
  targets.push(...cellTargets);
}

/** A connector, its head, then its label centred where the core placed it. */
function edgeItems(
  part: Extract<StagedPart, { part: "Edge" }>,
  random: Random,
  tokens: StagedTokens,
  items: Item[],
): TextMark | null {
  const { start, control, end } = part.path;
  const dashed = part.style === "Dashed";
  const width = tokens.diagramEdgeWidth;
  const head = tokens.diagramArrowHead;
  if (control === null) {
    items.push(
      makeStroke(
        roughLine(random, start.x, start.y, end.x, end.y, { overshoot: 1 }),
        "ink",
        width,
        {
          dashed,
        },
      ),
      makeStroke(
        arrowHead(random, end.x, end.y, Math.atan2(end.y - start.y, end.x - start.x), head),
        "ink",
        width,
        { kind: "head", legs: [0, 2, 4] },
      ),
    );
  } else {
    items.push(
      makeStroke(
        roughCurve(random, start.x, start.y, control.x, control.y, end.x, end.y),
        "ink",
        width,
        { kind: "curve", dashed },
      ),
      makeStroke(
        arrowHead(random, end.x, end.y, Math.atan2(end.y - control.y, end.x - control.x), head),
        "ink",
        width,
        { kind: "head", legs: [0, 2, 4] },
      ),
    );
  }
  if (part.label === null || part.label.text === "") {
    return null;
  }
  const label = makeText(
    part.label.at.x,
    part.label.at.y,
    part.label.text,
    tokens.diagramEdge,
    "ink2",
  );
  items.push(label);
  return label;
}

function pathBounds(
  start: { x: number; y: number },
  control: { x: number; y: number } | null,
  end: { x: number; y: number },
): Rect {
  let left = Math.min(start.x, end.x);
  let top = Math.min(start.y, end.y);
  let right = Math.max(start.x, end.x);
  let bottom = Math.max(start.y, end.y);
  if (control !== null) {
    left = Math.min(left, control.x);
    top = Math.min(top, control.y);
    right = Math.max(right, control.x);
    bottom = Math.max(bottom, control.y);
  }
  return { x: left, y: top, w: right - left, h: bottom - top };
}

/** The box a line of writing covers, from where the stroke engine starts and ends it. */
function textBox(mark: TextMark): Rect {
  const half = mark.size * 0.6;
  return {
    x: mark.start[0],
    y: mark.y - half,
    w: mark.end[0] - mark.start[0],
    h: 2 * half,
  };
}

function union(a: Rect, b: Rect): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, w: Math.max(a.x + a.w, b.x + b.w) - x, h: Math.max(a.y + a.h, b.y + b.h) - y };
}

/** Where a part's drawing sits, for a replay scene's frame and a camera test. */
export function partRect(part: StagedPart): Rect {
  switch (part.part) {
    case "Frame":
    case "Node":
    case "Note":
    case "Legend":
      return part.rect;
    case "Record":
      return part.fields.map((field) => field.rect).reduce(union);
    case "Cells":
      return part.cells.map((cell) => cell.rect).reduce(union);
    case "Edge":
      return pathBounds(part.path.start, part.path.control, part.path.end);
  }
}

/** The order a reveal's targets stack in: frames under edges, edges under every other part. */
export const TARGET_PASSES = [0, 1, 2] as const;

/** Which pass a reveal's target is hit-tested in: a record's field or a row's cell goes last. */
export function targetPass(parts: readonly StagedPart[], id: string): number {
  const part = parts.find((candidate) => candidate.id === id);
  if (part === undefined) {
    return 2;
  }
  switch (part.part) {
    case "Frame":
      return 0;
    case "Edge":
      return 1;
    case "Node":
    case "Note":
    case "Record":
    case "Cells":
    case "Legend":
      return 2;
  }
}
