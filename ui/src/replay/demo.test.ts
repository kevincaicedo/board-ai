import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { emptyArea, stageArea } from "../canvas/camera.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { parseReplayLog } from "../transport/log.ts";
import {
  DEMO_CAMERA_FRAME,
  DEMO_STAGE,
  DEMO_SUGGESTION,
  DESIGN_FRAMES_MS,
  demoJsonl,
} from "./demo.ts";
import { primitivesJsonl } from "./primitives.ts";
import { say } from "./speech.ts";

const committed = readFileSync(
  new URL("../../public/fixtures/demo.events.jsonl", import.meta.url),
  "utf8",
);

test("the committed demo fixture is what the script writes (run src/replay/write-demo.ts)", () => {
  assert.equal(committed, demoJsonl());
});

test("the committed primitives fixture is current and fully valid", () => {
  const primitives = readFileSync(
    new URL("../../public/fixtures/primitives.events.jsonl", import.meta.url),
    "utf8",
  );
  assert.equal(primitives, primitivesJsonl());
  assert.deepEqual(parseReplayLog(primitives).skipped, {
    malformed: 0,
    otherKind: 0,
    overLimit: 0,
  });
});

test("every fixture line is a valid UI event, in time order", () => {
  const log = parseReplayLog(committed);
  assert.deepEqual(log.skipped, { malformed: 0, otherKind: 0, overLimit: 0 });
  const times = committed
    .trim()
    .split("\n")
    .map((line) => (JSON.parse(line) as { t: number }).t);
  assert.deepEqual(
    times,
    [...times].sort((a, b) => a - b),
  );
});

test("the demo's chapters are the design's seven frames", () => {
  const log = parseReplayLog(committed);
  assert.deepEqual(
    log.chapters.map((chapter) => chapter.kind),
    ["Idle", "Listening", "Explaining", "Detour", "Code", "Capture", "Resume"],
  );
  log.chapters.forEach((chapter, index) => {
    const frame = DESIGN_FRAMES_MS[index] ?? 0;
    assert.ok(chapter.at <= frame, `chapter ${index + 1} starts before its key frame`);
  });
  assert.ok(log.durationMs > 44_000 && log.durationMs < 47_000, "about 45 s, like the canvas");
});

test("the demo freezes mid-stroke, detours, and resumes the same stroke", () => {
  const types = committed
    .trim()
    .split("\n")
    .map((line) => (JSON.parse(line) as { event: { type: string } }).event.type);
  const order = ["Freeze", "LayerPush", "SurfaceShow", "Chip", "LayerPop", "Resume"].map((type) =>
    types.indexOf(type),
  );
  assert.deepEqual(
    order,
    [...order].sort((a, b) => a - b),
  );
  assert.ok(order.every((index) => index >= 0));
});

test("the demo's camera frame is the board area at the design's size, so it draws 1:1", () => {
  const tokens = readTokens(
    tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
  );
  const area = stageArea(DEMO_STAGE, tokens, emptyArea());
  assert.deepEqual(DEMO_CAMERA_FRAME, { x: area.x, y: area.y, w: area.w, h: area.h });
  assert.ok(area.maxScale >= 1, "the design's own size is never shrunk");
});

test("the demo shows frame 1's suggestion, the detour's question and frame 5's underline", () => {
  const events = committed
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line) as { t: number; event: Record<string, unknown> });
  const suggestions = events.filter((line) => line.event.type === "Suggestion");
  assert.deepEqual(
    suggestions.map((line) => [line.t, line.event.text]),
    [
      [500, DEMO_SUGGESTION],
      [2050, null],
    ],
  );
  const push = events.find((line) => line.event.type === "LayerPush");
  assert.equal(push?.event.question, "why Redis first?");
  const mark = events.find(
    (line) =>
      line.event.type === "SurfaceUpdate" &&
      (line.event.update as { kind: string }).kind === "Mark",
  );
  const asks = say("Assistant", 28277, "In the code, line 43 asks Redis first.").wordAt[5];
  assert.equal(mark?.t, asks, "the pen starts on 'asks'");
  const nodes = events
    .filter((line) => line.event.type === "OpStart")
    .map((line) => line.event.op as { kind: string; id: string; fill?: string | null })
    .filter((op) => op.kind === "Node")
    .map((op) => `${op.id}:${op.fill}`);
  assert.deepEqual(nodes, [
    "client:null",
    "cdn:Sky",
    "lb:Lilac",
    "api:null",
    "api-1:Mint",
    "api-2:Mint",
    "api-3:Mint",
    "redis:Blush",
    "api:Mint",
    "redis:Blush",
    "postgres:Sky",
    "postgres:Sky",
  ]);
});

test("word timings match the design's speech model", () => {
  // Main.dc.html: 'To find a user, …' starts at 15.797 s; its words land at these times.
  const utterance = say(
    "Assistant",
    15797,
    "To find a user, before touching Postgres, the service checks Redis.",
    { endPauseMs: 100 },
  );
  assert.deepEqual(utterance.wordAt.slice(0, 5), [15797, 15963, 16195, 16328, 16710]);
  assert.equal(utterance.endAt, 19079);
  const cut = say("Assistant", 19159, "If it hits, we sk—", { cutAfterMs: 130 });
  assert.equal(cut.endAt, 20169);
  assert.equal(cut.cut, true);
});
