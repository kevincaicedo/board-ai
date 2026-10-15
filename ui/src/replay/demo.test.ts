import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parseReplayLog } from "../transport/log.ts";
import { DESIGN_FRAMES_MS, demoJsonl } from "./demo.ts";
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
