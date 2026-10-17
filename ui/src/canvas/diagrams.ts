// The diagram types' strokes (PRODUCT §7.1, D-120), in the order a hand draws them:
// - a participant: its head box, its label, then its dashed lifeline;
// - a message: the lifeline stretches it needs, the arrow, its head, then its label;
// - a row of cells: its name, the outline, the dividers, each value, then the index labels;
// - a circle node: the line from its parent first, then the circle, its label and its fill;
// - a written cell value.
// Lifelines are the design's light grey (UseCases "Following a flow": G3 at 1.3), returns the
// second ink and dashed, cells divided like the queue's slots (1.8), and index labels in the
// third ink. Where a label sits beside a drawing mirrors the core's `board::layout` constants,
// which measure the same places for the camera.

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { EdgePath } from "../protocol/generated/EdgePath.ts";
import type { Point } from "../protocol/generated/Point.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import {
  arrowHead,
  ellipsePolygon,
  hachure,
  roughEllipse,
  roughLine,
  type Vec,
} from "./geometry.ts";
import type { Random } from "./rng.ts";
import {
  boxSides,
  CONNECTOR_WIDTH,
  HATCH_DEGREES,
  HATCH_GAP,
  HATCH_INSET,
  LABEL_ABOVE,
  labelSize,
  OUTLINE_WIDTH,
  type PlanTokens,
} from "./sketch.ts";
import { type Item, makeHatch, makeStroke, makeText, type TextMark, type Tone } from "./stroke.ts";

/** A lifeline: the design's light grey, thin, dashed, drawn quicker than an outline. */
const LIFELINE_WIDTH = 1.3;
const LIFELINE_PACE = 0.6;
/** A reply arrow is lighter than a call. */
const RETURN_WIDTH = 1.6;
/** A cell's dividers, like a queue's slots. */
const DIVIDER_WIDTH = 1.8;
/** Mirrors of the core's `board::layout` label offsets. */
const MESSAGE_LABEL_ABOVE = 14;
const SELF_LABEL_GAP = 10;
const STRIP_LABEL_GAP = 14;
const INDEX_BELOW = 16;
const INDEX_LEFT_GAP = 10;

/** An element inside an op's drawing (a cell of a row): pointable, and its label strikable. */
export interface PlanTarget {
  readonly id: string;
  readonly rect: Rect;
  readonly label: TextMark | null;
}

function lifeline(random: Random, x: number, top: number, bottom: number): Item {
  return makeStroke(
    roughLine(random, x, top, x, bottom, { overshoot: 0 }),
    "grey",
    LIFELINE_WIDTH,
    {
      dashed: true,
      pace: LIFELINE_PACE,
    },
  );
}

export function participantItems(
  op: Extract<DrawOp, { kind: "Participant" }>,
  random: Random,
  tokens: PlanTokens,
): Item[] {
  const { rect } = op;
  const items: Item[] = boxSides(random, rect, "ink", OUTLINE_WIDTH, 1);
  if (op.label !== "") {
    items.push(
      makeText(
        rect.x + rect.w / 2,
        rect.y + rect.h / 2,
        op.label,
        labelSize(tokens, rect, op.label),
        "ink",
      ),
    );
  }
  items.push(lifeline(random, rect.x + rect.w / 2, rect.y + rect.h, op.lifeline_bottom));
  return items;
}

/** Points as one stroke, each segment a leg, so a loop turns its corners like a hand. */
function polyline(
  random: Random,
  points: readonly Point[],
  tone: Tone,
  width: number,
  dashed: boolean,
): Item {
  const path: Vec[] = [];
  const legs: number[] = [0];
  for (let index = 1; index < points.length; index += 1) {
    const from = points[index - 1];
    const to = points[index];
    if (from === undefined || to === undefined) {
      continue;
    }
    const segment = roughLine(random, from.x, from.y, to.x, to.y, { overshoot: 1 });
    path.push(...(path.length === 0 ? segment : segment.slice(1)));
    legs.push(path.length - 1);
  }
  return makeStroke(path, tone, width, { dashed, legs });
}

export function messageItems(
  op: Extract<DrawOp, { kind: "Message" }>,
  random: Random,
  tokens: PlanTokens,
): Item[] {
  const items: Item[] = op.extend.map((stretch) =>
    lifeline(random, stretch.x, stretch.top, stretch.bottom),
  );
  const reply = op.style === "Dashed";
  const tone: Tone = reply ? "ink2" : "ink";
  const width = reply ? RETURN_WIDTH : CONNECTOR_WIDTH;
  const end = op.points[op.points.length - 1];
  const before = op.points[op.points.length - 2];
  const first = op.points[0];
  if (end === undefined || before === undefined || first === undefined) {
    return items;
  }
  items.push(
    polyline(random, op.points, tone, width, reply),
    makeStroke(
      arrowHead(random, end.x, end.y, Math.atan2(end.y - before.y, end.x - before.x)),
      tone,
      width,
      { kind: "head", legs: [0, 2, 4] },
    ),
  );
  if (op.label !== null && op.label !== "") {
    if (op.points.length > 2) {
      const right = Math.max(...op.points.map((point) => point.x));
      items.push(
        makeText(
          right + SELF_LABEL_GAP,
          (first.y + end.y) / 2,
          op.label,
          tokens.handEdge,
          "ink2",
          "left",
        ),
      );
    } else {
      items.push(
        makeText(
          (first.x + end.x) / 2,
          first.y - MESSAGE_LABEL_ABOVE,
          op.label,
          tokens.handEdge,
          "ink2",
        ),
      );
    }
  }
  return items;
}

