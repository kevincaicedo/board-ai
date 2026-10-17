// A detour card sized from its drawing (D-188), checked on the owner's three detours of
// 2026-10-16: frame 4's card held the first one, and fit the other two at 0.42 and 0.25.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Rect } from "../protocol/generated/Rect.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import type { Camera } from "./board.ts";
import { fitFrame } from "./camera.ts";
import { cardBounds, cardFor, detourCardAt, emptyCard, type Size } from "./detourCard.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const DETOUR_ONE: Rect = { x: -27, y: -22, w: 436, h: 184 };
const DETOUR_TWO: Rect = { x: -21, y: -34, w: 1610, h: 208 };
const DETOUR_THREE: Rect = { x: -40, y: -72, w: 1876, h: 984 };
/** The canvas's frame 4 drawing: API, Redis and Postgres, about 512 × 208. */
const FRAME_FOUR: Rect = { x: 44, y: 66, w: 512, h: 208 };

function bounds(width: number, height: number): { base: Size; max: Size } {
  const base = { width: 0, height: 0 };
  const max = { width: 0, height: 0 };
  cardBounds({ width, height }, tokens, base, max);
  return { base, max };
}

function camera(frame: Rect | null, previous: Rect | null = null, at = 0): Camera {
  return { frame, previous, at, focus: null, previousFocus: null, focusAt: 0 };
}

function scaleIn(frame: Rect, at: number, width: number, height: number): number {
  const { base, max } = bounds(width, height);
  const card = detourCardAt(camera(frame), at, base, max, tokens, emptyCard());
  return fitFrame(frame, card.area, { scale: 0, x: 0, y: 0 }).scale;
}

test("frame 4's drawing keeps frame 4's card", () => {
  const { base, max } = bounds(1440, 900);
  assert.deepEqual(base, { width: tokens.surfaceWidth, height: tokens.detourCardHeight });
  const size = { width: 0, height: 0 };
  cardFor(FRAME_FOUR, base, max, tokens, size);
  assert.deepEqual(size, base);
  cardFor(null, base, max, tokens, size);
  assert.deepEqual(size, base, "a detour with nothing drawn yet");
});

test("the owner's detours read at the minimum scale where the stage has room", () => {
  // Before: 1.00, 0.42 and 0.25 in frame 4's card.
  assert.ok(scaleIn(DETOUR_ONE, 0, 1440, 900) > 1, "a small detour is enlarged");
  assert.ok(scaleIn(DETOUR_ONE, 0, 1440, 900) <= tokens.detourCameraScaleMax);
  assert.ok(Math.abs(scaleIn(DETOUR_TWO, 0, 1440, 900) - tokens.detourScaleMin) < 0.01);
  assert.ok(scaleIn(DETOUR_THREE, 0, 1440, 900) > 0.55, "at its largest on a 1440 stage");
  // The owner's window has room for all three at the minimum.
  for (const frame of [DETOUR_TWO, DETOUR_THREE]) {
    assert.ok(Math.abs(scaleIn(frame, 0, 2000, 1283) - tokens.detourScaleMin) < 0.01);
  }
});

test("a card grows taller first, then wider, and never past the board's area", () => {
  const { base, max } = bounds(1440, 900);
  const size = { width: 0, height: 0 };
  cardFor({ x: 0, y: 0, w: 400, h: 700 }, base, max, tokens, size);
  assert.equal(size.width, base.width, "a tall drawing needs no more width");
  assert.ok(size.height > base.height && size.height <= max.height);
  cardFor(DETOUR_TWO, base, max, tokens, size);
  assert.equal(size.height, base.height, "a wide, short drawing needs no more height");
  assert.ok(size.width > base.width && size.width <= max.width);
  cardFor({ x: 0, y: 0, w: 9000, h: 9000 }, base, max, tokens, size);
  assert.deepEqual(size, max);
  // The board's area: from the column's right edge to the side margin, above the captions.
  assert.equal(max.width, 1440 / 2 + tokens.surfaceWidth - tokens.boardSide);
  assert.equal(max.height, 900 - tokens.surfaceTop - tokens.boardBottom);
});

test("the whole frame always fits its card, even past the minimum scale (I11)", () => {
  const { base, max } = bounds(1440, 900);
  const huge: Rect = { x: -500, y: -300, w: 6000, h: 4000 };
  const card = detourCardAt(camera(huge), 0, base, max, tokens, emptyCard());
  const fit = fitFrame(huge, card.area, { scale: 0, x: 0, y: 0 });
  assert.ok(huge.w * fit.scale <= card.area.w + 0.5 && huge.h * fit.scale <= card.area.h + 0.5);
});

test("the card eases with the camera, and a focus is never enlarged", () => {
  const { base, max } = bounds(1440, 900);
  const growing = camera(DETOUR_TWO, FRAME_FOUR, 1_000);
  const start = detourCardAt(growing, 1_000, base, max, tokens, emptyCard());
  assert.equal(start.width, base.width);
  const middle = detourCardAt(
    growing,
    1_000 + tokens.durationSceneMs / 2,
    base,
    max,
    tokens,
    emptyCard(),
  );
  assert.ok(middle.width > base.width);
  const end = detourCardAt(growing, 1_000 + tokens.durationSceneMs, base, max, tokens, emptyCard());
  assert.ok(end.width > middle.width);
  assert.equal(end.area.maxScale, tokens.detourCameraScaleMax);
  const focused = detourCardAt(
    { ...camera(DETOUR_ONE), focus: "redis" },
    0,
    base,
    max,
    tokens,
    emptyCard(),
  );
  assert.equal(focused.area.maxScale, tokens.cameraScaleMax);
});
