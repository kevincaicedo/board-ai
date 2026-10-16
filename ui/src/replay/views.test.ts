import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { parseReplayLog } from "../transport/log.ts";
import { sessionFixtures, VIEW_FRAMES_MS, viewsJsonl } from "./views.ts";

const fixture = (name: string): string =>
  readFileSync(new URL(`../../public/fixtures/${name}.events.jsonl`, import.meta.url), "utf8");

const committed = fixture("views");

function events(text: string): { t: number; event: UiEvent }[] {
  return parseReplayLog(text).batches.flatMap((batch) =>
    batch.events.map((event) => ({ t: batch.at, event })),
  );
}

test("the committed views fixture is what the script writes (run src/replay/write-demo.ts)", () => {
  assert.equal(committed, viewsJsonl());
  assert.deepEqual(parseReplayLog(committed).skipped, { malformed: 0, otherKind: 0, overLimit: 0 });
});

test("the views fixture opens every view, replacing and closing them in order", () => {
  const all = events(committed);
  assert.equal(all[0]?.event.type, "SessionStart");
  const views = all.flatMap(({ t, event }) =>
    event.type === "ViewShow"
      ? [`${event.view.kind}`]
      : event.type === "ViewDismiss"
        ? [`close@${t}`]
        : [],
  );
  assert.deepEqual(
    views.map((entry) => entry.replace(/@\d+/, "")),
    ["Library", "Tasks", "Memory", "PastSession", "close", "Replay"],
  );
  const shown = (kind: string): number =>
    all.find(({ event }) => event.type === "ViewShow" && event.view.kind === kind)?.t ?? -1;
  assert.ok(shown("Library") < VIEW_FRAMES_MS.library);
  assert.ok(shown("Tasks") < VIEW_FRAMES_MS.tasks && VIEW_FRAMES_MS.library < shown("Tasks"));
  assert.ok(shown("Memory") < VIEW_FRAMES_MS.memory);
  assert.ok(shown("PastSession") < VIEW_FRAMES_MS.recall);
});

test("each card a fixture can play has a session log, current and valid", () => {
  const ids = new Set<string>();
  for (const { event } of events(committed)) {
    if (event.type === "ViewShow" && event.view.kind === "Library") {
      for (const session of event.view.sessions) {
        ids.add(session.id);
      }
    }
  }
  const logs = sessionFixtures();
  assert.ok(logs.length >= 1);
  for (const [id, text] of logs) {
    assert.ok(ids.has(id), `${id} is a library card`);
    assert.equal(fixture(`sessions/${id}`), text, `sessions/${id} is current`);
    const log = parseReplayLog(text);
    assert.deepEqual(log.skipped, { malformed: 0, otherKind: 0, overLimit: 0 });
    const first = log.batches[0]?.events[0];
    assert.deepEqual(first, { type: "SessionStart", session: id });
    assert.ok(log.batches.some((batch) => batch.events.some((event) => event.type === "OpStart")));
  }
});
