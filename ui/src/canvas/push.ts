// How the main board steps back (frame 4, 15b, D-145): blurred, faded and nudged left while a
// detour is open; blurred, faded and shrunk around the part a detail layer drilled into while
// one is open; faded out for good once the session ended. Pure numbers from session time, written
// to the board's host by the renderer only when they change.

import type { Tokens } from "../styles/tokens.ts";
import type { BoardModel } from "./board.ts";
import { type Box, detailOpen, screenBox } from "./detail.ts";
import { easeOut, ramp, round1, round2 } from "./math.ts";
import type { Transform } from "./presence.ts";

export type PushTokens = Pick<
  Tokens,
  | "durationSceneMs"
  | "detourPushBlur"
  | "detourPushShift"
  | "detourPushShrink"
  | "detourPushFade"
  | "surfaceBlurOut"
  | "detailPushBlur"
  | "detailPushFade"
  | "detailPushShrink"
>;

/** The push-back's slots: the detour's amount, the end's, the detail's, and its origin. */
const DETOUR = 0;
const ENDED = 1;
const DETAIL = 2;
const ORIGIN_X = 3;
const ORIGIN_Y = 4;

function rounded(amount: number): number {
  return amount <= 0.001 ? 0 : Math.round(amount * 1000) / 1000;
}

/**
 * The push-back at `at` into `out` (five numbers, see the slots). `lifted`: you draw on the main
 * board, which then stays sharp under a detour (13 · You draw). `anchor` is scratch for the
 * detail's anchor box on the stage.
 */
export function boardPush(
  model: BoardModel,
  at: number,
  tokens: PushTokens,
  lifted: boolean,
  mainTransform: Transform,
  anchor: Box,
  out: Float64Array,
): Float64Array {
  let detour = 0;
  for (let index = model.detours.length - 1; index >= 0; index -= 1) {
    const span = model.detours[index];
    if (span === undefined || span.openAt > at) {
      continue;
    }
    const rise = easeOut(ramp(at, span.openAt, span.openAt + tokens.durationSceneMs));
    const fall =
      span.closeAt === null || at < span.closeAt
        ? 0
        : easeOut(ramp(at, span.closeAt, span.closeAt + tokens.durationSceneMs));
    detour = rise * (1 - fall);
    break;
  }
  let detail = 0;
  for (const layer of model.layers.values()) {
    const drilled = layer.anchor;
    if (drilled === null || drilled.layer !== 0 || layer.id === 0) {
      continue;
    }
    const open = detailOpen(layer.pushedAt, layer.poppedAt, at, tokens.durationSceneMs);
    if (open > detail) {
      detail = open;
      screenBox(drilled.rect, mainTransform, anchor);
    }
  }
  out[DETOUR] = lifted ? 0 : rounded(detour);
  out[ENDED] = rounded(model.endedAmount(at, tokens.durationSceneMs));
  out[DETAIL] = rounded(detail);
  out[ORIGIN_X] = round1(anchor.x + anchor.w / 2);
  out[ORIGIN_Y] = round1(anchor.y + anchor.h / 2);
  return out;
}

/** Writes a push-back onto the board's host; nothing at all when nothing steps back. */
export function writePush(host: CSSStyleDeclaration, push: Float64Array, tokens: PushTokens): void {
  const detour = push[DETOUR] ?? 0;
  const ended = push[ENDED] ?? 0;
  const detail = push[DETAIL] ?? 0;
  if (detour === 0 && ended === 0 && detail === 0) {
    host.removeProperty("filter");
    host.removeProperty("opacity");
    host.removeProperty("transform");
    host.removeProperty("transform-origin");
    return;
  }
  // A detail layer steps the board back around the part it drilled into (15b).
  if (detail > 0) {
    host.transformOrigin = `${push[ORIGIN_X] ?? 0}px ${push[ORIGIN_Y] ?? 0}px`;
  } else {
    host.removeProperty("transform-origin");
  }
  const blur =
    detour * tokens.detourPushBlur + detail * tokens.detailPushBlur + ended * tokens.surfaceBlurOut;
  host.filter = `blur(${round1(blur)}px)`;
  host.opacity = String(
    round2(
      (1 - tokens.detourPushFade * detour) * (1 - tokens.detailPushFade * detail) * (1 - ended),
    ),
  );
  const scale = (1 - tokens.detourPushShrink * detour) * (1 - tokens.detailPushShrink * detail);
  host.transform = `translateX(${round1(-tokens.detourPushShift * detour)}px) scale(${Math.round(scale * 1000) / 1000})`;
}
