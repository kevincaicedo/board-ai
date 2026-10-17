// Turns one protocol DrawOp into the strokes and handwriting a hand would make, in a human order
// (PRODUCT §7.1): outline, then label, then fill; connector, then arrowhead, then label. Shapes and
// proportions follow the design's canvas primitives (Components.dc.html and the live demo).

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Fill } from "../protocol/generated/Fill.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import {
  cellsItems,
  circleItems,
  linkItem,
  messageItems,
  type PlanTarget,
  participantItems,
  writeItems,
} from "./diagrams.ts";
import {
  arrowHead,
  cylinderPolygon,
  ellipsePolygon,
  hachure,
  rectPolygon,
  roughCurve,
  roughEllipse,
  roughLine,
  roughRoundedRect,
  roundedRectPolygon,
  type Vec,
} from "./geometry.ts";
import { type Random, randomFor } from "./rng.ts";
import {
  boxSides,
  CONNECTOR_WIDTH,
  EDGE_LABEL_GAP,
  GROUP_PACE,
  GROUP_WIDTH,
  HATCH_DEGREES,
  HATCH_GAP,
  HATCH_INSET,
  LABEL_ABOVE,
  labelSize,
  OUTLINE_WIDTH,
  type PlanTokens,
} from "./sketch.ts";
import { revealItems } from "./staged.ts";

export type { PlanTokens } from "./sketch.ts";

import {
  type Item,
  itemEnd,
  itemStart,
  makeHatch,
  makeStroke,
  makeText,
  type Stroke,
  type TextAlign,
  type Tone,
  travelMs,
} from "./stroke.ts";

export interface OpPlan {
  readonly items: readonly Item[];
  /** Natural start of each item, in ms from the first item's start. */
  readonly starts: Float64Array;
  readonly ends: Float64Array;
  readonly naturalMs: number;
  /** World-space bounds, for pointer hit testing and the hover highlight. */
  readonly bounds: Rect;
  /** Elements inside the drawing (a row's cells), each pointable and strikable on its own. */
  readonly targets: readonly PlanTarget[];
}

const STACK_OFFSET = 8;
const QUEUE_SLOTS = 5;
const QUEUE_FILLED_SLOTS = 3;
const ANNOTATION_PAD = 10;

/** Sequential natural times: each item starts after the previous one plus the pen's travel. */
export function schedule(items: readonly Item[]): {
  starts: Float64Array;
  ends: Float64Array;
  naturalMs: number;
} {
  const starts = new Float64Array(items.length);
  const ends = new Float64Array(items.length);
  const from: Vec = [0, 0];
  const to: Vec = [0, 0];
  let time = 0;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    if (item === undefined) {
      continue;
    }
    const previous = items[index - 1];
    if (previous !== undefined) {
      time += travelMs(itemEnd(previous, from), itemStart(item, to));
    }
    starts[index] = time;
    time += item.durationMs;
    ends[index] = time;
  }
  return { starts, ends, naturalMs: time };
}

function service(
  random: Random,
  rect: Rect,
  label: string,
  stack: number,
  fill: Tone | null,
  tokens: PlanTokens,
): Item[] {
  const items: Item[] = [];
  for (let copy = Math.min(stack, 4) - 1; copy >= 1; copy -= 1) {
    const offset = STACK_OFFSET * copy;
    const corner: Vec[] = [
      ...roughLine(
        random,
        rect.x + offset,
        rect.y - offset,
        rect.x + rect.w + offset,
        rect.y - offset,
        {
          overshoot: 2,
        },
      ),
    ];
    const down = roughLine(
      random,
      rect.x + rect.w + offset,
      rect.y - offset,
      rect.x + rect.w + offset,
      rect.y + rect.h - offset,
      { overshoot: 2 },
    );
    const legs = [0, corner.length - 1, corner.length + down.length - 2];
    items.push(
      makeStroke([...corner, ...down.slice(1)], "ink", OUTLINE_WIDTH, { legs, pace: GROUP_PACE }),
    );
  }
  items.push(...boxSides(random, rect, "ink", OUTLINE_WIDTH, 1));
  if (label !== "") {
    items.push(
      makeText(
        rect.x + rect.w / 2,
        rect.y + rect.h / 2,
        label,
        labelSize(tokens, rect, label),
        "ink",
      ),
    );
  }
  if (fill !== null) {
    items.push(
      makeHatch(
        hachure(
          random,
          rectPolygon(rect.x, rect.y, rect.w, rect.h, HATCH_INSET),
          HATCH_GAP,
          HATCH_DEGREES,
        ),
        fill,
      ),
    );
  }
  return items;
}

