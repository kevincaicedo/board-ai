// The diagram types' strokes (D-120): each draws in a hand's order, keeps to the core's geometry,
// and a row of cells names each cell for pointing and strikes.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { BoardModel, type LeadDecision, type Run } from "./board.ts";
import { planOp } from "./plan.ts";
import type { Item, Stroke, TextMark } from "./stroke.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const lead: LeadDecision = {
  nibAtRest: () => "out",
  flightLead: () => ({ waitMs: 0, flightMs: 0 }),
};

const strokes = (items: readonly Item[]): Stroke[] =>
  items.filter((item): item is Stroke => item.type === "stroke");
const texts = (items: readonly Item[]): TextMark[] =>
  items.filter((item): item is TextMark => item.type === "text");
const firstPoint = (stroke: Stroke): [number, number] => [
  stroke.points[0] ?? Number.NaN,
  stroke.points[1] ?? Number.NaN,
];

const participant: Extract<DrawOp, { kind: "Participant" }> = {
  kind: "Participant",
  id: "c",
  op: 1,
  label: "Client",
  rect: { x: 47, y: 46, w: 105, h: 48 },
  lifeline_bottom: 242,
};

test("a participant draws its head, then its label, then its dashed lifeline down", () => {
  const plan = planOp(participant, tokens);
  const kinds = plan.items.map((item) => item.type);
  assert.deepEqual(kinds, ["stroke", "stroke", "stroke", "stroke", "text", "stroke"]);
  const lifeline = plan.items.at(-1) as Stroke;
  assert.equal(lifeline.dashed, true);
  assert.equal(lifeline.tone, "grey");
  const [x, top] = firstPoint(lifeline);
  assert.ok(Math.abs(x - 99.5) < 2 && Math.abs(top - 94) < 2, `${x}, ${top}`);
  assert.ok(plan.bounds.y + plan.bounds.h >= 240, "the lifeline is part of what it covers");
});

test("a message stretches the lifelines first, then the arrow, its head and its label", () => {
  const message: Extract<DrawOp, { kind: "Message" }> = {
    kind: "Message",
    id: "m:4",
    op: 4,
    from: "c",
    to: "s",
    points: [
      { x: 99, y: 242 },
      { x: 300, y: 242 },
    ],
    label: "ACK",
    style: "Solid",
    extend: [
      { participant: "c", x: 99, top: 242, bottom: 346 },
      { participant: "s", x: 300, top: 242, bottom: 346 },
    ],
  };
  const items = planOp(message, tokens).items;
  assert.deepEqual(
    items.map((item) =>
      item.type === "stroke" ? `${item.kind}${item.dashed ? "-" : ""}` : "text",
    ),
    ["line-", "line-", "line", "head", "text"],
  );
  assert.equal(texts(items)[0]?.y, 242 - 14, "the label sits above the arrow");
  const reply = planOp({ ...message, style: "Dashed", extend: [] }, tokens).items;
  assert.equal(strokes(reply)[0]?.dashed, true, "a reply is dashed");
  assert.equal(strokes(reply)[0]?.tone, "ink2");
});

test("a message to itself loops right and writes its label beside the loop", () => {
  const loop: Extract<DrawOp, { kind: "Message" }> = {
    kind: "Message",
    id: "m:6",
    op: 6,
    from: "s",
    to: "s",
    points: [
      { x: 300, y: 346 },
      { x: 344, y: 346 },
      { x: 344, y: 372 },
      { x: 300, y: 372 },
    ],
    label: "parse",
    style: "Solid",
    extend: [],
  };
  const items = planOp(loop, tokens).items;
  const [arrow] = strokes(items);
  assert.equal(arrow?.legs.length, 4, "three legs, one per side of the loop");
  const label = texts(items)[0];
  assert.equal(label?.align, "left");
  assert.equal(label?.x, 344 + 10);
});

const cells = (vertical: boolean): Extract<DrawOp, { kind: "Cells" }> => ({
  kind: "Cells",
  id: "arr",
  op: 1,
  label: "nums",
  cells: [0, 1, 2].map((index) => ({
    id: `arr.${index}`,
    rect: vertical
      ? { x: 80, y: 60 + index * 48, w: 48, h: 48 }
      : { x: 86 + index * 48, y: 31, w: 48, h: 48 },
    value: index === 1 ? "" : String(index + 3),
    index: String(index),
  })),
  vertical,
});

test("a row of cells: its name, the outline, the dividers, the values, then the indexes", () => {
  const plan = planOp(cells(false), tokens);
  const order = plan.items.map((item) => (item.type === "text" ? `t:${item.text}` : "s"));
  assert.deepEqual(order, [
    "t:nums",
    "s",
    "s",
    "s",
    "s",
    "s",
    "s",
    "t:3",
    "t:5",
    "t:0",
    "t:1",
    "t:2",
  ]);
  const name = texts(plan.items)[0];
  assert.equal(name?.align, "right", "a row's name ends left of it");
  assert.equal(name?.x, 86 - 14);
  const index = texts(plan.items).find((text) => text.text === "1");
  assert.equal(index?.y, 31 + 48 + 16, "an index under its cell");
  assert.deepEqual(
    plan.targets.map((target) => [target.id, target.label?.text ?? null]),
    [
      ["arr.0", "3"],
      ["arr.1", null],
      ["arr.2", "5"],
    ],
    "each cell is a target, with the value a strike would cross",
  );
});

