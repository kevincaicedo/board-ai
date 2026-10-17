import assert from "node:assert/strict";
import { test } from "node:test";
import { parseDrawOp, parseUiEvent } from "./parse.ts";
import { REVEAL_PARTS_MAX } from "./staged.ts";

const parts = [
  {
    part: "Frame",
    id: "arena",
    rect: { x: 22, y: 14, w: 336, h: 640 },
    label: { text: "Arena", at: { x: 40, y: 38 } },
    note: null,
    category: "Slate",
    dashed: true,
  },
  {
    part: "Node",
    id: "free",
    shape: "Pill",
    rect: { x: 40, y: 96, w: 62, h: 30 },
    label: "Free",
    note: "a slot",
    step: 2,
    category: null,
    dashed: false,
  },
  { part: "Note", id: "n", rect: { x: 0, y: 0, w: 10, h: 18 }, lines: ["one", "two"] },
  {
    part: "Record",
    id: "rec",
    label: "struct Arena",
    fields: [{ id: "rec.0", rect: { x: 0, y: 0, w: 40, h: 30 }, value: "cfg", index: null }],
  },
  {
    part: "Cells",
    id: "c",
    label: "",
    cells: [{ id: "c.0", rect: { x: 0, y: 0, w: 40, h: 28 }, value: "", index: "0" }],
    vertical: false,
  },
  {
    part: "Edge",
    id: "free>rec",
    from: "free",
    to: "rec",
    path: { start: { x: 0, y: 0 }, control: { x: 5, y: -5 }, end: { x: 10, y: 0 } },
    label: { text: "try", at: { x: 5, y: -9 } },
    style: "Dashed",
  },
  {
    part: "Legend",
    id: "inf:legend0",
    rect: { x: 0, y: 0, w: 120, h: 22 },
    category: "Green",
    label: "tiered RAM",
  },
];

test("a reveal parses with every kind of part, as sent", () => {
  const op = { kind: "Reveal", id: "r:7", op: 7, parts };
  assert.deepEqual(parseDrawOp(op), op);
});

test("one malformed part drops the whole reveal", () => {
  for (const bad of [
    { ...parts[0], category: "Plaid" },
    { ...parts[0], label: "Arena" },
    { ...parts[1], step: 300 },
    { ...parts[2], lines: "one" },
    { ...parts[3], fields: [] },
    { ...parts[5], label: { text: "x" } },
    { ...parts[6], category: null },
    { part: "Spiral", id: "s" },
  ]) {
    assert.equal(parseDrawOp({ kind: "Reveal", id: "r:7", op: 7, parts: [parts[0], bad] }), null);
  }
  assert.equal(parseDrawOp({ kind: "Reveal", id: "r:7", op: 7, parts: [] }), null);
  const tooMany = Array.from({ length: REVEAL_PARTS_MAX + 1 }, () => parts[2]);
  assert.equal(parseDrawOp({ kind: "Reveal", id: "r:7", op: 7, parts: tooMany }), null);
});

test("a pill parses as a sketch's node too", () => {
  const node = {
    kind: "Node",
    id: "s",
    op: 1,
    shape: "Pill",
    label: "Free",
    rect: { x: 0, y: 0, w: 1, h: 1 },
    stack: 1,
    fill: null,
    link: null,
  };
  assert.deepEqual(parseDrawOp(node), node);
});

test("a detail layer's anchor, a close-up and a diagram header parse; bad ones don't", () => {
  const push = {
    type: "LayerPush",
    layer: 2,
    title: "Arena · inside",
    question: null,
    anchor: {
      layer: 0,
      element: "arena",
      rect: { x: 22, y: 14, w: 336, h: 640 },
      category: "Slate",
    },
  };
  assert.deepEqual(parseUiEvent(push), push);
  assert.equal(parseUiEvent({ ...push, anchor: { layer: 0, element: "a b" } }), null);
  const camera = {
    type: "Camera",
    layer: 0,
    frame: { x: 0, y: 0, w: 10, h: 10 },
    focus: null,
    enlarge: true,
  };
  assert.deepEqual(parseUiEvent(camera), camera);
  assert.equal(parseUiEvent({ ...camera, enlarge: "yes" }), null);
  const header = { type: "Diagram", layer: 0, title: "inf-alloc", part: 3, parts: 4 };
  assert.deepEqual(parseUiEvent(header), header);
  assert.equal(parseUiEvent({ ...header, part: 5 }), null);
  assert.equal(parseUiEvent({ ...header, title: 7 }), null);
});