function cylinder(
  random: Random,
  rect: Rect,
  label: string,
  fill: Tone | null,
  tokens: PlanTokens,
): Item[] {
  const cx = rect.x + rect.w / 2;
  const rx = rect.w / 2;
  // The design's cylinders keep a cap about 9% of their width deep (144 → 13, 136 → 12).
  const ry = Math.max(6, Math.round(rect.w * 0.09));
  const top = rect.y + ry;
  const bottom = rect.y + rect.h - ry;
  const items: Item[] = [
    makeStroke(roughEllipse(random, cx, top, rx, ry, { grow: 0.02 }), "ink", OUTLINE_WIDTH, {
      kind: "ellipse",
    }),
    makeStroke(
      roughLine(random, cx - rx, top, cx - rx, bottom, { overshoot: 2 }),
      "ink",
      OUTLINE_WIDTH,
    ),
    makeStroke(
      roughEllipse(random, cx, bottom, rx, ry, { start: Math.PI, span: -Math.PI, grow: 0 }),
      "ink",
      OUTLINE_WIDTH,
      { kind: "ellipse" },
    ),
    makeStroke(
      roughLine(random, cx + rx, bottom, cx + rx, top, { overshoot: 2 }),
      "ink",
      OUTLINE_WIDTH,
    ),
  ];
  if (label !== "") {
    const body = { x: rect.x, y: top, w: rect.w, h: bottom - top };
    items.push(
      makeText(cx, (top + bottom) / 2 + ry / 2, label, labelSize(tokens, body, label), "ink"),
    );
  }
  if (fill !== null) {
    items.push(
      makeHatch(
        hachure(
          random,
          cylinderPolygon(cx, top, bottom, rx, ry, HATCH_INSET),
          HATCH_GAP,
          HATCH_DEGREES,
        ),
        fill,
      ),
    );
  }
  return items;
}

function queue(
  random: Random,
  rect: Rect,
  label: string,
  fill: Tone | null,
  tokens: PlanTokens,
): Item[] {
  const items: Item[] = [];
  if (label !== "") {
    items.push(
      makeText(rect.x + rect.w / 2, rect.y - LABEL_ABOVE, label, tokens.handGroup, "ink2"),
    );
  }
  items.push(...boxSides(random, rect, "ink", OUTLINE_WIDTH, 1));
  const slot = rect.w / QUEUE_SLOTS;
  for (let divider = 1; divider < QUEUE_SLOTS; divider += 1) {
    const x = rect.x + slot * divider;
    items.push(
      makeStroke(roughLine(random, x, rect.y, x, rect.y + rect.h, { overshoot: 2 }), "ink", 1.8),
    );
  }
  if (fill !== null) {
    const filled = rectPolygon(rect.x, rect.y, slot * QUEUE_FILLED_SLOTS, rect.h, HATCH_INSET);
    items.push(makeHatch(hachure(random, filled, HATCH_GAP, HATCH_DEGREES), fill));
  }
  return items;
}

/** A stick figure in the design's proportions (42 × 86), scaled to the rect; label below it. */
function actor(random: Random, rect: Rect, label: string, tokens: PlanTokens): Item[] {
  const sx = rect.w / 42;
  const sy = rect.h / 86;
  const cx = rect.x + rect.w / 2;
  const at = (dx: number, dy: number): Vec => [cx + dx * sx, rect.y + dy * sy];
  const line = (from: Vec, to: Vec, overshoot: number): Stroke =>
    makeStroke(
      roughLine(random, from[0], from[1], to[0], to[1], { overshoot }),
      "ink",
      OUTLINE_WIDTH,
    );
  const head = at(0, 13);
  const items: Item[] = [
    makeStroke(roughEllipse(random, head[0], head[1], 13 * sx, 13 * sy), "ink", OUTLINE_WIDTH, {
      kind: "ellipse",
    }),
    line(at(0, 27), at(1, 60), 2),
    line(at(-21, 41), at(21, 39), 3),
    line(at(1, 60), at(-13, 86), 2),
    line(at(1, 60), at(15, 86), 2),
  ];
  if (label !== "") {
    items.push(makeText(cx, rect.y + rect.h + 24 * sy, label, tokens.handNode, "ink"));
  }
  return items;
}

