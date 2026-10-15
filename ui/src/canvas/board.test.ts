import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { type BoardEvent, BoardModel, LAYER_ELEMENTS_MAX, runActive } from "./board.ts";
import { cameraAt, depthOfField, fitFrame } from "./camera.ts";
import { planOp } from "./plan.ts";
import {
  dockSwell,
  homeStart,
  leadDecision,
  type NibFrame,
  nibAt,
  orbPlacement,
  Presence,
} from "./presence.ts";
import { naturalTime } from "./timeline.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const box = (id: string, label = id, x = 0): Extract<DrawOp, { kind: "Node" }> => ({
  kind: "Node",
  id,
  op: 1,
  shape: "Service",
  label,
  rect: { x, y: 0, w: 132, h: 76 },
  stack: 1,
});

function scene() {
  const board = new BoardModel(tokens);
  const presence = new Presence(tokens);
  const lead = leadDecision(board, presence);
  const apply = (event: BoardEvent, at: number): void => {
    if (event.type === "OpStart" && event.layer === 0) {
      presence.drawingStarted(at);
    }
    if (event.type === "Freeze") {
      presence.freeze(at);
    }
    board.apply(event, at, lead);
  };
  return { board, presence, apply };
}

const start = (op: DrawOp, budget = 2500, layer = 0): BoardEvent => ({
  type: "OpStart",
  layer,
  op,
  budget_ms: budget,
});

test("plans are deterministic per element id and follow a hand's order", () => {
  const first = planOp(box("cdn", "CDN"), "sky", tokens);
  const again = planOp(box("cdn", "CDN"), "sky", tokens);
  assert.deepEqual(
    first.items.map((item) => (item.type === "stroke" ? item.path : item.text)),
    again.items.map((item) => (item.type === "stroke" ? item.path : item.text)),
  );
  // Outline, then the label, then the hachure fill.
  assert.deepEqual(
    first.items.map((item) => (item.type === "stroke" ? item.kind : "text")),
    ["line", "line", "line", "line", "text", "hatch"],
  );
  assert.ok(first.naturalMs > 1000 && first.naturalMs < 2000);
  for (let index = 1; index < first.items.length; index += 1) {
    assert.ok(
      (first.starts[index] ?? 0) > (first.ends[index - 1] ?? 0),
      "the pen travels between items",
    );
  }
});

test("every op kind and shape plans without throwing", () => {
  const shapes = ["Service", "Db", "Queue", "Actor", "Group", "Note"] as const;
  for (const shape of shapes) {
    const plan = planOp({ ...box("n"), shape, stack: 3 }, "mint", tokens);
    assert.ok(plan.items.length > 0, shape);
  }
  for (const mark of ["Circle", "Underline", "Arrow", "Bracket"] as const) {
    const op: DrawOp = {
      kind: "Annotate",
      id: "a",
      op: 2,
      target: "n",
      mark,
      rect: { x: 0, y: 0, w: 60, h: 20 },
      text: "TTL",
    };
    assert.equal(planOp(op, null, tokens).items.at(-1)?.type, "text", mark);
  }
  const strike: DrawOp = {
    kind: "Strike",
    op: 3,
    target: "q",
    label: "Kafka",
    rect: { x: 0, y: 0, w: 60, h: 26 },
  };
  assert.equal(planOp(strike, null, tokens).items.length, 3);
  const dashed: DrawOp = {
    kind: "Edge",
    id: "a>b",
    op: 4,
    from: "a",
    to: "b",
    path: { start: { x: 0, y: 0 }, control: { x: 50, y: -40 }, end: { x: 100, y: 0 } },
    label: "async",
    style: "Dashed",
  };
  const edge = planOp(dashed, null, tokens);
  assert.equal(edge.items[0]?.type === "stroke" && edge.items[0].dashed, true);
  const trace: DrawOp = { kind: "Trace", id: "t", op: 5, points: [] };
  assert.equal(planOp(trace, null, tokens).items.length, 0);
});