/** A row or column of cells, and each cell as a target (its box, and its value to strike). */
export function cellsItems(
  op: Extract<DrawOp, { kind: "Cells" }>,
  random: Random,
  tokens: PlanTokens,
): { items: Item[]; targets: PlanTarget[] } {
  const items: Item[] = [];
  const strip = op.cells.reduce<Rect | null>(
    (bounds, cell) => (bounds === null ? cell.rect : union(bounds, cell.rect)),
    null,
  );
  if (strip === null) {
    return { items, targets: [] };
  }
  if (op.label !== "") {
    items.push(
      op.vertical
        ? makeText(strip.x + strip.w / 2, strip.y - LABEL_ABOVE, op.label, tokens.handEdge, "ink2")
        : makeText(
            strip.x - STRIP_LABEL_GAP,
            strip.y + strip.h / 2,
            op.label,
            tokens.handEdge,
            "ink2",
            "right",
          ),
    );
  }
  items.push(...boxSides(random, strip, "ink", OUTLINE_WIDTH, 1));
  for (const cell of op.cells.slice(1)) {
    const { x, y, w } = cell.rect;
    items.push(
      makeStroke(
        op.vertical
          ? roughLine(random, x, y, x + w, y, { overshoot: 2 })
          : roughLine(random, x, y, x, y + cell.rect.h, { overshoot: 2 }),
        "ink",
        DIVIDER_WIDTH,
      ),
    );
  }
  const targets: PlanTarget[] = [];
  for (const cell of op.cells) {
    const { x, y, w, h } = cell.rect;
    let label: TextMark | null = null;
    if (cell.value !== "") {
      label = makeText(
        x + w / 2,
        y + h / 2,
        cell.value,
        labelSize(tokens, cell.rect, cell.value),
        "ink",
      );
      items.push(label);
    }
    targets.push({ id: cell.id, rect: cell.rect, label });
  }
  for (const cell of op.cells) {
    if (cell.index === null) {
      continue;
    }
    const { x, y, w, h } = cell.rect;
    items.push(
      op.vertical
        ? makeText(x - INDEX_LEFT_GAP, y + h / 2, cell.index, tokens.handIndex, "ink3", "right")
        : makeText(x + w / 2, y + h + INDEX_BELOW, cell.index, tokens.handIndex, "ink3"),
    );
  }
  return { items, targets };
}

/** A value written into an empty cell, as its own value would have been. */
export function writeItems(op: Extract<DrawOp, { kind: "Write" }>, tokens: PlanTokens): Item[] {
  const { x, y, w, h } = op.rect;
  return [makeText(x + w / 2, y + h / 2, op.label, labelSize(tokens, op.rect, op.label), "ink")];
}

/** The line from a tree node's parent, drawn before the node: the pen comes down to it. */
export function linkItem(link: EdgePath, random: Random): Item {
  const { start, end } = link;
  return makeStroke(
    roughLine(random, start.x, start.y, end.x, end.y, { overshoot: 1 }),
    "ink",
    CONNECTOR_WIDTH,
  );
}

/** A circle (an ellipse for a long label): tree nodes and states. */
export function circleItems(
  random: Random,
  rect: Rect,
  label: string,
  fill: Tone | null,
  tokens: PlanTokens,
): Item[] {
  const cx = rect.x + rect.w / 2;
  const cy = rect.y + rect.h / 2;
  const items: Item[] = [
    makeStroke(
      roughEllipse(random, cx, cy, rect.w / 2, rect.h / 2, { grow: 0.03 }),
      "ink",
      OUTLINE_WIDTH,
      {
        kind: "ellipse",
      },
    ),
  ];
  if (label !== "") {
    items.push(makeText(cx, cy, label, labelSize(tokens, rect, label), "ink"));
  }
  if (fill !== null) {
    items.push(
      makeHatch(
        hachure(
          random,
          ellipsePolygon(cx, cy, rect.w / 2, rect.h / 2, HATCH_INSET),
          HATCH_GAP,
          HATCH_DEGREES,
        ),
        fill,
      ),
    );
  }
  return items;
}

function union(a: Rect, b: Rect): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return {
    x,
    y,
    w: Math.max(a.x + a.w, b.x + b.w) - x,
    h: Math.max(a.y + a.h, b.y + b.h) - y,
  };
}