test("a column of cells writes its name above and its indexes on the left", () => {
  const plan = planOp(cells(true), tokens);
  const name = texts(plan.items)[0];
  assert.equal(name?.align, "center");
  assert.equal(name?.y, 60 - 21);
  const index = texts(plan.items).find((text) => text.text === "2");
  assert.equal(index?.align, "right");
  assert.equal(index?.x, 80 - 10);
});

test("a tree node draws the line from its parent first, then its circle and label", () => {
  const plan = planOp(
    {
      kind: "Node",
      id: "n3",
      op: 2,
      shape: "Circle",
      label: "3",
      rect: { x: -88, y: 154, w: 56, h: 56 },
      stack: 1,
      fill: "Mint",
      link: { start: { x: 100, y: 98 }, control: null, end: { x: -60, y: 154 } },
    },
    tokens,
  );
  const [link, circle] = strokes(plan.items);
  const [x, y] = firstPoint(link as Stroke);
  assert.ok(Math.hypot(x - 100, y - 98) < 3, `the pen starts at the parent: ${x}, ${y}`);
  assert.equal(circle?.kind, "ellipse");
  assert.equal(plan.items.at(-1)?.type, "stroke", "the fill comes last");
  assert.equal((plan.items.at(-1) as Stroke).kind, "hatch");
});

test("a highlight shades its target in the core's tone, round on a circle", () => {
  const highlight: Extract<DrawOp, { kind: "Annotate" }> = {
    kind: "Annotate",
    id: "a:5",
    op: 5,
    target: "n4",
    mark: "Highlight",
    rect: { x: 0, y: 0, w: 56, h: 56 },
    text: "i",
    fill: "Mint",
  };
  const square = planOp(highlight, tokens).items;
  const [shade] = strokes(square);
  assert.equal(shade?.kind, "hatch");
  assert.equal(shade?.tone, "mint");
  assert.equal(texts(square)[0]?.y, -21, "its note sits above it");
  const round = strokes(planOp(highlight, tokens, null, true).items)[0] as Stroke;
  for (let index = 0; index < round.points.length; index += 2) {
    const dx = (round.points[index] ?? 0) - 28;
    const dy = (round.points[index + 1] ?? 0) - 28;
    assert.ok(Math.hypot(dx, dy) <= 28, "inside the circle");
  }
});

function draw(board: BoardModel, op: DrawOp, at: number): void {
  board.apply({ type: "OpStart", layer: 0, op, budget_ms: 1000 }, at, lead);
}

const live = (board: BoardModel): Run[] => board.main.runs.filter((run) => run.replacedAt === null);

test("a value written into a cell, then struck: the strike crosses the written value", () => {
  const board = new BoardModel(tokens);
  draw(board, cells(false), 0);
  draw(
    board,
    { kind: "Write", op: 2, target: "arr.1", label: "8", rect: { x: 134, y: 31, w: 48, h: 48 } },
    1000,
  );
  draw(
    board,
    { kind: "Strike", op: 3, target: "arr.1", label: "9", rect: { x: 134, y: 31, w: 48, h: 48 } },
    2000,
  );
  draw(
    board,
    { kind: "Strike", op: 4, target: "arr.0", label: "7", rect: { x: 86, y: 31, w: 48, h: 48 } },
    3000,
  );
  const runs = live(board);
  assert.deepEqual(
    runs.map((run) => run.key),
    ["arr", "arr.1#write", "arr.1#strike", "arr.0#strike"],
    "nothing replaced the row",
  );
  const crossed = (run: Run | undefined): number => {
    const line = run?.plan.items[0];
    return line?.type === "stroke" ? (line.points[1] ?? Number.NaN) : Number.NaN;
  };
  const written = texts(runs[1]?.plan.items ?? [])[0];
  assert.ok(Math.abs(crossed(runs[2]) - (written?.y ?? 0)) < 4, "through the written 8");
  const strikeStart = runs[2]?.plan.items[0];
  const startX =
    strikeStart?.type === "stroke" ? (strikeStart.points[0] ?? Number.NaN) : Number.NaN;
  assert.ok(
    Math.abs(startX - ((written?.start[0] ?? 0) - 6)) < 4,
    `the line spans the written value, not the whole cell: ${startX}`,
  );
  assert.equal(texts(runs[2]?.plan.items ?? [])[0]?.size, written?.size, "rewritten at its size");
  const value = runs[0]?.plan.targets[0]?.label;
  assert.ok(Math.abs(crossed(runs[3]) - (value?.y ?? 0)) < 4, "through the cell's own 3");
});