/** A state as a sketch draws it: a box with fully rounded ends, its label, then its fill. */
function pill(
  random: Random,
  rect: Rect,
  label: string,
  fill: Tone | null,
  tokens: PlanTokens,
): Item[] {
  const { x, y, w, h } = rect;
  const outline = roughRoundedRect(random, x, y, w, h, h / 2);
  const items: Item[] = [makeStroke(outline.points, "ink", OUTLINE_WIDTH, { legs: outline.legs })];
  if (label !== "") {
    items.push(makeText(x + w / 2, y + h / 2, label, labelSize(tokens, rect, label), "ink"));
  }
  if (fill !== null) {
    items.push(
      makeHatch(
        hachure(random, roundedRectPolygon(x, y, w, h, h / 2), HATCH_GAP, HATCH_DEGREES),
        fill,
      ),
    );
  }
  return items;
}

function group(random: Random, rect: Rect, label: string, tokens: PlanTokens): Item[] {
  const items: Item[] = boxSides(random, rect, "grey", GROUP_WIDTH, GROUP_PACE);
  if (label !== "") {
    items.push(
      makeText(rect.x + rect.w / 2, rect.y - LABEL_ABOVE, label, tokens.handGroup, "ink2"),
    );
  }
  return items;
}

function edgeItems(
  op: Extract<DrawOp, { kind: "Edge" }>,
  random: Random,
  tokens: PlanTokens,
): Item[] {
  const { start, control, end } = op.path;
  const dashed = op.style === "Dashed";
  const items: Item[] = [];
  let labelAt: Vec;
  let labelAlign: "center" | "left" = "center";
  if (control === null) {
    items.push(
      makeStroke(
        roughLine(random, start.x, start.y, end.x, end.y, { overshoot: 2 }),
        "ink",
        CONNECTOR_WIDTH,
        { dashed },
      ),
      makeStroke(
        arrowHead(random, end.x, end.y, Math.atan2(end.y - start.y, end.x - start.x)),
        "ink",
        CONNECTOR_WIDTH,
        {
          kind: "head",
          legs: [0, 2, 4],
        },
      ),
    );
    const middle: Vec = [(start.x + end.x) / 2, (start.y + end.y) / 2];
    if (Math.abs(end.x - start.x) >= Math.abs(end.y - start.y)) {
      labelAt = [middle[0], middle[1] - EDGE_LABEL_GAP];
    } else {
      labelAt = [middle[0] + EDGE_LABEL_GAP - 2, middle[1]];
      labelAlign = "left";
    }
  } else {
    items.push(
      makeStroke(
        roughCurve(random, start.x, start.y, control.x, control.y, end.x, end.y),
        "ink",
        CONNECTOR_WIDTH,
        {
          kind: "curve",
          dashed,
        },
      ),
      makeStroke(
        arrowHead(random, end.x, end.y, Math.atan2(end.y - control.y, end.x - control.x)),
        "ink",
        CONNECTOR_WIDTH,
        {
          kind: "head",
          legs: [0, 2, 4],
        },
      ),
    );
    // The label sits on the outside of the bend, off the curve's midpoint.
    const middle: Vec = [
      0.25 * start.x + 0.5 * control.x + 0.25 * end.x,
      0.25 * start.y + 0.5 * control.y + 0.25 * end.y,
    ];
    const chord: Vec = [(start.x + end.x) / 2, (start.y + end.y) / 2];
    const bend = Math.hypot(middle[0] - chord[0], middle[1] - chord[1]) || 1;
    // Below a downward bend the label needs room for its own height, as in the design.
    const gap = EDGE_LABEL_GAP + 4 + (middle[1] > chord[1] ? tokens.handEdge * 0.75 : 0);
    labelAt = [
      middle[0] + ((middle[0] - chord[0]) / bend) * gap,
      middle[1] + ((middle[1] - chord[1]) / bend) * gap,
    ];
  }
  if (op.label !== null && op.label !== "") {
    items.push(makeText(labelAt[0], labelAt[1], op.label, tokens.handEdge, "ink2", labelAlign));
  }
  return items;
}

