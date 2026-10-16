import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { View } from "../protocol/generated/View.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { escapeAction, REPLAY_PROGRESS_MAX, Replays } from "./replays.ts";
import { Scene } from "./scene.ts";
import {
  entranceOf,
  VIEW_ENTRIES_MAX,
  VIEW_LEAVE_LINGER_MS,
  ViewStore,
  viewMotion,
  viewVisible,
} from "./views.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const durations = { sceneMs: 900, standardMs: 420 };
const card = { id: "s1", title: "Raft", date: "2026-10-12", duration_ms: 60_000, cover: [] };
const library: View = { kind: "Library", sessions: [card], more_sessions: 0 };
const tasks: View = { kind: "Tasks", project: null, tasks: [], more_tasks: 0, notes: [] };

test("a shown view is open until it is dismissed or replaced", () => {
  const store = new ViewStore();
  // Read through a function: an assertion on the getter would narrow it for the next line.
  const open = () => store.open;
  assert.equal(open(), null);
  store.show(library, "2026-10-15", 1000);
  assert.equal(open()?.view.kind, "Library");
  store.show(tasks, "2026-10-15", 2000);
  assert.equal(open()?.view.kind, "Tasks");
  assert.deepEqual(
    store.entries.value.map((entry) => [entry.view.kind, entry.shownAt, entry.leftAt]),
    [
      ["Library", 1000, 2000],
      ["Tasks", 2000, null],
    ],
  );
  store.dismiss(3000);
  assert.equal(open(), null);
  assert.equal(store.entries.value[1]?.leftAt, 3000);
  store.dismiss(3500);
  assert.equal(store.entries.value[1]?.leftAt, 3000, "a second dismiss changes nothing");
});

test("views that finished leaving are dropped, and the list stays bounded", () => {
  const store = new ViewStore();
  store.show(library, "2026-10-15", 0);
  store.dismiss(100);
  store.show(tasks, "2026-10-15", 100 + VIEW_LEAVE_LINGER_MS + 1);
  assert.deepEqual(
    store.entries.value.map((entry) => entry.view.kind),
    ["Tasks"],
  );
  for (let index = 0; index < 20; index += 1) {
    store.show(library, "2026-10-15", 5000 + index);
  }
  assert.ok(store.entries.value.length <= VIEW_ENTRIES_MAX);
  assert.equal(
    new Set(store.entries.value.map((entry) => entry.serial)).size,
    store.entries.value.length,
  );
  store.reset();
  assert.deepEqual(store.entries.value, []);
});

test("panels slide in over a scene duration; the rest rise over the standard one", () => {
  assert.equal(entranceOf("Tasks"), "panel");
  assert.equal(entranceOf("Memory"), "panel");
  assert.equal(entranceOf("Library"), "rise");
  assert.equal(entranceOf("PastSession"), "rise");
  assert.equal(entranceOf("Replay"), "rise");
  const store = new ViewStore();
  store.show(tasks, "2026-10-15", 1000);
  const panel = store.open;
  assert.ok(panel !== null);
  assert.equal(viewMotion(panel, 1000, durations).enter, 0);
  assert.ok(viewMotion(panel, 1420, durations).enter < 1);
  assert.equal(viewMotion(panel, 1900, durations).enter, 1);
  store.dismiss(3000);
  const leaving = store.entries.value[0];
  assert.ok(leaving !== undefined);
  assert.equal(viewMotion(leaving, 3420, durations).exit, 1);
  assert.ok(viewVisible(leaving, 3200, durations));
  assert.ok(!viewVisible(leaving, 3420, durations));
  assert.ok(!viewVisible(leaving, 999, durations), "not before it was shown");
});

test("the scene routes view events, and a reset empties them", () => {
  const scene = new Scene(tokens);
  scene.apply({ type: "ViewShow", view: library, today: "2026-10-15" }, 500);
  assert.equal(scene.views.open?.today, "2026-10-15");
  scene.apply({ type: "SessionStart", session: "s2" }, 600);
  assert.equal(scene.views.open?.view.kind, "Library", "the transport resets, not the event");
  scene.apply({ type: "ViewDismiss" }, 700);
  assert.equal(scene.views.open, null);
  scene.apply({ type: "ViewShow", view: tasks, today: "2026-10-15" }, 800);
  scene.reset();
  assert.deepEqual(scene.views.entries.value, []);
});

test("replay progress only moves forward, in whole percents, bounded", () => {
  const replays = new Replays();
  replays.record("s1", 0.256);
  replays.record("s1", 0.1);
  replays.record("s1", Number.NaN);
  assert.equal(replays.progressOf("s1"), 0.25, "a replay watched again from the start keeps 25%");
  replays.record("s1", 7);
  assert.equal(replays.progressOf("s1"), 1);
  replays.record("s2", 0.249);
  assert.equal(replays.progressOf("s2"), 0.24);
  assert.equal(replays.progressOf("unknown"), 0);
  for (let index = 0; index <= REPLAY_PROGRESS_MAX; index += 1) {
    replays.record(`many-${index}`, 0.5);
  }
  assert.equal(replays.progress.value.size, REPLAY_PROGRESS_MAX);
  assert.equal(replays.progressOf("s1"), 0, "the oldest is forgotten first");
  replays.open(card, "library");
  assert.equal(replays.local.value?.from, "library");
  replays.close();
  assert.equal(replays.local.value, null);
});

test("Esc leaves a card's replay first, then closes the core's view", () => {
  assert.equal(escapeAction(true, true), "close-replay");
  assert.equal(escapeAction(true, false), "close-replay");
  assert.equal(escapeAction(false, true), "close-view");
  assert.equal(escapeAction(false, false), "none");
});