test("fills: same subject, same tone; new subjects cycle; detours start two tones along", () => {
  const { board } = scene();
  assert.equal(board.fillFor(box("cdn", "CDN"), 0), "sky");
  assert.equal(board.fillFor(box("lb", "Load balancer"), 0), "lilac");
  assert.equal(board.fillFor(box("pod-1", ""), 0), "mint");
  assert.equal(board.fillFor(box("pod-2", ""), 0), "mint");
  assert.equal(board.fillFor(box("redis", "Redis"), 0), "blush");
  assert.equal(board.fillFor(box("api", "API"), 1), "mint");
  assert.equal(board.fillFor(box("redis", "Redis"), 1), "blush");
  assert.equal(board.fillFor({ ...box("db"), shape: "Db" }, 1), "sky");
  assert.equal(board.fillFor({ ...box("g"), shape: "Group" }, 0), null);
});

test("a repeated id replaces the element instead of duplicating it", () => {
  const { board, apply } = scene();
  apply(start(box("lb", "LB")), 0);
  apply(start(box("lb", "Load balancer")), 3000);
  const runs = board.main.runs;
  assert.equal(runs.length, 2);
  assert.equal(runs[0]?.replacedAt, 3000);
  assert.equal(runs[1]?.replacedAt, null);
});

test("freeze stops the stroke in progress and resume continues from there", () => {
  const { board, apply } = scene();
  apply(start(box("redis", "Redis")), 0);
  const run = board.main.runs[0];
  assert.ok(run !== undefined);
  apply({ type: "Freeze", layer: 0 }, 1200);
  const frozenAt = naturalTime(run.clock, 1200);
  assert.equal(naturalTime(run.clock, 9000), frozenAt);
  assert.equal(runActive(run, 9000), true);
  apply({ type: "Resume", layer: 0, budget_ms: 2000 }, 10_000);
  assert.equal(naturalTime(run.clock, 10_100), frozenAt + 100);
  assert.equal(runActive(run, 60_000), false);
});

test("the first op on the board flies the nib out of the orb; later ones travel", () => {
  const { board, presence, apply } = scene();
  presence.orb("Drawing", 70, 0);
  apply(start(box("a")), tokens.durationOrbCondenseMs);
  apply(start(box("b", "b", 300)), 5000);
  const [first, second] = board.main.runs;
  assert.equal(first?.lead.kind, "flight");
  assert.equal(first?.leadMs, tokens.durationNibFlightMs);
  assert.equal(second?.lead.kind, "travel");
  assert.ok((second?.leadMs ?? 0) > 30 && (second?.leadMs ?? 0) < 300);
});

test("an op that arrives before the orb finished condensing waits for it", () => {
  const { board, apply } = scene();
  apply(start(box("a")), 1000);
  const run = board.main.runs[0];
  assert.equal(run?.lead.kind === "flight" && run.lead.waitMs, tokens.durationOrbCondenseMs);
});

test("detour layers push, pop, and stop accepting more than the board can hold", () => {
  const { board, apply } = scene();
  apply({ type: "LayerPush", layer: 1, title: "cache-aside" }, 100);
  apply(start(box("api"), 500, 1), 200);
  assert.equal(board.openDetours().length, 1);
  apply({ type: "LayerPop", layer: 1 }, 900);
  assert.equal(board.openDetours().length, 0);
  assert.deepEqual(board.detours, [{ openAt: 100, closeAt: 900 }]);
  apply({ type: "LayerPop", layer: 1 }, 950);
  assert.equal(board.counters.orphanEvents, 1, "a second pop is counted, not fatal");
  for (let index = 0; index < LAYER_ELEMENTS_MAX + 5; index += 1) {
    apply(start(box(`n${index}`), 150), 1000 + index);
  }
  assert.equal(board.main.runs.length, LAYER_ELEMENTS_MAX);
  assert.equal(board.counters.droppedOps, 5);
});