function annotateItems(
  op: Extract<DrawOp, { kind: "Annotate" }>,
  random: Random,
  tokens: PlanTokens,
  round: boolean,
): Item[] {
  const { x, y, w, h } = op.rect;
  const cx = x + w / 2;
  const cy = y + h / 2;
  const items: Item[] = [];
  let textAt: Vec = [x + w + ANNOTATION_PAD * 2, cy];
  let textAlign: "center" | "left" = "left";
  let textTone: Tone = "annotate";
  switch (op.mark) {
    case "Highlight": {
      // A shade under the ink in the core's tone, like the canvas's tinted pages (Ex-SSD), inside
      // a round target's outline; its note is written above it in the second ink, clear of a
      // row's neighbouring cells, as the design labels those pages.
      const outline = round
        ? ellipsePolygon(cx, cy, w / 2, h / 2, HATCH_INSET)
        : rectPolygon(x, y, w, h, HATCH_INSET);
      items.push(
        makeHatch(hachure(random, outline, HATCH_GAP, HATCH_DEGREES), toneOf(op.fill) ?? "blush"),
      );
      textAt = [cx, y - LABEL_ABOVE];
      textAlign = "center";
      textTone = "ink2";
      break;
    }
    case "Circle": {
      const rx = (w / 2) * 1.08 + ANNOTATION_PAD;
      const ry = (h / 2) * 1.08 + ANNOTATION_PAD * 0.6;
      items.push(
        makeStroke(
          roughEllipse(random, cx, cy, rx, ry, {
            start: -2.2,
            span: Math.PI * 2 + 0.5,
            grow: 0.06,
          }),
          "annotate",
          2,
          {
            kind: "ellipse",
          },
        ),
      );
      textAt = [cx + rx + ANNOTATION_PAD, cy];
      break;
    }
    case "Underline": {
      items.push(
        makeStroke(
          roughLine(random, x, y + h + 4, x + w, y + h + 3, { overshoot: 3, bow: 1.5 }),
          "annotate",
          2.2,
        ),
      );
      textAt = [cx, y + h + 4 + tokens.handAnnotation];
      textAlign = "center";
      break;
    }
    case "Arrow": {
      const tail: Vec = [x + w + 64, cy];
      const tip: Vec = [x + w + 14, cy];
      items.push(
        makeStroke(
          roughLine(random, tail[0], tail[1], tip[0], tip[1], { overshoot: 2 }),
          "annotate",
          1.9,
        ),
        makeStroke(arrowHead(random, tip[0], tip[1], Math.PI), "annotate", 1.9, {
          kind: "head",
          legs: [0, 2, 4],
        }),
      );
      textAt = [tail[0] + ANNOTATION_PAD + 4, cy];
      break;
    }
    case "Bracket": {
      const left = x + w + 6;
      const right = x + w + 14;
      const top = roughLine(random, left, y, right, y, { overshoot: 1 });
      const side = roughLine(random, right, y, right, y + h, { overshoot: 1 });
      const bottom = roughLine(random, right, y + h, left, y + h, { overshoot: 1 });
      const points = [...top, ...side.slice(1), ...bottom.slice(1)];
      items.push(
        makeStroke(points, "annotate", 2, {
          legs: [0, top.length - 1, top.length + side.length - 2, points.length - 1],
        }),
      );
      textAt = [right + ANNOTATION_PAD, cy];
      break;
    }
  }
  if (op.text !== null && op.text !== "") {
    items.push(makeText(textAt[0], textAt[1], op.text, tokens.handAnnotation, textTone, textAlign));
  }
  return items;
}

/** The label a strike crosses out, as its element's plan wrote it. */
export interface StruckLabel {
  readonly x: number;
  readonly y: number;
  readonly left: number;
  readonly right: number;
  readonly size: number;
  readonly tone: Tone;
  readonly align: TextAlign;
}

/** The handwritten label of an element's plan (its first text), or `null` when it has none. */
export function labelOf(plan: OpPlan): StruckLabel | null {
  for (const item of plan.items) {
    if (item.type === "text") {
      return {
        x: item.x,
        y: item.y,
        left: item.start[0],
        right: item.end[0],
        size: item.size,
        tone: item.tone,
        align: item.align,
      };
    }
  }
  return null;
}

