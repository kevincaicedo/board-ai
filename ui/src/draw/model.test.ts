import assert from "node:assert/strict";
import { test } from "node:test";
import { LEAVING_PAGE } from "../canvas/board.ts";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import { USER_SHAPES_PER_LAYER_MAX } from "./limits.ts";
import { DrawModel } from "./model.ts";

function boxed(id: string, label: string | null = null): UserShape {
  return { id, kind: { kind: "Box", rect: { x: 0, y: 0, w: 100, h: 60 } }, label, note: null };
}

test("drawing mode, the hint and your shapes come from the core's events", () => {
  const model = new DrawModel();
  assert.equal(model.tool.value, "pen", "the pen is the first tool");
  model.apply({ type: "DrawMode", drawing: true }, 100);
  assert.equal(model.drawing.value, true);
  model.apply({ type: "UserShape", layer: 0, shape: boxed("u1") }, 200);
  const version = model.version;
  model.apply({ type: "LabelHint", hint: { target: "u1", words: "this is the" } }, 250);
  assert.deepEqual(model.hint.value, { target: "u1", words: "this is the" });
  assert.equal(model.version, version, "a hint isn't a change to the shapes");
  // A re-sent shape replaces the old one whole.
  model.apply({ type: "UserShape", layer: 0, shape: boxed("u1", "cache") }, 300);
  assert.equal(model.shape(0, "u1")?.shape.label, "cache");
  assert.equal(model.shape(0, "u1")?.at, 300);
  assert.equal(model.layers.get(0)?.size, 1);
  assert.ok(model.version > version);
  model.apply({ type: "UserShapeRemove", layer: 0, id: "u1" }, 400);
  assert.equal(model.shape(0, "u1"), null);
  assert.equal(model.echoes.value, 3, "every echo moves the preview on");
  model.apply({ type: "LabelHint", hint: { target: "u1", words: "x" } }, 450);
  model.apply({ type: "DrawMode", drawing: false }, 500);
  assert.equal(model.drawing.value, false);
  assert.equal(model.hint.value, null, "leaving drawing clears the hint");
});

test("a layer keeps the core's number of your shapes, and reset clears everything", () => {
  const model = new DrawModel();
  for (let index = 1; index <= USER_SHAPES_PER_LAYER_MAX + 3; index += 1) {
    model.apply({ type: "UserShape", layer: 1, shape: boxed(`u${index}`) }, index);
  }
  assert.equal(model.layers.get(1)?.size, USER_SHAPES_PER_LAYER_MAX);
  assert.equal(model.counters.dropped, 3);
  // Changing one already there still works at the bound.
  model.apply({ type: "UserShape", layer: 1, shape: boxed("u1", "kept") }, 1_000);
  assert.equal(model.shape(1, "u1")?.shape.label, "kept");
  model.apply({ type: "DrawMode", drawing: true }, 1_001);
  model.reset();
  assert.equal(model.layers.size, 0);
  assert.equal(model.drawing.value, false);
  assert.equal(model.counters.dropped, 0);
});

test("a session's end ends drawing and keeps the shapes for the board's fade", () => {
  const model = new DrawModel();
  model.apply({ type: "DrawMode", drawing: true }, 100);
  model.apply({ type: "UserShape", layer: 0, shape: boxed("u1") }, 200);
  model.apply({ type: "LabelHint", hint: { target: "u1", words: "the cache" } }, 250);
  model.end();
  assert.equal(model.drawing.value, false);
  assert.equal(model.hint.value, null);
  assert.notEqual(model.shape(0, "u1"), null);
});

// D-177: your shapes on the main board leave with the page, never silently lost.
test("a page turn sends your main-board shapes away with the page", () => {
  const model = new DrawModel();
  model.apply({ type: "UserShape", layer: 0, shape: boxed("u1") }, 100);
  model.apply({ type: "UserShape", layer: 1, shape: boxed("u2") }, 110);
  const version = model.version;
  model.turnPage();
  assert.equal(model.shape(0, "u1"), null, "the fresh page has none of them");
  assert.equal(model.shape(LEAVING_PAGE, "u1")?.shape.id, "u1", "they leave with the page");
  assert.equal(model.shape(1, "u2")?.shape.id, "u2", "other layers are untouched");
  assert.ok(model.version > version);
  model.apply({ type: "UserShape", layer: 0, shape: boxed("u3") }, 200);
  model.turnPage();
  assert.equal(model.shape(LEAVING_PAGE, "u1"), null, "an older leaving page is let go");
  assert.equal(model.shape(LEAVING_PAGE, "u3")?.shape.id, "u3");
});
