// Camera follow (PRODUCT §7.1): the core names a frame in world units, and the UI eases the view
// so that frame fills the viewport. Depth of field softens everything but the focused element.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Camera } from "./board.ts";
import { easeOut, lerp, ramp } from "./math.ts";
import type { Transform } from "./presence.ts";

export interface Viewport {
  readonly width: number;
  readonly height: number;
}

/** The transform that fits `frame` inside the viewport, centered, keeping its aspect ratio. */
export function fitFrame(frame: Rect, viewport: Viewport, out: Transform): Transform {
  if (frame.w <= 0 || frame.h <= 0) {
    out.scale = 1;
    out.x = viewport.width / 2 - frame.x;
    out.y = viewport.height / 2 - frame.y;
    return out;
  }
  const scale = Math.min(viewport.width / frame.w, viewport.height / frame.h);
  out.scale = scale;
  out.x = viewport.width / 2 - (frame.x + frame.w / 2) * scale;
  out.y = viewport.height / 2 - (frame.y + frame.h / 2) * scale;
  return out;
}

const fromScratch: Transform = { scale: 1, x: 0, y: 0 };
const toScratch: Transform = { scale: 1, x: 0, y: 0 };

/** The view at `at`: identity with no frame, else easing from the previous frame to the new one. */
export function cameraAt(
  camera: Camera,
  at: number,
  viewport: Viewport,
  durationMs: number,
  out: Transform,
): Transform {
  if (camera.frame === null) {
    out.scale = 1;
    out.x = 0;
    out.y = 0;
    return out;
  }
  fitFrame(camera.frame, viewport, toScratch);
  if (camera.previous === null) {
    out.scale = toScratch.scale;
    out.x = toScratch.x;
    out.y = toScratch.y;
    return out;
  }
  fitFrame(camera.previous, viewport, fromScratch);
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
