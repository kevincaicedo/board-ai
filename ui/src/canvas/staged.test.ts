import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { StagedPart } from "../protocol/generated/StagedPart.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { type BoardEvent, BoardModel } from "./board.ts";
import { cameraAt, emptyArea, stageArea } from "./camera.ts";
import { labelOf, planOp } from "./plan.ts";
import { leadDecision, Presence } from "./presence.ts";
import { partRect, revealItems } from "./staged.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const frame: StagedPart = {
  part: "Frame",
  id: "arena",
  rect: { x: 22, y: 14, w: 336, h: 640 },
  label: { text: "Arena", at: { x: 40, y: 38 } },
  note: { text: "slab allocation · RAM records", at: { x: 110, y: 38 } },
  category: "Slate",
  dashed: true,
};
const slot: StagedPart = {
  part: "Node",
  id: "slot",
  shape: "Pill",
  rect: { x: 40, y: 96, w: 62, h: 30 },
  label: "Free",
  note: null,
  step: null,
  category: "Blue",
  dashed: false,
};
const step: StagedPart = {
  part: "Node",
  id: "round",
  shape: "Service",
  rect: { x: 140, y: 96, w: 180, h: 52 },
  label: "round up to a class",
  note: "40 KiB → the 64 KiB class",
  step: 1,
  category: null,
  dashed: false,
};
const note: StagedPart = {
  part: "Note",
  id: "arena:note1",
  rect: { x: 40, y: 380, w: 260, h: 36 },
  lines: ["over threshold → its own region", "res_bytes reserved, not committed"],
};
const record: StagedPart = {
  part: "Record",
  id: "rec",
  label: "struct Arena",
  fields: [
    { id: "rec.0", rect: { x: 34, y: 590, w: 44, h: 30 }, value: "cfg", index: null },
    { id: "rec.1", rect: { x: 78, y: 590, w: 64, h: 30 }, value: "threshold", index: null },
  ],
};
const cells: StagedPart = {
  part: "Cells",
  id: "classes",
  label: "size classes",
  cells: [
    { id: "classes.0", rect: { x: 40, y: 160, w: 46, h: 28 }, value: "64K", index: "0" },
    { id: "classes.1", rect: { x: 86, y: 160, w: 46, h: 28 }, value: "128K", index: "1" },
  ],
  vertical: false,
};
const edge: StagedPart = {
  part: "Edge",
  id: "slot>round",
  from: "slot",
  to: "round",
  path: { start: { x: 102, y: 111 }, control: null, end: { x: 140, y: 111 } },
  label: { text: "try lease", at: { x: 121, y: 102 } },
  style: "Solid",
};
const legend: StagedPart = {
  part: "Legend",
  id: "inf:legend0",
  rect: { x: 1100, y: 90, w: 140, h: 22 },
  category: "Slate",
  label: "RAM records",
};

const reveal = (parts: StagedPart[], op = 7): Extract<DrawOp, { kind: "Reveal" }> => ({
  kind: "Reveal",
  id: `r:${op}`,
  op,
  parts,
});

test("a reveal plans every part in the core's order, each a target with its own seed", () => {
  const op = reveal([frame, slot, step, edge, note, record, cells, legend]);
  const plan = revealItems(op, tokens);
  const ids = plan.targets.map((target) => target.id);
  assert.deepEqual(ids, [
    "arena",
    "slot",
    "round",
    "slot>round",
    "arena:note1",
    "rec",
    "rec.0",
    "rec.1",
    "classes",
    "classes.0",
    "classes.1",
    "inf:legend0",
  ]);
  // The frame's outline and words come first, the legend's last.
  const texts = plan.items.filter((item) => item.type === "text").map((item) => item.text);
  assert.deepEqual(texts.slice(0, 2), ["Arena", "slab allocation · RAM records"]);
  assert.equal(texts[texts.length - 1], "RAM records");
  assert.ok(texts.includes("1. round up to a class"), "a step is numbered");
  // A part planned in another reveal draws the same strokes: it is seeded by its id.
  const alone = revealItems(reveal([slot], 9), tokens);
  const first = plan.items.find((item) => item.type === "stroke");
  const again = alone.items.find((item) => item.type === "stroke");
  assert.ok(first !== undefined && again !== undefined);
  const slotStroke = plan.items.filter((item) => item.type === "stroke")[1];
  assert.ok(slotStroke?.type === "stroke" && again.type === "stroke");
  assert.equal(slotStroke.path, again.path);
});

test("a categorised box is filled under its outline, a frame takes its category's stroke", () => {
  const plan = revealItems(reveal([frame, slot]), tokens);
  const strokes = plan.items.filter((item) => item.type === "stroke");
  assert.equal(strokes[0]?.type === "stroke" && strokes[0].tone, "slate");
  assert.ok(strokes[0]?.type === "stroke" && strokes[0].dashed, "a dashed container");
  const fill = plan.items.findIndex((item) => item.type === "stroke" && item.kind === "fill");
  const outline = plan.items.findIndex(
    (item) => item.type === "stroke" && item.tone === "ink" && item.kind === "line",
  );
  assert.ok(fill > outline && outline >= 0, "the fill comes after the outline");
  const filled = plan.items[fill];
  assert.ok(filled?.type === "stroke" && filled.tone === "blue-soft");
  // An uncategorised box is not filled.
  const plain = revealItems(reveal([step]), tokens);
  assert.ok(!plain.items.some((item) => item.type === "stroke" && item.kind === "fill"));
});

