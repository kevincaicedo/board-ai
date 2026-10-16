// Relabels (review finding 19). The core strikes an element with its box as the rect
// (core board/place.rs `strike`), whatever the shape; the old label sits inside the box for a
// service, above it for a group or a queue, and below it for an actor. The strike has to cross
// the label it replaces and write the new one above that label.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { NodeShape } from "../protocol/generated/NodeShape.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { BoardModel, type LeadDecision, type Run } from "./board.ts";
import type { TextMark } from "./stroke.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const lead: LeadDecision = {
  nibAtRest: () => "out",
  flightLead: () => ({ waitMs: 0, flightMs: 0 }),
};

const node = (shape: NodeShape, label: string, h: number): Extract<DrawOp, { kind: "Node" }> => ({
  kind: "Node",
  id: "g",
  op: 1,
  shape,
  label,
  rect: { x: 0, y: 0, w: 300, h },
  stack: 1,
  fill: null,
});

function draw(board: BoardModel, op: DrawOp, at: number): void {
  board.apply({ type: "OpStart", layer: 0, op, budget_ms: 1000 }, at, lead);
}

const live = (board: BoardModel): Run[] => board.main.runs.filter((run) => run.replacedAt === null);

function textOf(run: Run | undefined): TextMark {
  const text = run?.plan.items.find((item) => item.type === "text");
  assert.ok(text !== undefined && text.type === "text");
  return text;
}

for (const [shape, h] of [
  ["Service", 86],
  ["Group", 220],
  ["Queue", 86],
  ["Actor", 86],
] as const) {
  test(`a strike on a ${shape} crosses the label it replaces and writes above it`, () => {
    const board = new BoardModel(tokens);
    const old = node(shape, "Old", h);
    draw(board, old, 0);
    draw(board, { kind: "Strike", op: 2, target: "g", label: "New", rect: old.rect }, 2000);
    const [element, strike] = live(board);
    const label = textOf(element);
    const line = strike?.plan.items.find((item) => item.type === "stroke");
    assert.ok(line !== undefined && line.type === "stroke");
    const lineY = line.points[1] ?? Number.NaN;
    assert.ok(Math.abs(lineY - label.y) < 6, `strike at y=${lineY}, old label at y=${label.y}`);
    const lineLeft = line.points[0] ?? Number.NaN;
    assert.ok(lineLeft < label.start[0] && lineLeft > label.start[0] - 20, "it spans the label");
    const rewrite = textOf(strike);
    assert.equal(rewrite.text, "New");
    assert.ok(rewrite.y < label.y - label.size, "the new label sits above the old one");
    assert.ok(rewrite.y > -tokens.handGroup * 4, "and near it, inside the camera's margin");
    if (shape === "Service" || shape === "Actor") {
      assert.ok(rewrite.y < 0, "a label inside or under the drawing is rewritten above it");
    }
  });
}

test("a group re-sent with its new label drops the strike, so the label shows once", () => {
  const board = new BoardModel(tokens);
  const old = node("Group", "Old", 220);
  draw(board, old, 0);
  draw(board, { kind: "Strike", op: 2, target: "g", label: "New", rect: old.rect }, 2000);
  // A child joined: the core re-sends the group, grown, with its current label.
  draw(board, { ...node("Group", "New", 300), op: 3 }, 4000);
  const labels = live(board).flatMap((run) =>
    run.plan.items.filter((item) => item.type === "text").map((item) => item.text),
  );
  assert.deepEqual(labels, ["New"]);
});

test("a strike whose target isn't drawn falls back to the middle of its rect", () => {
  const board = new BoardModel(tokens);
  draw(
    board,
    { kind: "Strike", op: 2, target: "x", label: "New", rect: { x: 0, y: 0, w: 80, h: 24 } },
    0,
  );
  const line = live(board)[0]?.plan.items.find((item) => item.type === "stroke");
  assert.ok(line !== undefined && line.type === "stroke");
  assert.ok(Math.abs((line.points[1] ?? Number.NaN) - 12) < 2);
});