/**
 * Components 10b's correction, in label sizes: the new label is written 1.92 sizes above the old
 * one (50 px over a 26 px label), with the caret between them at 0.62–0.85 sizes under the new
 * label's middle (92–98 px).
 */
const REWRITE_RISE = 1.92;
const CARET_TOP = 0.62;
const CARET_BOTTOM = 0.85;

/**
 * Strike through the old label in orange, then write the new one above it (Components 10b, D-032).
 * The core's rect is the element's box; the label is where the element's own plan wrote it
 * (inside a service, above a group or a queue, under an actor), or the rect's middle when the
 * element isn't drawn here. A label inside or under the drawing is rewritten above its box, where
 * a group's label sits, so the new words never land on the outline.
 */
function strikeItems(
  op: Extract<DrawOp, { kind: "Strike" }>,
  random: Random,
  tokens: PlanTokens,
  struck: StruckLabel | null,
): Item[] {
  const { x, y, w, h } = op.rect;
  const label = struck ?? {
    x: x + w / 2,
    y: y + h / 2,
    left: x,
    right: x + w,
    size: tokens.handNode,
    tone: "ink",
    align: "center",
  };
  const rewriteY = Math.min(label.y - REWRITE_RISE * label.size, y - LABEL_ABOVE);
  const lineX = label.left - 6;
  const caretTop = rewriteY + CARET_TOP * label.size;
  const caretBottom = rewriteY + CARET_BOTTOM * label.size;
  return [
    makeStroke(
      roughLine(random, lineX, label.y, label.right + 6, label.y - 3, { overshoot: 3, bow: 1 }),
      "annotate",
      2.4,
    ),
    makeStroke(
      [
        [lineX + 8, caretBottom],
        [lineX + 14, caretTop],
        [lineX + 21, caretBottom],
      ],
      "annotate",
      1.8,
      { kind: "head", legs: [0, 1, 2] },
    ),
    makeText(label.x, rewriteY, op.label, label.size, label.tone, label.align),
  ];
}

function boundsOf(items: readonly Item[], fallback: Rect): Rect {
  let left = fallback.x;
  let top = fallback.y;
  let right = fallback.x + fallback.w;
  let bottom = fallback.y + fallback.h;
  for (const item of items) {
    if (item.type === "text") {
      const half = item.size * 0.6;
      left = Math.min(left, item.start[0]);
      right = Math.max(right, item.end[0]);
      top = Math.min(top, item.y - half);
      bottom = Math.max(bottom, item.y + half);
      continue;
    }
    for (let index = 0; index < item.points.length; index += 2) {
      const px = item.points[index] ?? 0;
      const py = item.points[index + 1] ?? 0;
      left = Math.min(left, px);
      right = Math.max(right, px);
      top = Math.min(top, py);
      bottom = Math.max(bottom, py);
    }
  }
  return { x: left, y: top, w: right - left, h: bottom - top };
}

/** The label a target inside a drawing (a cell) shows, for a strike to cross. */
export function labelOfTarget(plan: OpPlan, id: string): StruckLabel | null {
  const label = plan.targets.find((target) => target.id === id)?.label ?? null;
  return label === null
    ? null
    : {
        x: label.x,
        y: label.y,
        left: label.start[0],
        right: label.end[0],
        size: label.size,
        tone: label.tone,
        align: label.align,
      };
}

/** The element id that seeds an op's randomness; a strike or a write has none, so it uses its
 * target's. */
export function seedKey(op: DrawOp): string {
  switch (op.kind) {
    case "Strike":
      return `${op.target}#strike`;
    case "Write":
      return `${op.target}#write`;
    case "Node":
    case "Edge":
    case "Annotate":
    case "Trace":
    case "Participant":
    case "Message":
    case "Cells":
    case "Reveal":
      return op.id;
  }
}

/** The hachure tone the core chose for a node; the UI never picks one itself (D-031 retired). */
export function toneOf(fill: Fill | null): Tone | null {
  switch (fill) {
    case "Lilac":
      return "lilac";
    case "Sky":
      return "sky";
    case "Mint":
      return "mint";
    case "Blush":
      return "blush";
    case null:
      return null;
  }
}

