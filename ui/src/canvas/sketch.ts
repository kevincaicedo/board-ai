// The hand-drawn primitives every plan shares (Components.dc.html): a box's four sides, a
// label's size, and the stroke widths, hachure and label offsets the design uses.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import { roughLine } from "./geometry.ts";
import type { Random } from "./rng.ts";
import { makeStroke, type Stroke, type Tone } from "./stroke.ts";

export type PlanTokens = Pick<
  Tokens,
  | "handNode"
  | "handNodeSmall"
  | "handNodeSmallBelow"
  | "handGroup"
  | "handEdge"
  | "handAnnotation"
  | "handIndex"
>;

export const OUTLINE_WIDTH = 2.1;
export const CONNECTOR_WIDTH = 2;
export const GROUP_WIDTH = 1.6;
/** The design draws a group's frame quicker than a box: it is scaffolding, not the subject. */
export const GROUP_PACE = 0.55;
export const HATCH_GAP = 7;
export const HATCH_DEGREES = -41;
export const HATCH_INSET = 4;
export const LABEL_ABOVE = 21;
export const EDGE_LABEL_GAP = 20;

export function boxSides(
  random: Random,
  rect: Rect,
  tone: Tone,
  width: number,
  pace: number,
): Stroke[] {
  const { x, y, w, h } = rect;
  const side = (x1: number, y1: number, x2: number, y2: number, overshoot?: number): Stroke =>
    makeStroke(
      roughLine(random, x1, y1, x2, y2, overshoot === undefined ? {} : { overshoot }),
      tone,
      width,
      { pace },
    );
  return [
    side(x, y, x + w, y),
    side(x + w, y, x + w, y + h),
    side(x + w, y + h, x, y + h),
    side(x, y + h, x, y, 4),
  ];
}

export function labelSize(tokens: PlanTokens, rect: Rect, label: string): number {
  const preferred = rect.h < tokens.handNodeSmallBelow ? tokens.handNodeSmall : tokens.handNode;
  const letters = Math.max(1, Array.from(label).length);
  // Handwriting advances half its size per letter; keep a margin inside the box.
  const fitting = (rect.w - 16) / (letters * 0.5);
  return Math.max(12, Math.min(preferred, Math.floor(fitting)));
}
