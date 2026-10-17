// The staged diagram fixture (15 and 15b) is the core's own output: `sim_staged` writes it from
// the simulator (BOARDAI_WRITE_FIXTURES=1). These tests hold it to its shape, not its numbers,
// which move as the core's layout is tuned.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { DiagramStore, headerView } from "../app/diagrams.ts";
import { planOp } from "../canvas/plan.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { parseReplayLog } from "../transport/log.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const fixture = readFileSync(
  new URL("../../public/fixtures/staged.events.jsonl", import.meta.url),
  "utf8",
);
const events: { t: number; event: UiEvent }[] = parseReplayLog(fixture).batches.flatMap((batch) =>
  batch.events.map((event) => ({ t: batch.at, event })),
);

test("the staged fixture is fully valid", () => {
  assert.deepEqual(parseReplayLog(fixture).skipped, {
    malformed: 0,
    otherKind: 0,
    overLimit: 0,
    rows: 0,
  });
  assert.equal(events.length, fixture.trim().split("\n").length);
});

test("it reveals every kind of part, a detail layer drills in and backs out", () => {
  const parts = new Set<string>();
  const shapes = new Set<string>();
  for (const { event } of events) {
    if (event.type !== "OpStart" || event.op.kind !== "Reveal") {
      continue;
    }
    for (const part of event.op.parts) {
      parts.add(part.part);
      if (part.part === "Node") {
        shapes.add(part.shape);
      }
    }
    // Every reveal plans, so the renderer can draw it.
    assert.ok(planOp(event.op, tokens).items.length > 0);
  }
  for (const kind of ["Frame", "Node", "Note", "Record", "Cells", "Edge", "Legend"]) {
    assert.ok(parts.has(kind), `no ${kind} part`);
  }
  for (const shape of ["Service", "Pill", "Db", "Queue"]) {
    assert.ok(shapes.has(shape), `no ${shape} node`);
  }
  const push = events.find(({ event }) => event.type === "LayerPush");
  assert.ok(push?.event.type === "LayerPush" && push.event.anchor !== null, "a detail layer");
  const layer = push.event.layer;
  const pop = events.find(({ event }) => event.type === "LayerPop" && event.layer === layer);
  assert.ok(pop !== undefined && pop.t > push.t, "it backs out");
  assert.ok(
    events.some(
      ({ event, t }) =>
        event.type === "OpStart" &&
        event.layer === layer &&
        event.op.kind === "Reveal" &&
        t < pop.t,
    ),
    "the detail layer reveals its own diagram",
  );
  assert.ok(
    events.some(({ event }) => event.type === "Camera" && event.enlarge),
    "close-ups on dense parts",
  );
});

test("the header follows the parts, then the path while drilling down", () => {
  const store = new DiagramStore();
  const seen = new Set<string>();
  for (const { event, t } of events) {
    store.apply(event, t);
    const view = headerView(store.headers.value, store.details.value);
    if (view?.kind === "diagram" && view.part !== null) {
      seen.add(view.part);
    }
    if (view?.kind === "detail") {
      seen.add(`${view.crumbs.join(" › ")} | ${view.depth}`);
    }
  }
  assert.ok(seen.has("part 1 of 4") && seen.has("part 4 of 4"), [...seen].join(", "));
  assert.ok(
    [...seen].some((entry) => entry.endsWith("| depth 2 of 4") && entry.includes(" › ")),
    [...seen].join(", "),
  );
});
