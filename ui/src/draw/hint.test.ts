import assert from "node:assert/strict";
import { test } from "node:test";
import { hintPlacement, hintWords } from "./hint.ts";

const tokens = { drawHintLeft: 28, drawHintBelow: 20 };

test("the hint sits under the shape it will label, where artboard 13 puts it", () => {
  // Artboard 13's cache shape (548, 430 in its SVG, 80 px down the stage) and its pill at 520, 590.
  const cache = { x: 548, y: 510, w: 116, h: 60 };
  assert.deepEqual(hintPlacement(cache, { scale: 1, x: 0, y: 0 }, tokens), { left: 520, top: 590 });
  // Through a camera that moved and shrank the board.
  assert.deepEqual(hintPlacement(cache, { scale: 0.5, x: 100, y: 10 }, tokens), {
    left: 346,
    top: 315,
  });
});

test("the words trail in from an ellipsis, without a sentence's capital or closing mark", () => {
  assert.equal(hintWords("And this one is the cache."), "“…and this one is the cache”");
  assert.equal(hintWords("…and this one"), "“…and this one”");
  assert.equal(hintWords("API gateway"), "“…API gateway”", "an acronym keeps its case");
  assert.equal(hintWords("  "), "“…”");
});
