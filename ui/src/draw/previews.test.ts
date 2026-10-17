import assert from "node:assert/strict";
import { test } from "node:test";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import { DrawModel } from "./model.ts";
import {
  echoed,
  expired,
  nextExpiry,
  PENDING_PREVIEWS_MAX,
  type Preview,
  sent,
} from "./previews.ts";

function box(id: string, label: string | null = null): UserShape {
  return { id, kind: { kind: "Box", rect: { x: 0, y: 0, w: 40, h: 40 } }, label, note: null };
}

function ink(id: string, strokes: number): UserShape {
  const stroke = [
    { x: 0, y: 0 },
    { x: 30, y: 30 },
  ];
  return {
    id,
    kind: {
      kind: "Ink",
      strokes: Array.from({ length: strokes }, () => stroke),
      bounds: { x: 0, y: 0, w: 30, h: 30 },
    },
    label: null,
    note: null,
  };
}

/** Sends one shape's preview against the model's count now, as the surface does. */
function send(previews: readonly Preview[], model: DrawModel, serial: number): readonly Preview[] {
  return sent(previews, serial, `M${serial} 0`, model.accepted.peek(), 0);
}

// Review 2: any shape's echo took the newest preview away early. The label landing on u1 re-sends
// u1 whole while the new box's own echo is still on its way.
test("a preview stays until its own shape is on the board, not any echo", () => {
  const model = new DrawModel();
  model.apply({ type: "UserShape", layer: 0, shape: box("u1") }, 1);
  let previews = send([], model, 1);
  model.apply({ type: "UserShape", layer: 0, shape: box("u1", "cache") }, 2);
  previews = echoed(previews, model.accepted.peek());
  assert.equal(previews.length, 1, "a relabelled u1 is not the new box");
  model.apply({ type: "UserShape", layer: 0, shape: box("u2") }, 3);
  previews = echoed(previews, model.accepted.peek());
  assert.equal(previews.length, 0, "u2 is the new box");
});

test("a pen stroke that joins the newest pen shape is its echo; an undo is no one's", () => {
  const model = new DrawModel();
  model.apply({ type: "UserShape", layer: 0, shape: ink("u1", 1) }, 1);
  let previews = send([], model, 1);
  model.apply({ type: "UserShapeRemove", layer: 0, id: "u9" }, 2);
  previews = echoed(previews, model.accepted.peek());
  assert.equal(previews.length, 1, "an undo elsewhere leaves the preview");
  // The core joined the stroke to u1 (D-164): u1 comes back with two strokes.
  model.apply({ type: "UserShape", layer: 0, shape: ink("u1", 2) }, 3);
  previews = echoed(previews, model.accepted.peek());
  assert.equal(previews.length, 0);
  // Re-sent with the same strokes (a label): no acceptance.
  const before = model.accepted.peek();
  model.apply({ type: "UserShape", layer: 0, shape: { ...ink("u1", 2), label: "path" } }, 4);
  assert.equal(model.accepted.peek(), before);
});

test("previews are bounded, and a refused shape's preview goes when its hold runs out", () => {
  let previews: readonly Preview[] = [];
  for (let serial = 1; serial <= PENDING_PREVIEWS_MAX + 2; serial += 1) {
    previews = sent(previews, serial, "M0 0", 0, serial * 100);
  }
  assert.equal(previews.length, PENDING_PREVIEWS_MAX);
  assert.equal(previews[0]?.serial, 3, "the oldest went first");
  assert.equal(nextExpiry(previews, 400, 600), 500);
  assert.equal(expired(previews, 1000, 600).length, PENDING_PREVIEWS_MAX - 2);
  assert.equal(nextExpiry([], 0, 600), null);
  assert.equal(echoed(previews, 0), previews, "nothing covered: the same list");
});
