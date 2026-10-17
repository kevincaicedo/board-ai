// A detail layer's card (15b · drill-down, D-188): the layer fills the board's area, from
// --detail-card-side to the stage's other side and from --detail-card-top to --detail-card-bottom
// above the bottom, and its drawing fits inside it one unit per pixel, as a detour card's does
// (D-151). It grows out of its anchor: over a scene duration the card's box eases from the
// anchor's box on screen to the full card, and the board under it steps back around the anchor
// (blurred, faded and shrunk as 15b); its close plays the same backwards.
//
// Pure functions of session time, so a replay seek lands on the same frame.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { FitArea, Viewport } from "./camera.ts";
import type { DetourCard } from "./detourCard.ts";
import { easeOut, lerp, ramp } from "./math.ts";
import type { Transform } from "./presence.ts";

export type DetailTokens = Pick<
  Tokens,
  | "detailCardSide"
  | "detailCardTop"
  | "detailCardBottom"
  | "detailBoardInset"
  | "cameraScaleMax"
  | "durationSceneMs"
>;

/** A box on the stage, in CSS pixels. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The card's box on this stage, at its full size. */
export function detailBox(stage: Viewport, tokens: DetailTokens, out: Box): Box {
  out.x = tokens.detailCardSide;
  out.y = tokens.detailCardTop;
  out.w = Math.max(0, Math.round(stage.width - 2 * tokens.detailCardSide));
  out.h = Math.max(0, Math.round(stage.height - tokens.detailCardTop - tokens.detailCardBottom));
  return out;
}

/** The card as the renderer sizes a layer's view: its full size, its drawing inset inside it. */
export function detailCard(stage: Viewport, tokens: DetailTokens, out: DetourCard): DetourCard {
  const box = detailBox(stage, tokens, scratchBox);
  out.width = box.w;
  out.height = box.h;
  const area: FitArea = out.area;
  area.x = tokens.detailBoardInset;
  area.y = tokens.detailBoardInset;
  area.w = Math.max(0, box.w - 2 * tokens.detailBoardInset);
  area.h = Math.max(0, box.h - 2 * tokens.detailBoardInset);
  area.maxScale = tokens.cameraScaleMax;
  return out;
}

const scratchBox: Box = { x: 0, y: 0, w: 0, h: 0 };

/** How far the detail is open at `at`, from 0 (in its anchor) to 1 (filling the board's area). */
export function detailOpen(
  openAt: number,
  closedAt: number | null,
  at: number,
  durationMs: number,
): number {
  const enter = easeOut(ramp(at, openAt, openAt + durationMs));
  const exit = closedAt === null ? 0 : easeOut(ramp(at, closedAt, closedAt + durationMs));
  return enter * (1 - exit);
}

/** Where a world rect of a layer lands on the stage under that layer's transform. */
export function screenBox(rect: Rect, transform: Transform, out: Box): Box {
  out.x = rect.x * transform.scale + transform.x;
  out.y = rect.y * transform.scale + transform.y;
  out.w = rect.w * transform.scale;
  out.h = rect.h * transform.scale;
  return out;
}

/** The card's box `open` of the way from its anchor's box to its full box. */
export function boxAt(from: Box, to: Box, open: number, out: Box): Box {
  out.x = lerp(from.x, to.x, open);
  out.y = lerp(from.y, to.y, open);
  out.w = lerp(from.w, to.w, open);
  out.h = lerp(from.h, to.h, open);
  return out;
}