/**
 * `struck` is the label a strike crosses out: its target's, from the board (`labelOf`); `round`
 * says a highlight's target is a circle, so its shade stays inside the outline.
 */
export function planOp(
  op: DrawOp,
  tokens: PlanTokens,
  struck: StruckLabel | null = null,
  round = false,
): OpPlan {
  const random = randomFor(seedKey(op));
  let items: Item[];
  let fallback: Rect;
  let targets: readonly PlanTarget[] = [];
  switch (op.kind) {
    case "Node": {
      fallback = op.rect;
      const fill = toneOf(op.fill);
      // The line from a tree node's parent comes first, drawing the pen down to the node.
      const link = op.link === null ? [] : [linkItem(op.link, random)];
      switch (op.shape) {
        case "Service":
          items = service(random, op.rect, op.label, op.stack, fill, tokens);
          break;
        case "Db":
          items = cylinder(random, op.rect, op.label, fill, tokens);
          break;
        case "Queue":
          items = queue(random, op.rect, op.label, fill, tokens);
          break;
        case "Actor":
          items = actor(random, op.rect, op.label, tokens);
          break;
        case "Group":
          items = group(random, op.rect, op.label, tokens);
          break;
        case "Note":
          items = [
            makeText(
              op.rect.x + op.rect.w / 2,
              op.rect.y + op.rect.h / 2,
              op.label,
              tokens.handNode,
              "ink",
            ),
          ];
          break;
        case "Circle":
          items = circleItems(random, op.rect, op.label, fill, tokens);
          break;
        case "Pill":
          items = pill(random, op.rect, op.label, fill, tokens);
          break;
      }
      items = [...link, ...items];
      break;
    }
    case "Edge":
      items = edgeItems(op, random, tokens);
      fallback = { x: op.path.start.x, y: op.path.start.y, w: 0, h: 0 };
      break;
    case "Annotate":
      items = annotateItems(op, random, tokens, round);
      fallback = op.rect;
      break;
    case "Strike":
      items = strikeItems(op, random, tokens, struck);
      fallback = op.rect;
      break;
    case "Trace":
      items = [];
      fallback = traceBounds(op.points);
      break;
    case "Participant":
      items = participantItems(op, random, tokens);
      fallback = op.rect;
      break;
    case "Message":
      items = messageItems(op, random, tokens);
      fallback = traceBounds(op.points);
      break;
    case "Cells": {
      const cells = cellsItems(op, random, tokens);
      items = cells.items;
      targets = cells.targets;
      fallback = op.cells[0]?.rect ?? { x: 0, y: 0, w: 0, h: 0 };
      break;
    }
    case "Write":
      items = writeItems(op, tokens);
      fallback = op.rect;
      break;
    case "Reveal": {
      // Each part is seeded by its own id, so a part drawn by another reveal looks the same.
      const staged = revealItems(op, tokens);
      items = staged.items;
      targets = staged.targets;
      fallback = staged.targets[0]?.rect ?? { x: 0, y: 0, w: 0, h: 0 };
      break;
    }
  }
  const { starts, ends, naturalMs } = schedule(items);
  return { items, starts, ends, naturalMs, bounds: boundsOf(items, fallback), targets };
}

function traceBounds(points: readonly { x: number; y: number }[]): Rect {
  const first = points[0];
  if (first === undefined) {
    return { x: 0, y: 0, w: 0, h: 0 };
  }
  let left = first.x;
  let top = first.y;
  let right = first.x;
  let bottom = first.y;
  for (const point of points) {
    left = Math.min(left, point.x);
    right = Math.max(right, point.x);
    top = Math.min(top, point.y);
    bottom = Math.max(bottom, point.y);
  }
  return { x: left, y: top, w: right - left, h: bottom - top };
}

/** The path a trace dot follows: the op's points as one polyline. */
export function traceStroke(points: readonly { x: number; y: number }[]): Stroke {
  const path: Vec[] = points.map((point) => [point.x, point.y]);
  if (path.length === 1) {
    const only = path[0];
    if (only !== undefined) {
      path.push([only[0], only[1]]);
    }
  }
  return makeStroke(
    path.length === 0
      ? [
          [0, 0],
          [0, 0],
        ]
      : path,
    "signal",
    0,
  );
}