test("the nib draws, freezes in place, and flies home once drawing is over", () => {
  const { board, presence, apply } = scene();
  const anchors = { center: [720, 388] as [number, number], dock: [720, 857] as [number, number] };
  const identity = { scale: 1, x: 0, y: 0 };
  const nib: NibFrame = {
    visible: false,
    x: 0,
    y: 0,
    scale: 1,
    down: false,
    frozen: false,
    frozenAt: 0,
  };
  presence.orb("Drawing", 70, 0);
  assert.equal(nibAt(board, presence, 100, identity, anchors, nib).visible, false, "still the orb");
  const opAt = tokens.durationOrbCondenseMs;
  apply(start(box("a")), opAt);
  nibAt(board, presence, opAt, identity, anchors, nib);
  assert.deepEqual([nib.visible, nib.x, nib.y], [true, 720, 388], "leaves from the orb");
  nibAt(board, presence, opAt + tokens.durationNibFlightMs + 100, identity, anchors, nib);
  assert.equal(nib.down, true);
  apply({ type: "Freeze", layer: 0 }, opAt + 900);
  const frozen = nibAt(board, presence, opAt + 900, identity, anchors, nib);
  const at = [frozen.x, frozen.y];
  nibAt(board, presence, opAt + 5000, identity, anchors, nib);
  assert.deepEqual([nib.x, nib.y, nib.frozen], [...at, true]);
  presence.orb("Idle", 0, opAt + 6000);
  assert.equal(homeStart(board, presence, opAt + 6000), null, "a frozen stroke keeps the nib out");
  apply({ type: "Resume", layer: 0, budget_ms: 2000 }, opAt + 7000);
  const end = opAt + 9500;
  assert.ok((homeStart(board, presence, end) ?? Number.NaN) <= end);
  nibAt(board, presence, end + tokens.durationNibHomeMs + 10, identity, anchors, nib);
  assert.equal(nib.visible, false, "home in the dock");
  assert.ok(dockSwell(board, presence, end + 3000) > 0.9);
});

test("presence keeps state changes only, so per-frame level updates don't pile up", () => {
  const { presence } = scene();
  for (let frame = 0; frame < 600; frame += 1) {
    presence.orb("Speaking", frame % 100, frame * 16);
  }
  presence.orb("Listening", 40, 10_000);
  assert.equal(presence.transitions.length, 2);
  assert.equal(presence.notDrawingSince(10_001), 0);
});

test("the orb sits centered, condenses into the nib, then docks", () => {
  const { board, presence } = scene();
  const anchors = { center: [720, 388] as [number, number], dock: [720, 857] as [number, number] };
  const place = { x: 0, y: 0, size: 0, opacity: 0, docked: 0, condense: 0 };
  orbPlacement(board, presence, 0, anchors, place);
  assert.deepEqual([place.x, place.y, place.size], [720, 388, tokens.orbSize]);
  presence.orb("Drawing", 0, 1000);
  orbPlacement(board, presence, 1000 + tokens.durationOrbCondenseMs / 2, anchors, place);
  assert.ok(place.size < tokens.orbSize && place.size > tokens.nibSize);
  orbPlacement(board, presence, 1000 + tokens.durationOrbCondenseMs + 5000, anchors, place);
  assert.deepEqual([place.x, place.y, place.docked], [720, 857, 1]);
  assert.ok(Math.abs(place.size - tokens.orbDockSize) < 0.5);
});

test("the camera fits its frame and eases between frames; focus softens the rest", () => {
  const out = { scale: 1, x: 0, y: 0 };
  fitFrame({ x: 0, y: 0, w: 1440, h: 900 }, { width: 1440, height: 900 }, out);
  assert.deepEqual(out, { scale: 1, x: 0, y: 0 });
  fitFrame({ x: 100, y: 100, w: 720, h: 450 }, { width: 1440, height: 900 }, out);
  assert.deepEqual(out, { scale: 2, x: -200, y: -200 });
  const camera = {
    frame: { x: 100, y: 100, w: 720, h: 450 },
    previous: { x: 0, y: 0, w: 1440, h: 900 },
    at: 1000,
    focus: "redis",
    previousFocus: null,
    focusAt: 1000,
  };
  cameraAt(camera, 1000, { width: 1440, height: 900 }, 900, out);
  assert.equal(out.scale, 1);
  cameraAt(camera, 1900, { width: 1440, height: 900 }, 900, out);
  assert.equal(out.scale, 2);
  assert.equal(depthOfField(camera, 1000, 420), 0);
  assert.equal(depthOfField(camera, 1420, 420), 1);
  assert.equal(depthOfField({ ...camera, focus: null, previousFocus: "redis" }, 1420, 420), 0);
});
