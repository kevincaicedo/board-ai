import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { ARROW_LABEL_ABOVE, NOTE_BELOW, planYours, shapeBounds, smoothPath } from "./ink.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

function texts(shape: UserShape) {
  return planYours(shape, tokens).pieces.filter((piece) => piece.type === "text");
}

function paths(shape: UserShape) {
  return planYours(shape, tokens).pieces.filter((piece) => piece.type === "path");
}

const client: UserShape = {
  id: "u1",
  kind: { kind: "Box", rect: { x: 262, y: 326, w: 120, h: 72 } },
  label: "Client",
  note: null,
};

test("a box is four rough sides in your ink with its label in the middle", () => {
  assert.equal(paths(client).length, 4);
  for (const piece of paths(client)) {
    assert.ok(piece.type === "path" && piece.width === tokens.inkYouWidth);
  }
  assert.deepEqual(
    texts(client).map((piece) => piece.type === "text" && [piece.x, piece.y, piece.text]),
    [[322, 362, "Client"]],
  );
  // Seeded by the id: the same shape draws the same strokes, a label landing changes no stroke.
  assert.deepEqual(paths(client), paths({ ...client, label: null }));
});

test("a pen shape keeps every stroke and writes its note under it, left-aligned", () => {
  const postgres: UserShape = {
    id: "u5",
    kind: {
      kind: "Ink",
      strokes: [
        [
          { x: 852, y: 326 },
          { x: 900, y: 316 },
          { x: 942, y: 326 },
        ],
        [
          { x: 852, y: 326 },
          { x: 942, y: 326 },
        ],
      ],
      bounds: { x: 852, y: 316, w: 91, h: 95 },
    },
    label: "Postgres",
    note: "source of truth",
  };
  assert.deepEqual(
    paths(postgres).map((piece) => piece.type === "path" && piece.d),
    ["M852 326Q900 316 921 321L942 326", "M852 326L942 326"],
  );
  const note = texts(postgres).find((piece) => piece.type === "text" && piece.note);
  assert.deepEqual(note, {
    type: "text",
    x: 852,
    y: 316 + 95 + NOTE_BELOW,
    text: "source of truth",
    size: tokens.handNote,
    align: "left",
    note: true,
  });
});

test("an arrow is a line and its head, its label above its middle", () => {
  const arrow: UserShape = {
    id: "u2",
    kind: {
      kind: "Arrow",
      start: { x: 382, y: 364 },
      end: { x: 522, y: 362 },
      from: "u1",
      to: null,
    },
    label: "calls",
    note: null,
  };
  assert.equal(paths(arrow).length, 2);
  const [label] = texts(arrow);
  assert.ok(label?.type === "text");
  assert.deepEqual([label.x, label.y, label.size], [452, 363 - ARROW_LABEL_ABOVE, tokens.handEdge]);
  assert.deepEqual(shapeBounds(arrow), { x: 382, y: 362, w: 140, h: 2 });
});

test("a freehand stroke passes through its points' midpoints", () => {
  assert.equal(smoothPath([]), "");
  assert.equal(smoothPath([{ x: 1, y: 2 }]), "M1 2L1 2");
  assert.equal(
    smoothPath([
      { x: 0, y: 0 },
      { x: 10, y: 0 },
      { x: 10, y: 10 },
      { x: 0, y: 10 },
    ]),
    "M0 0Q10 0 10 5Q10 10 5 10L0 10",
  );
});
