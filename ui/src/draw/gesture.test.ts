import assert from "node:assert/strict";
import { test } from "node:test";
import type { Point } from "../protocol/generated/Point.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import { DrawingInput, type PointerInput } from "./gesture.ts";
import { StrokeSampler } from "./input.ts";
import { DrawModel } from "./model.ts";

const ORIGIN = { left: 0, top: 0 };

/** A surface over a drawing model at identity, recording what it sends. */
function surface() {
  const draw = new DrawModel();
  const sent: UiCommand[] = [];
  const input = new DrawingInput({
    draw,
    send: (command) => sent.push(command),
    transform: () => ({ scale: 1, x: 0, y: 0 }),
  });
  const unwatch = input.watch();
  const strokes = (): Point[][] =>
    sent.flatMap((command) =>
      command.type === "Draw" &&
      command.action.kind === "Shape" &&
      command.action.drawn.tool === "Pen"
        ? [command.action.drawn.points]
        : [],
    );
  return { draw, sent, input, unwatch, strokes };
}

function at(x: number, y: number, extra: Partial<PointerInput> = {}): PointerInput {
  return { pointerId: 1, button: 0, buttons: 1, clientX: x, clientY: y, ...extra };
}

// Review 2 (High): DrawingSurface stopped rendering when drawing ended, but its gesture stayed.
// The next pointerdown was ignored, the next stroke continued the old one, and the stroke in
// flight at "done" was never sent although the core accepts late shapes (D-167).
test("a stroke cut off when drawing ends is sent, and does not leak into the next drawing", () => {
  const { draw, input, strokes, unwatch } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  input.down(at(10, 10), ORIGIN);
  input.move(at(60, 60));
  // "…the cache, done" is heard while the pen is down.
  draw.apply({ type: "DrawMode", drawing: false }, 1);
  assert.equal(input.active, false, "the gesture ended with drawing");
  assert.deepEqual(strokes(), [
    [
      { x: 10, y: 10 },
      { x: 60, y: 60 },
    ],
  ]);
  // Later the user draws again: a fresh stroke from (300, 300).
  draw.apply({ type: "DrawMode", drawing: true }, 2);
  assert.equal(input.down(at(300, 300), ORIGIN), true, "the next pointerdown starts a stroke");
  input.move(at(350, 340));
  input.up(at(400, 380));
  assert.deepEqual(strokes()[1], [
    { x: 300, y: 300 },
    { x: 350, y: 340 },
    { x: 400, y: 380 },
  ]);
  unwatch();
});

test("a hover without the button down neither starts nor extends a stroke", () => {
  const { draw, input, strokes } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  assert.equal(input.move(at(5, 5, { buttons: 0 })), false);
  input.down(at(10, 10), ORIGIN);
  input.move(at(40, 40));
  // The button came up outside the window and its pointerup never arrived.
  assert.equal(input.move(at(200, 200, { buttons: 0 })), false, "a hover is ignored");
  assert.equal(input.preview(), "M10 10L40 40");
  input.up(at(40, 40, { buttons: 0 }));
  assert.deepEqual(strokes(), [
    [
      { x: 10, y: 10 },
      { x: 40, y: 40 },
    ],
  ]);
});

test("a lost capture sends what was drawn; after a pointerup it does nothing", () => {
  const { draw, input, sent, strokes } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  input.down(at(10, 10), ORIGIN);
  input.move(at(80, 20));
  assert.ok(input.lost(at(80, 20)) !== null, "the stroke is sent");
  assert.equal(input.active, false);
  assert.equal(input.up(at(90, 20)), null, "nothing more on the late pointerup");
  assert.equal(strokes().length, 1);
  input.down(at(10, 10), ORIGIN);
  input.up(at(70, 70));
  assert.equal(input.lost(at(70, 70)), null, "capture is lost after every pointerup");
  assert.equal(sent.length, 2);
});

test("the surface is measured once per gesture, at pointerdown", () => {
  const { draw, input, strokes } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  input.down(at(110, 60), { left: 100, top: 50 });
  input.move(at(160, 110));
  input.up(at(200, 150));
  assert.deepEqual(strokes()[0], [
    { x: 10, y: 10 },
    { x: 60, y: 60 },
    { x: 100, y: 100 },
  ]);
});

test("a cancelled gesture sends nothing, and ending drawing with none in flight sends nothing", () => {
  const { draw, input, sent, unwatch } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  input.down(at(10, 10), ORIGIN);
  input.move(at(80, 80));
  assert.equal(input.cancel({ pointerId: 1 }), true);
  draw.apply({ type: "DrawMode", drawing: false }, 1);
  assert.deepEqual(sent, []);
  unwatch();
  // Unwatched (the surface went away), drawing changes reach it no more.
  draw.apply({ type: "DrawMode", drawing: true }, 2);
  input.down(at(10, 10), ORIGIN);
  input.move(at(80, 80));
  draw.apply({ type: "DrawMode", drawing: false }, 3);
  assert.equal(input.active, true);
});

// Review 2 (CLAUDE.md, per-move work): the live pen preview rebuilt its whole path on every
// sample, so a long stroke formatted points quadratically. Counted through the sampler's reads.
test("a long pen stroke's live preview formats each point about once", () => {
  const { draw, input } = surface();
  draw.apply({ type: "DrawMode", drawing: true }, 0);
  const read = StrokeSampler.prototype.x;
  let reads = 0;
  StrokeSampler.prototype.x = function (this: StrokeSampler, index: number): number {
    reads += 1;
    return read.call(this, index);
  };
  const moves = 3_000;
  try {
    input.down(at(0, 0), ORIGIN);
    input.preview();
    for (let step = 1; step <= moves; step += 1) {
      if (input.move(at(step * 4, Math.round(Math.sin(step / 40) * 80)))) {
        input.preview();
      }
    }
  } finally {
    StrokeSampler.prototype.x = read;
  }
  assert.ok(reads <= 3 * moves, `${reads} point reads for ${moves} moves`);
  assert.match(input.preview(), /^M0 0L/);
});
