import assert from "node:assert/strict";
import { test } from "node:test";
import type { Vec } from "./geometry.ts";
import {
  lettersAt,
  makeHatch,
  makeStroke,
  makeText,
  maskRegion,
  pointAtLength,
  revealLength,
  revealLengthStepped,
  textOffset,
  travelMs,
} from "./stroke.ts";

const square: Vec[] = [
  [0, 0],
  [100, 0],
  [100, 100],
  [0, 100],
];

test("a stroke measures its length and writes its path once", () => {
  const stroke = makeStroke(square, "ink", 2);
  assert.equal(stroke.length, 300);
  assert.equal(stroke.path, "M0 0L100 0L100 100L0 100");
  assert.equal(stroke.durationMs, (0.05 + 300 / 760) * 1000);
});

test("reveal is monotonic, starts at 0 and ends at the full length", () => {
  const stroke = makeStroke(square, "ink", 2, { legs: [0, 1, 2, 3] });
  let previous = -1;
  for (let step = 0; step <= 100; step += 1) {
    const length = revealLength(stroke, step / 100);
    assert.ok(length >= previous - 1e-9, `monotonic at ${step}`);
    previous = length;
  }
  assert.equal(revealLength(stroke, 0), 0);
  assert.equal(revealLength(stroke, 1), 300);
});

test("pacing slows into corners: each leg ends slower than its middle", () => {
  const stroke = makeStroke(square, "ink", 2, { legs: [0, 1, 2, 3] });
  const legEnd = stroke.legTimes[1] ?? 0;
  const speed = (f: number): number => revealLength(stroke, f + 0.001) - revealLength(stroke, f);
  assert.ok(
    speed(legEnd / 2) > 2 * speed(legEnd - 0.003),
    "faster on the straight than into the corner",
  );
  // At each leg boundary the pen is exactly at the corner.
  assert.ok(Math.abs(revealLength(stroke, legEnd) - 100) < 1e-6);
});

test("reduced motion reveals whole legs", () => {
  const stroke = makeStroke(square, "ink", 2, { legs: [0, 1, 2, 3] });
  assert.equal(revealLengthStepped(stroke, 0.01), 0);
  assert.equal(revealLengthStepped(stroke, (stroke.legTimes[1] ?? 0) + 0.01), 100);
  assert.equal(revealLengthStepped(stroke, 1), 300);
});

test("points along the stroke are interpolated without allocating", () => {
  const stroke = makeStroke(square, "ink", 2);
  const out: Vec = [0, 0];
  assert.equal(pointAtLength(stroke, 150, out), out);
  assert.deepEqual(out, [100, 50]);
  assert.deepEqual(pointAtLength(stroke, -5, out), [0, 0]);
  assert.deepEqual(pointAtLength(stroke, 999, out), [0, 100]);
});

test("hachure durations are clamped to the design's range", () => {
  const short = makeHatch(
    [
      [0, 0],
      [10, 0],
    ],
    "sky",
  );
  assert.equal(short.durationMs, 240);
  assert.equal(short.kind, "hatch");
});

test("handwriting writes letter by letter, never splitting a letter", () => {
  const mark = makeText(100, 50, "API pods ×3", 20, "ink2");
  assert.equal(mark.count, 11);
  assert.equal(mark.letterWidth, 10);
  assert.deepEqual(mark.start, [45, 50]);
  assert.equal(lettersAt(mark, 0), 0);
  assert.equal(lettersAt(mark, 0.5), 5);
  assert.equal(lettersAt(mark, 1), 11);
  assert.equal(mark.text.slice(0, textOffset(mark, 10)), "API pods ×");
  assert.equal(mark.durationMs, (0.05 + 11 * 0.046) * 1000);
});

test("the pen's travel costs a beat plus distance", () => {
  assert.equal(travelMs([0, 0], [0, 0]), 30);
  assert.equal(travelMs([0, 0], [2600, 0]), 1030);
});

test("a dashed stroke's mask covers the whole stroke, however far from the origin it reaches", () => {
  // A frame's bottom side far below the viewport (world y ≈ 1092 under a scaled-down camera) was
  // clipped by the mask's default region of -10%…120% of the viewport.
  const far = makeStroke(
    [
      [22, 14],
      [1418, 14],
      [1418, 1092],
      [22, 1092],
    ],
    "slate",
    1.6,
    { dashed: true },
  );
  const region = maskRegion(far, 6);
  assert.deepEqual(region, { x: 16, y: 8, w: 1408, h: 1090 });
  assert.ok(region.y + region.h >= 1092 + 6 && region.x <= 22 - 6);
  // Fractional points round outward.
  assert.deepEqual(
    maskRegion(
      makeStroke(
        [
          [0.5, 0.5],
          [9.5, 2.25],
        ],
        "ink",
        1,
      ),
      1,
    ),
    {
      x: -1,
      y: -1,
      w: 12,
      h: 5,
    },
  );
});
