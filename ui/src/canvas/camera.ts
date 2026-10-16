// Camera follow (PRODUCT §7.1): the core names a frame in world units (the layer's drawing plus
// a margin, or a focused element), and the UI eases the view so that frame sits in the part of
// the stage the board owns: below the thread bar, above the captions. A small board is never
// enlarged past its natural size, so a focus frame centers its element rather than zooming in.
// Depth of field softens everything but the focused element.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { Camera } from "./board.ts";
import { easeOut, lerp, ramp } from "./math.ts";
import type { Transform } from "./presence.ts";

export interface Viewport {
  readonly width: number;
  readonly height: number;
}

/** Where a layer's frame is fitted, in the layer's screen space, and how far it may enlarge. */
export interface FitArea {
  x: number;
  y: number;
  w: number;
  h: number;
  maxScale: number;
}

export type AreaTokens = Pick<
  Tokens,
  | "boardTop"
  | "boardBottom"
  | "boardSide"
  | "detourBoardTop"
  | "detourBoardBottom"
  | "detourBoardSide"
  | "cameraScaleMax"
>;

export function emptyArea(): FitArea {
  return { x: 0, y: 0, w: 0, h: 0, maxScale: 1 };
}

/** The main board's area: the stage clear of the thread bar and the captions. */
export function stageArea(stage: Viewport, tokens: AreaTokens, out: FitArea): FitArea {
  out.x = tokens.boardSide;
  out.y = tokens.boardTop;
  out.w = Math.max(0, stage.width - 2 * tokens.boardSide);
  out.h = Math.max(0, stage.height - tokens.boardTop - tokens.boardBottom);
  out.maxScale = tokens.cameraScaleMax;
  return out;
}

/** A detour layer's area: its card's design space below the card head. */
export function cardArea(card: Viewport, tokens: AreaTokens, out: FitArea): FitArea {
  out.x = tokens.detourBoardSide;
  out.y = tokens.detourBoardTop;
  out.w = Math.max(0, card.width - 2 * tokens.detourBoardSide);
  out.h = Math.max(0, card.height - tokens.detourBoardTop - tokens.detourBoardBottom);
  out.maxScale = tokens.cameraScaleMax;
  return out;
}

/** The transform that fits `frame` inside the area, centered, keeping its aspect ratio. */
export function fitFrame(frame: Rect, area: FitArea, out: Transform): Transform {
  const cx = area.x + area.w / 2;
  const cy = area.y + area.h / 2;
  if (frame.w <= 0 || frame.h <= 0 || area.w <= 0 || area.h <= 0) {
    out.scale = 1;
    out.x = cx - frame.x;
    out.y = cy - frame.y;
    return out;
  }
  const scale = Math.min(area.maxScale, area.w / frame.w, area.h / frame.h);
  out.scale = scale;
  out.x = cx - (frame.x + frame.w / 2) * scale;
  out.y = cy - (frame.y + frame.h / 2) * scale;
  return out;
}

const fromScratch: Transform = { scale: 1, x: 0, y: 0 };
const toScratch: Transform = { scale: 1, x: 0, y: 0 };

/** The view at `at`: identity with no frame, else easing from the previous frame to the new one. */
export function cameraAt(
  camera: Camera,
  at: number,
  area: FitArea,
  durationMs: number,
  out: Transform,
): Transform {
  if (camera.frame === null) {
    out.scale = 1;
    out.x = 0;
    out.y = 0;
    return out;
  }
  fitFrame(camera.frame, area, toScratch);
  if (camera.previous === null) {
    out.scale = toScratch.scale;
    out.x = toScratch.x;
    out.y = toScratch.y;
    return out;
  }
  fitFrame(camera.previous, area, fromScratch);
  const f = easeOut(ramp(at, camera.at, camera.at + durationMs));
  out.scale = lerp(fromScratch.scale, toScratch.scale, f);
  out.x = lerp(fromScratch.x, toScratch.x, f);
  out.y = lerp(fromScratch.y, toScratch.y, f);
  return out;
}

/** How soft the unfocused elements are at `at`, from 0 (all sharp) to 1. */
export function depthOfField(camera: Camera, at: number, durationMs: number): number {
  const f = easeOut(ramp(at, camera.focusAt, camera.focusAt + durationMs));
  if (camera.focus !== null) {
    return camera.previousFocus === null ? f : 1;
  }
  return camera.previousFocus === null ? 0 : 1 - f;
}