test("fields and cells are written in the code font, their names and notes in the hand", () => {
  const plan = revealItems(reveal([record, cells]), tokens);
  const font = (words: string) =>
    plan.items.find((item) => item.type === "text" && item.text === words);
  const field = font("threshold");
  assert.equal(field?.type === "text" && field.font, "code");
  const name = font("struct Arena");
  assert.ok(name?.type === "text" && name.font === "hand" && name.align === "left");
  assert.equal(name.x, 34, "left-aligned at the first field");
  assert.equal(name.y, 620 + tokens.diagramRecordLabelCenter);
  const index = font("1");
  assert.ok(index?.type === "text" && index.font === "code");
  assert.equal(index.y, 188 + tokens.diagramCellsIndexBelow);
});

test("the plan bounds every part, and parts' rects cover their drawings", () => {
  const plan = planOp(reveal([frame, slot, edge]), tokens);
  assert.ok(plan.bounds.w >= frame.rect.w && plan.bounds.h >= frame.rect.h);
  assert.deepEqual(partRect(edge), { x: 102, y: 111, w: 38, h: 0 });
  assert.deepEqual(partRect(record), { x: 34, y: 590, w: 108, h: 30 });
});

function board() {
  const model = new BoardModel(tokens);
  const lead = leadDecision(model, new Presence(tokens));
  return { model, apply: (event: BoardEvent, at: number) => model.apply(event, at, lead) };
}

test("a strike on a staged part crosses the label its reveal wrote", () => {
  const { model, apply } = board();
  apply({ type: "OpStart", layer: 0, op: reveal([frame, slot]), budget_ms: 3000 }, 0);
  apply(
    {
      type: "OpStart",
      layer: 0,
      op: { kind: "Strike", op: 8, target: "slot", label: "Leased", rect: slot.rect },
      budget_ms: 600,
    },
    4000,
  );
  const runs = model.main.runs;
  assert.equal(runs.length, 2);
  assert.equal(runs[0]?.key, "r:7", "a reveal is keyed by its own id, not a part's");
  const written = runs[1]?.plan.items.filter((item) => item.type === "text") ?? [];
  // The rewrite is written at the struck label's size, the part's, not a sketch's: the reveal's
  // targets gave the strike its label.
  assert.equal(written[0]?.type === "text" && written[0].size, tokens.diagramLabel);
  assert.equal(labelOf(runs[1]?.plan ?? planOp(reveal([slot]), tokens))?.size, tokens.diagramLabel);
});

test("a staged circle takes a highlight inside its outline", () => {
  const shade = (shape: "Circle" | "Service"): number => {
    const { model, apply } = board();
    const part: StagedPart = { ...slot, id: "s1", shape, rect: { x: 0, y: 0, w: 60, h: 60 } };
    apply({ type: "OpStart", layer: 0, op: reveal([part]), budget_ms: 1000 }, 0);
    apply(
      {
        type: "OpStart",
        layer: 0,
        op: {
          kind: "Annotate",
          id: "a:9",
          op: 9,
          target: "s1",
          mark: "Highlight",
          rect: part.rect,
          text: null,
          fill: "Blush",
        },
        budget_ms: 500,
      },
      2000,
    );
    const hatch = model.main.runs[1]?.plan.items.find((item) => item.type === "stroke");
    assert.ok(hatch?.type === "stroke" && hatch.kind === "hatch");
    return hatch.length;
  };
  // Inside an ellipse the shade's lines are shorter than across the box's corners.
  assert.ok(shade("Circle") < shade("Service"));
});

test("a close-up may enlarge up to the diagram's cap; every other frame stays at 1", () => {
  const area = stageArea({ width: 1440, height: 900 }, tokens, emptyArea());
  const small = { x: 0, y: 0, w: 200, h: 100 };
  const out = { scale: 1, x: 0, y: 0 };
  const camera = {
    frame: small,
    previous: null,
    at: 0,
    focus: null,
    previousFocus: null,
    focusAt: 0,
  };
  cameraAt({ ...camera, enlarge: true }, 0, area, 900, out, tokens.diagramCameraScaleMax);
  assert.equal(out.scale, tokens.diagramCameraScaleMax);
  cameraAt({ ...camera, enlarge: false }, 0, area, 900, out, tokens.diagramCameraScaleMax);
  assert.equal(out.scale, tokens.cameraScaleMax);
  // Easing out of a close-up into an ordinary frame starts from the close-up's scale.
  cameraAt(
    { ...camera, frame: { x: 0, y: 0, w: 2000, h: 1000 }, previous: small, previousEnlarge: true },
    0,
    area,
    900,
    out,
    tokens.diagramCameraScaleMax,
  );
  assert.equal(out.scale, tokens.diagramCameraScaleMax);
});

test("a pill node in a sketch draws as one rounded outline, its label and its fill", () => {
  const plan = planOp(
    {
      kind: "Node",
      id: "free",
      op: 1,
      shape: "Pill",
      label: "Free",
      rect: { x: 0, y: 0, w: 120, h: 52 },
      stack: 1,
      fill: "Sky",
      link: null,
    },
    tokens,
  );
  const kinds = plan.items.map((item) => (item.type === "text" ? "text" : item.kind));
  assert.deepEqual(kinds, ["line", "text", "hatch"]);
});
