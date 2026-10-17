import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { parseReplayLog } from "../transport/log.ts";
import {
  SOURCES_ARTBOARD_MS,
  SOURCES_DOC_MS,
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

test("at the artboard's moment the tray is open on inf-alloc being read, the paper read", () => {
  const trays = until(SOURCES_ARTBOARD_MS).filter((event) => event.type === "Sources");
  const last = trays.at(-1);
  assert.ok(last?.type === "Sources" && last.tray.open);
  assert.equal(last.tray.session[0]?.state.state, "Reading");
  assert.equal(last.tray.reading.length, 4);
  assert.deepEqual(last.tray.session[1]?.state, {
    state: "Ready",
    detail: "14 pages · text, 4 tables and 9 of 9 figures read · figures by Gemini 3.8 Flash",
  });
  // Before it was ready, the paper counted its pages.
  const counted = trays.flatMap((event) =>
    event.type === "Sources" ? event.tray.session.filter((row) => row.id === 2) : [],
  );
  assert.ok(
    counted.some(
      (row) =>
        row.state.state === "Reading" &&
        row.state.files_read > 0 &&
        row.state.files_estimate === 14,
    ),
  );
});

test("the drop overlay shows the right half, then the dropped paper is kept and read", () => {
  const drops = until(SOURCES_DROP_MS).filter((event) => event.type === "SourceDrop");
  const last = drops.at(-1);
  assert.ok(last?.type === "SourceDrop" && last.drop?.half === "Project");
  const after = until(13_500)
    .filter((event) => event.type === "Sources")
    .at(-1);
  assert.ok(after?.type === "Sources");
  const kept = after.tray.project?.sources.at(-1);
  assert.equal(kept?.title, "hekaton-sigmod13.pdf");
  assert.deepEqual(kept?.state, {
    state: "Partial",
    reason: "Text read · 3 figures need a model that reads files",
  });
});

test("14b: the Doc card shows Figure 1 of page 3, its highlight following the voice", () => {
  const events = until(SOURCES_DOC_MS);
  const shown = events.find((event) => event.type === "SurfaceShow");
  assert.ok(shown?.type === "SurfaceShow" && shown.surface.kind === "Doc");
  assert.equal(shown.surface.chip, "p. 3 · Figure 1");
  assert.equal(shown.surface.figure?.read, true);
  assert.equal(shown.surface.highlight, 0);
  const moves = events.filter((event) => event.type === "SurfaceUpdate");
  assert.deepEqual(
    moves.map((event) => (event.type === "SurfaceUpdate" ? event.update : null)),
    [{ kind: "Sentence", index: 1 }],
  );
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
