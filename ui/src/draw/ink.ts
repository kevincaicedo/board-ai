// Your shapes in your ink (13 · You draw), with the stroke engine's hand: a box's four rough
// sides, an arrow's rough line and open head, and pen strokes exactly as drawn (smoothed through
// their points, never roughened). Labels are handwritten inside a box or pen shape and above an
// arrow; a note goes under the shape, where the core measures it (board::user::area). Seeded by
// the shape's id, so a re-sent shape (a label landed) draws the same strokes.

import { arrowHead, roughLine } from "../canvas/geometry.ts";
import { randomFor } from "../canvas/rng.ts";
import { boxSides, labelSize, type PlanTokens } from "../canvas/sketch.ts";
import { makeStroke, type TextAlign } from "../canvas/stroke.ts";
import type { Point } from "../protocol/generated/Point.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import type { Tokens } from "../styles/tokens.ts";

/** Where an arrow's label sits above its middle: the core's `ARROW_LABEL_ABOVE`. */
export const ARROW_LABEL_ABOVE = 20;
/** Where a note's middle sits below its shape: the core's `NOTE_BELOW`. */
export const NOTE_BELOW = 18;
/** Room around a shape that still points at it. */
const HIT_PAD = 6;

export type InkTokens = PlanTokens & Pick<Tokens, "handNote" | "inkYouWidth">;

export type InkPiece =
  | { readonly type: "path"; readonly d: string; readonly width: number }
  | {
      readonly type: "text";
      readonly x: number;
      readonly y: number;
      readonly text: string;
      readonly size: number;
      readonly align: TextAlign;
      /** A note is written lighter (`--hand-note-opacity`). */
      readonly note: boolean;
    };

export interface InkPlan {
  readonly pieces: readonly InkPiece[];
  /** What points at the shape: its box, or the arrow's span. */
  readonly bounds: Rect;
}

/** The shape's box for the hint and the hit target: a box, a pen shape's bounds, an arrow's span. */
export function shapeBounds(shape: UserShape): Rect {
  switch (shape.kind.kind) {
    case "Box":
      return shape.kind.rect;
    case "Ink":
      return shape.kind.bounds;
    case "Arrow": {
      const { start, end } = shape.kind;
      return {
        x: Math.min(start.x, end.x),
        y: Math.min(start.y, end.y),
        w: Math.abs(end.x - start.x),
        h: Math.abs(end.y - start.y),
      };
    }
  }
}

export function planYours(shape: UserShape, tokens: InkTokens): InkPlan {
  const random = randomFor(shape.id);
  const width = tokens.inkYouWidth;
  const pieces: InkPiece[] = [];
  const text = (
    x: number,
    y: number,
    words: string,
    size: number,
    align: TextAlign,
    note: boolean,
  ): void => {
    pieces.push({ type: "text", x, y, text: words, size, align, note });
  };
  const kind = shape.kind;
  switch (kind.kind) {
    case "Box":
      for (const side of boxSides(random, kind.rect, "you", width, 1)) {
        pieces.push({ type: "path", d: side.path, width });
      }
      break;
    case "Ink":
      for (const stroke of kind.strokes) {
        pieces.push({ type: "path", d: smoothPath(stroke), width });
      }
      break;
    case "Arrow": {
      const { start, end } = kind;
      const line = roughLine(random, start.x, start.y, end.x, end.y, { overshoot: 2 });
      const head = arrowHead(random, end.x, end.y, Math.atan2(end.y - start.y, end.x - start.x));
      pieces.push({ type: "path", d: makeStroke(line, "you", width).path, width });
      pieces.push({ type: "path", d: makeStroke(head, "you", width).path, width });
      const middle = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 };
      if (shape.label !== null) {
        text(middle.x, middle.y - ARROW_LABEL_ABOVE, shape.label, tokens.handEdge, "center", false);
      }
      if (shape.note !== null) {
        text(middle.x, middle.y + NOTE_BELOW, shape.note, tokens.handNote, "center", true);
      }
      return { pieces, bounds: grow(shapeBounds(shape), HIT_PAD) };
    }
  }
  const bounds = shapeBounds(shape);
  if (shape.label !== null) {
    const middle = { x: bounds.x + bounds.w / 2, y: bounds.y + bounds.h / 2 };
    text(middle.x, middle.y, shape.label, labelSize(tokens, bounds, shape.label), "center", false);
  }
  if (shape.note !== null) {
    text(bounds.x, bounds.y + bounds.h + NOTE_BELOW, shape.note, tokens.handNote, "left", true);
  }
  return { pieces, bounds: grow(bounds, HIT_PAD) };
}

/**
 * A freehand stroke as drawn: a curve through the midpoints between samples, so the polyline's
 * corners soften the way a pen's do, without moving away from the points.
 */
export function smoothPath(points: readonly Point[]): string {
  const first = points[0];
  if (first === undefined) {
    return "";
  }
  let path = `M${first.x} ${first.y}`;
  if (points.length === 1) {
    return `${path}L${first.x} ${first.y}`;
  }
  for (let index = 1; index < points.length - 1; index += 1) {
    const point = points[index];
    const next = points[index + 1];
    if (point === undefined || next === undefined) {
      continue;
    }
    path += `Q${point.x} ${point.y} ${(point.x + next.x) / 2} ${(point.y + next.y) / 2}`;
  }
  const last = points[points.length - 1] ?? first;
  return `${path}L${last.x} ${last.y}`;
}

function grow(rect: Rect, by: number): Rect {
  return { x: rect.x - by, y: rect.y - by, w: rect.w + 2 * by, h: rect.h + 2 * by };
}
