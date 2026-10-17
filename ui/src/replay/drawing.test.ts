import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { parseReplayLog } from "../transport/log.ts";
import { DRAWING_ANSWER_MS, DRAWING_ARTBOARD_MS, drawingJsonl } from "./drawing.ts";

const committed = readFileSync(
  new URL("../../public/fixtures/drawing.events.jsonl", import.meta.url),
  "utf8",
);

test("the committed drawing fixture is current and fully valid (run src/replay/write-demo.ts)", () => {
  assert.equal(committed, drawingJsonl());
  const log = parseReplayLog(committed);
  assert.deepEqual(log.skipped, { malformed: 0, otherKind: 0, overLimit: 0, rows: 0 });
  const kinds = new Set(log.batches.flatMap((batch) => batch.events.map((event) => event.type)));
  for (const kind of ["DrawMode", "UserShape", "LabelHint", "OpStart", "Camera"]) {
    assert.ok(kinds.has(kind as UiEvent["type"]), kind);
  }
  assert.ok(log.chapters.some((chapter) => chapter.kind === "Drawing"));
});

/** Every event up to `at`, in order. */
function until(at: number): UiEvent[] {
  return parseReplayLog(committed)
    .batches.filter((batch) => batch.at <= at)
    .flatMap((batch) => batch.events);
}

test("at the artboard's moment the cache is drawn, unlabelled, and the hint points at it", () => {
  const events = until(DRAWING_ARTBOARD_MS);
  const modes = events.filter((event) => event.type === "DrawMode");
  assert.deepEqual(modes.at(-1), { type: "DrawMode", drawing: true });
  const hints = events.filter((event) => event.type === "LabelHint");
  const hint = hints.at(-1);
  assert.ok(hint?.type === "LabelHint" && hint.hint?.target === "u6");
  const cache = events.filter((event) => event.type === "UserShape" && event.shape.id === "u6");
  assert.ok(
    cache.length > 0 &&
      cache.every((event) => event.type === "UserShape" && event.shape.label === null),
  );
  // Every arrow drawn before its target is connected by then, as the core connects it.
  const arrows = new Map<string, string | null>();
  for (const event of events) {
    if (event.type === "UserShape" && event.shape.kind.kind === "Arrow") {
      arrows.set(event.shape.id, event.shape.kind.to);
    }
  }
  assert.deepEqual(
    [...arrows],
    [
      ["u2", "u3"],
      ["u4", "u5"],
    ],
  );
});

test("BoardAI answers only after you finish, and draws beside your shapes, never as them", () => {
  const events = until(DRAWING_ANSWER_MS);
  const off = events.findIndex((event) => event.type === "DrawMode" && !event.drawing);
  const firstOp = events.findIndex((event) => event.type === "OpStart");
  assert.ok(off >= 0 && firstOp > off);
  const yours = new Set(
    events.flatMap((event) => (event.type === "UserShape" ? [event.shape.id] : [])),
  );
  for (const event of events) {
    if (event.type === "OpStart" && "id" in event.op) {
      assert.ok(!yours.has(event.op.id), `BoardAI redrew ${event.op.id}`);
    }
  }
});
