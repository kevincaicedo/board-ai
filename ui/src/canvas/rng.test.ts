import assert from "node:assert/strict";
import { test } from "node:test";
import { arrowHead, hachure, rectPolygon, roughEllipse, roughLine } from "./geometry.ts";
import { hashSeed, mulberry32, randomFor } from "./rng.ts";

test("the generator is deterministic per seed and matches the design's mulberry32", () => {
  const a = mulberry32(7);
  const b = mulberry32(7);
  const first = [a(), a(), a()];
  assert.deepEqual(first, [b(), b(), b()]);
  // Golden values from the design prototype's rng(7), so the port draws the same sequence.
  assert.deepEqual(
    first.map((value) => value.toFixed(9)),
    [0.011704753153026104, 0.06195825757458806, 0.97690763277933].map((value) => value.toFixed(9)),
  );
  const other = mulberry32(8);
  assert.notEqual(other(), first[0]);
});

test("every draw is in [0, 1)", () => {
  const random = mulberry32(123456);
  for (let index = 0; index < 10_000; index += 1) {
    const value = random();
    assert.ok(value >= 0 && value < 1);
  }
});

test("element ids hash to stable, distinct seeds", () => {
  assert.equal(hashSeed("redis"), hashSeed("redis"));
  assert.equal(hashSeed("cdn"), 4001137924);
  assert.notEqual(hashSeed("redis"), hashSeed("postgres"));
  assert.notEqual(hashSeed("api>redis"), hashSeed("redis>api"));
});

test("rough generators produce identical geometry for the same id and differ across ids", () => {
  const draw = (id: string): number[] => {
    const random = randomFor(id);
    return [
      ...roughLine(random, 0, 0, 120, 0).flat(),
      ...roughEllipse(random, 50, 50, 30, 20).flat(),
      ...arrowHead(random, 10, 10, 0).flat(),
      ...hachure(random, rectPolygon(0, 0, 100, 60, 4), 7, -41).flat(),
    ];
  };
  assert.deepEqual(draw("lb"), draw("lb"));
  assert.notDeepEqual(draw("lb"), draw("cdn"));
});

test("a rough line stays near its ideal segment and overshoots a little", () => {
  const points = roughLine(mulberry32(1), 0, 0, 100, 0);
  const first = points[0];
  const last = points[points.length - 1];
  assert.ok(first !== undefined && last !== undefined);
  assert.ok(Math.abs(first[0]) <= 7 && Math.abs(first[1]) <= 1);
  assert.ok(last[0] >= 100 && last[0] <= 108);
  for (const [, y] of points) {
    assert.ok(Math.abs(y) < 4, `bow stays small: ${y}`);
  }
});

test("hachure lines stay inside their polygon", () => {
  const points = hachure(mulberry32(3), rectPolygon(10, 20, 140, 76, 4), 7, -41);
  assert.ok(points.length > 20);
  for (const [x, y] of points) {
    assert.ok(x >= 12 && x <= 148 && y >= 22 && y <= 94, `${x},${y}`);
  }
});
