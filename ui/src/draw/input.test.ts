import assert from "node:assert/strict";
import { test } from "node:test";
import { drawnShape, previewPath, StrokeSampler, screenToWorld, spanRect } from "./input.ts";
import { PEN_SAMPLE_MIN_PX, USER_STROKE_POINTS_MAX } from "./limits.ts";

const identity = { scale: 1, x: 0, y: 0 };

test("the sampler skips moves closer than its spacing and ends where the pointer came up", () => {
  const sampler = new StrokeSampler();
  sampler.begin(10, 10);
  assert.equal(sampler.add(11, 11), false, "within the spacing");
  assert.equal(sampler.add(10 + PEN_SAMPLE_MIN_PX, 10), true);
  // Coming up close to the last kept point moves that point there, adding none.
  sampler.finish(14, 11);
  assert.equal(sampler.count, 2);
  assert.deepEqual([sampler.x(1), sampler.y(1)], [14, 11]);
  // Coming up farther adds the point.
  sampler.finish(30, 11);
  assert.equal(sampler.count, 3);
  assert.deepEqual([sampler.x(2), sampler.y(2)], [30, 11]);
});

test("a long stroke never passes the core's bound, and keeps its start and its end", () => {
  const sampler = new StrokeSampler();
  sampler.begin(0, 0);
  for (let step = 1; step <= 5_000; step += 1) {
    sampler.add(step * 4, Math.sin(step / 50) * 100);
    assert.ok(sampler.count <= USER_STROKE_POINTS_MAX, `${sampler.count} points at ${step}`);
  }
  sampler.finish(20_004, 7);
  assert.ok(sampler.count <= USER_STROKE_POINTS_MAX);
  assert.ok(sampler.count > USER_STROKE_POINTS_MAX / 4, "decimation keeps the shape");
  assert.deepEqual([sampler.x(0), sampler.y(0)], [0, 0]);
  assert.deepEqual([sampler.x(sampler.count - 1), sampler.y(sampler.count - 1)], [20_004, 7]);
  const shape = drawnShape("pen", sampler, identity);
  assert.ok(shape?.tool === "Pen" && shape.points.length <= USER_STROKE_POINTS_MAX);
});

test("screen points become world units through the camera, rounded", () => {
  const camera = { scale: 0.5, x: 100, y: 40 };
  assert.deepEqual(screenToWorld(camera, 100, 40), { x: 0, y: 0 });
  assert.deepEqual(screenToWorld(camera, 201, 90.4), { x: 202, y: 101 });
  assert.deepEqual(screenToWorld(identity, 3.6, -2.4), { x: 4, y: -2 });
});

function gesture(points: readonly [number, number][]): StrokeSampler {
  const sampler = new StrokeSampler();
  const [first, ...rest] = points;
  if (first !== undefined) {
    sampler.begin(first[0], first[1]);
  }
  for (const [x, y] of rest) {
    sampler.add(x, y);
  }
  return sampler;
}

test("each tool makes its shape from the drag, and a click makes none", () => {
  const drag = gesture([
    [300, 200],
    [250, 260],
    [120, 140],
  ]);
  assert.deepEqual(drawnShape("box", drag, identity), {
    tool: "Box",
    rect: { x: 120, y: 140, w: 180, h: 60 },
  });
  assert.deepEqual(drawnShape("arrow", drag, identity), {
    tool: "Arrow",
    start: { x: 300, y: 200 },
    end: { x: 120, y: 140 },
  });
  assert.deepEqual(drawnShape("pen", drag, identity), {
    tool: "Pen",
    points: [
      { x: 300, y: 200 },
      { x: 250, y: 260 },
      { x: 120, y: 140 },
    ],
  });
  const click = gesture([
    [50, 50],
    [52, 53],
  ]);
  for (const tool of ["box", "arrow", "pen"] as const) {
    assert.equal(drawnShape(tool, click, identity), null, tool);
  }
  // A flat drag is a fine arrow but no box.
  const flat = gesture([
    [0, 0],
    [200, 2],
  ]);
  assert.equal(drawnShape("box", flat, identity), null);
  assert.equal(drawnShape("arrow", flat, identity)?.tool, "Arrow");
  assert.deepEqual(spanRect({ x: 5, y: 9 }, { x: 1, y: 2 }), { x: 1, y: 2, w: 4, h: 7 });
});

test("the preview follows the tool", () => {
  const drag = gesture([
    [10, 10],
    [40, 30],
  ]);
  assert.equal(previewPath("box", drag), "M10 10H40V30H10Z");
  assert.equal(previewPath("pen", drag), "M10 10L40 30");
  assert.match(previewPath("arrow", drag), /^M10 10L40 30M[\d. -]+L40 30L[\d. -]+$/);
  assert.equal(previewPath("pen", new StrokeSampler()), "");
});
