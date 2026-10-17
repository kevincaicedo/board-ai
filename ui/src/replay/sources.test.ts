import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { parseReplayLog } from "../transport/log.ts";
import {
  SOURCES_ARTBOARD_MS,
  SOURCES_DROP_MS,
  SOURCES_STAGED_MS,
  sourcesJsonl,
} from "./sources.ts";

const committed = readFileSync(
  new URL("../../public/fixtures/sources.events.jsonl", import.meta.url),
  "utf8",
);

function until(at: number): UiEvent[] {
  return parseReplayLog(committed)
    .batches.filter((batch) => batch.at <= at)
    .flatMap((batch) => batch.events);
}

test("the committed sources fixture is current and fully valid (run src/replay/write-demo.ts)", () => {
  assert.equal(committed, sourcesJsonl());
  const log = parseReplayLog(committed);
  assert.deepEqual(log.skipped, { malformed: 0, otherKind: 0, overLimit: 0, rows: 0 });
});

test("at the artboard's moment the tray is open on inf-alloc being read, with ghosts", () => {
  const trays = until(SOURCES_ARTBOARD_MS).filter((event) => event.type === "Sources");
  const last = trays.at(-1);
  assert.ok(last?.type === "Sources" && last.tray.open);
  assert.equal(last.tray.session[0]?.state.state, "Reading");
  assert.equal(last.tray.reading.length, 4);
});

test("the drop overlay shows the right half, then the dropped paper is kept and waits", () => {
  const drops = until(SOURCES_DROP_MS).filter((event) => event.type === "SourceDrop");
  const last = drops.at(-1);
  assert.ok(last?.type === "SourceDrop" && last.drop?.half === "Project");
  const after = until(13_000)
    .filter((event) => event.type === "Sources")
    .at(-1);
  assert.ok(after?.type === "Sources");
  assert.equal(after.tray.project?.sources.at(-1)?.title, "hekaton-sigmod13.pdf");
});

test("the brief's diagram is staged with its chip, and the tray opens again beside it", () => {
  const header = until(SOURCES_STAGED_MS)
    .filter((event) => event.type === "Diagram")
    .at(-1);
  assert.ok(header?.type === "Diagram");
  assert.deepEqual(header.provenance, {
    reader: "Claude Code",
    source: "inf-alloc",
    files_read: 23,
  });
  const last = until(13_000)
    .filter((event) => event.type === "Sources")
    .at(-1);
  assert.ok(last?.type === "Sources" && last.tray.open);
});
