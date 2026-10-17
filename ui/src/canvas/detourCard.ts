// How big a detour's card is, from its drawing (D-188). Frame 4's card (the column's width,
// --detour-card-height tall) holds a small detour at its natural size, but a detour that grew
// shrank to fit: the owner's two larger detours drew at 0.42 and 0.25 of their size. So while
// the drawing reads at --detour-scale-min or more the card keeps frame 4's size (a small one is
// enlarged up to --detour-camera-scale-max); past it the card grows, taller first, then wider to
// the left, up to the board's area. The whole frame always fits (the core's I11 stays visible):
// a card at its largest shrinks the drawing below the minimum rather than crop it.
//
// Pure functions of the layer's camera and session time, eased with the camera, so a replay
// seek lands on the same card.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { Camera } from "./board.ts";
import type { FitArea, Viewport } from "./camera.ts";
import { easeOut, lerp, ramp } from "./math.ts";

export type DetourCardTokens = Pick<
  Tokens,
  | "surfaceWidth"
  | "surfaceRight"
  | "surfaceTop"
  | "detourCardHeight"
  | "boardSide"
  | "boardBottom"
  | "detourBoardTop"
  | "detourBoardBottom"
  | "detourBoardSide"
  | "detourScaleMin"
  | "detourCameraScaleMax"
  | "cameraScaleMax"
  | "durationSceneMs"
>;

/** A card's size in CSS pixels. */
export interface Size {
  width: number;
  height: number;
}

/** A detour card in CSS pixels, and where its layer fits its camera frame inside it. */
export interface DetourCard {
  width: number;
  height: number;
  area: FitArea;
}

export function emptyCard(): DetourCard {
  return { width: 0, height: 0, area: { x: 0, y: 0, w: 0, h: 0, maxScale: 1 } };
}

/** The column's width (stage.css: min(--surface-width, 50% − --surface-right)). */
export function columnWidth(stage: Viewport, tokens: DetourCardTokens): number {
  return Math.max(0, Math.min(tokens.surfaceWidth, stage.width / 2 - tokens.surfaceRight));
}

/**
 * Frame 4's card, and the largest a card may grow: from the column's right edge leftward to the
 * board's side margin, and from the column's top down to the board's bottom (the captions).
 */
export function cardBounds(stage: Viewport, tokens: DetourCardTokens, base: Size, max: Size): void {
  const column = columnWidth(stage, tokens);
  base.width = column;
  base.height = tokens.detourCardHeight;
  max.width = Math.max(column, stage.width / 2 + column - tokens.boardSide);
  max.height = Math.max(
    tokens.detourCardHeight,
    stage.height - tokens.surfaceTop - tokens.boardBottom,
  );
}

/** The scale `frame` gets inside a card of `width` × `height`, enlarging up to `maxScale`. */
function fitScale(frame: Rect, width: number, height: number, tokens: DetourCardTokens): number {
  const w = width - 2 * tokens.detourBoardSide;
  const h = height - tokens.detourBoardTop - tokens.detourBoardBottom;
  if (frame.w <= 0 || frame.h <= 0 || w <= 0 || h <= 0) {
    return 1;
  }
  return Math.min(w / frame.w, h / frame.h);
}

/** The card one frame asks for: frame 4's while the drawing reads, else just big enough. */
export function cardFor(
  frame: Rect | null,
  base: Viewport,
  max: Viewport,
  tokens: DetourCardTokens,
  out: Size,
): void {
  out.width = base.width;
  out.height = base.height;
  if (frame === null || fitScale(frame, base.width, base.height, tokens) >= tokens.detourScaleMin) {
    return;
  }
  const needHeight =
    frame.h * tokens.detourScaleMin + tokens.detourBoardTop + tokens.detourBoardBottom;
  const needWidth = frame.w * tokens.detourScaleMin + 2 * tokens.detourBoardSide;
  out.height = Math.min(max.height, Math.max(base.height, needHeight));
  out.width = Math.min(max.width, Math.max(base.width, needWidth));
}

const fromScratch: Size = { width: 0, height: 0 };
const toScratch: Size = { width: 0, height: 0 };

/** The card at `at`, easing from the previous frame's card to the new one's as the camera does. */
export function detourCardAt(
  camera: Camera,
  at: number,
  base: Viewport,
  max: Viewport,
  tokens: DetourCardTokens,
  out: DetourCard,
): DetourCard {
  cardFor(camera.frame, base, max, tokens, toScratch);
  if (camera.previous === null) {
    out.width = toScratch.width;
    out.height = toScratch.height;
  } else {
    cardFor(camera.previous, base, max, tokens, fromScratch);
    const f = easeOut(ramp(at, camera.at, camera.at + tokens.durationSceneMs));
    out.width = lerp(fromScratch.width, toScratch.width, f);
    out.height = lerp(fromScratch.height, toScratch.height, f);
  }
  // Whole pixels: the card's box and its layer's viewBox are written from these, and agree.
  out.width = Math.round(out.width);
  out.height = Math.round(out.height);
  const area = out.area;
  area.x = tokens.detourBoardSide;
  area.y = tokens.detourBoardTop;
  area.w = Math.max(0, out.width - 2 * tokens.detourBoardSide);
  area.h = Math.max(0, out.height - tokens.detourBoardTop - tokens.detourBoardBottom);
  // A focus centers its element, as on the main board; only a whole small detour is enlarged.
  area.maxScale =
    camera.focus === null && camera.previousFocus === null
      ? tokens.detourCameraScaleMax
      : tokens.cameraScaleMax;
  return out;
}
