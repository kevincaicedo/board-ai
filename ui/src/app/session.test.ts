import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Surface } from "../protocol/generated/Surface.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import {
  captionText,
  chipFlight,
  columnTops,
  type Placed,
  suggestionLook,
  surfaceHeight,
} from "./layout.ts";
import { CODE_MARKS_MAX, SessionStore } from "./session.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

test("captions grow word by word, then the finished line moves up when a new one starts", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "Caption", who: "User", text: "Wait", is_final: false }, 100);
  store.apply({ type: "Caption", who: "User", text: "Wait — why", is_final: false }, 300);
  store.apply({ type: "Caption", who: "User", text: "Wait — why", is_final: true }, 500);
  const user = store.captions.value.current;
  assert.deepEqual(
    [user?.text, user?.startAt, user?.wordAt, user?.finalAt],
    ["Wait — why", 100, 300, 500],
  );
  store.apply({ type: "Caption", who: "Assistant", text: "Cache-aside.", is_final: false }, 700);
  assert.equal(store.captions.value.previous?.text, "Wait — why");
  assert.equal(store.captions.value.current?.who, "Assistant");
});

test("a problem is spoken as a caption with its detail on the line above", () => {
  const store = new SessionStore(tokens);
  store.apply(
    { type: "Problem", spoken: "I lost the connection to Claude.", detail: "Network: timeout" },
    50,
  );
  assert.equal(store.captions.value.current?.text, "I lost the connection to Claude.");
  assert.equal(store.captions.value.current?.detail, "Network: timeout");
});

test("long captions keep the tail, with an ellipsis, and split off the newest word", () => {
  assert.deepEqual(
    captionText("To find a user, before touching Postgres, the service checks Redis.", 54),
    {
      // The design's frame 3 shows exactly this line.
      head: "…before touching Postgres, the service checks ",
      last: "Redis.",
    },
  );
  assert.deepEqual(captionText("Sure,", 54), { head: "", last: "Sure," });
});

test("the thread tracks when each step was reached and opens and closes detours", () => {
  const store = new SessionStore(tokens);
  const steps = ["Client", "Edge", "LB"];
  store.apply({ type: "Thread", steps, current: null, detours: [] }, 100);
  store.apply({ type: "Thread", steps, current: 1, detours: [] }, 400);
  store.apply({ type: "Thread", steps, current: 1, detours: ["cache-aside"] }, 900);
  store.apply({ type: "Thread", steps, current: 1, detours: [] }, 2000);
  const thread = store.thread.value;
  assert.deepEqual(thread?.reachedAt, [400, 400, null]);
  assert.equal(thread?.shownAt, 100);
  assert.deepEqual(thread?.detours, [{ title: "cache-aside", openAt: 900, closedAt: 2000 }]);
});

test("at most two surfaces sit beside the canvas; the oldest steps out", () => {
  const store = new SessionStore(tokens);
  const code = (id: number): Surface => ({
    kind: "Code",
    id,
    path: "a.go",
    first_line: 1,
    lines: ["x"],
    highlight: null,
  });
  store.apply(
    { type: "LayerPush", layer: 1, title: "cache-aside", question: "why Redis first?" },
    0,
  );
  store.apply({ type: "SurfaceShow", surface: code(1) }, 2000);
  store.apply({ type: "SurfaceShow", surface: code(2) }, 4000);
  const column = store.column.value;
  assert.deepEqual(
    column.map((item) => [item.key, item.leftAt]),
    [
      ["layer-1", 4000],
      ["surface-1", null],
      ["surface-2", null],
    ],
  );
  const detour = column[0];
  assert.equal(detour?.shownAt, tokens.durationDetourCardDelayMs);
  assert.equal(detour?.kind === "detour" && detour.question, "why Redis first?");
  assert.equal(
    column[2]?.kind === "surface" && column[2].enterAt,
    4000 + tokens.durationSurfaceDelayMs,
  );
  store.apply({ type: "SurfaceUpdate", id: 2, update: { kind: "Highlight", line: 1 } }, 4500);
  const updated = store.column.value[2];
  assert.deepEqual(updated?.kind === "surface" ? updated.highlights : [], [{ line: 1, at: 4500 }]);
  store.apply({ type: "SurfaceUpdate", id: 99, update: { kind: "Highlight", line: 1 } }, 4600);
  assert.equal(store.column.value.length, 3, "an update for an unknown surface is ignored");
});

test("the idle suggestion shows, clears once the user speaks, and a reset forgets it", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "Suggestion", text: "Pick up Raft log compaction?" }, 500);
  assert.deepEqual(store.suggestion.value, {
    text: "Pick up Raft log compaction?",
    shownAt: 500,
    clearedAt: null,
  });
  store.apply({ type: "Suggestion", text: null }, 2050);
  store.apply({ type: "Suggestion", text: null }, 9000);
  assert.equal(store.suggestion.value?.clearedAt, 2050, "the first clear wins");
  store.reset();
  assert.equal(store.suggestion.value, null);
  store.apply({ type: "Suggestion", text: null }, 100);
  assert.equal(store.suggestion.value, null, "nothing to clear");
});

test("code marks and compare cells land on their card, bounded", () => {
  const store = new SessionStore(tokens);
  store.apply(
    {
      type: "SurfaceShow",
      surface: { kind: "Code", id: 1, path: "a.go", first_line: 42, lines: ["x"], highlight: null },
    },
    0,
  );
  for (let index = 0; index < CODE_MARKS_MAX + 3; index += 1) {
    store.apply(
      {
        type: "SurfaceUpdate",
        id: 1,
        update: {
          kind: "Mark",
          line: 42,
          columns: { begin: index, end: index + 1 },
          mark: "Underline",
        },
      },
      100 + index,
    );
  }
  const card = store.column.value[0];
  const marks = card?.kind === "surface" ? card.marks : [];
  assert.equal(marks.length, CODE_MARKS_MAX);
  assert.equal(marks.at(-1)?.columns.begin, CODE_MARKS_MAX + 2, "the newest kept");
  store.apply(
    {
      type: "SurfaceShow",
      surface: {
        kind: "Compare",
        id: 2,
        options: ["Raft", "Paxos"],
        criteria: ["Easy"],
        title: null,
      },
    },
    500,
  );
  store.apply(
    {
      type: "SurfaceUpdate",
      id: 2,
      update: { kind: "Cell", row: 0, col: 1, mark: { kind: "No" } },
    },
    600,
  );
  const compare = store.column.value.find((item) => item.key === "surface-2");
  assert.deepEqual(compare?.kind === "surface" ? [...compare.cells.keys()] : [], ["0:1"]);
});

test("the column centers one card, stacks two, and springs between them", () => {
  const single: Placed = { key: "a", shownAt: 0, leftAt: null, height: 300 };
  assert.equal(columnTops([single], 100, tokens).get("a"), tokens.surfaceCenter - 150);
  const second: Placed = { key: "b", shownAt: 1000, leftAt: null, height: 331 };
  const settled = columnTops([single, second], 1000 + tokens.durationSceneMs, tokens);
  assert.equal(settled.get("a"), tokens.surfaceTop);
  assert.equal(settled.get("b"), tokens.surfaceTop + 300 + tokens.surfaceGap);
  const moving = columnTops([single, second], 1100, tokens).get("a") ?? 0;
  assert.ok(moving < tokens.surfaceCenter - 150 && moving > tokens.surfaceTop);
  const leaving = columnTops(
    [
      { ...single, leftAt: 2000 },
      { ...second, leftAt: 2000 },
    ],
    2200,
    tokens,
  );
  assert.equal(leaving.get("a"), tokens.surfaceTop, "leaving cards blur out where they were");
  const code: Surface = {
    kind: "Code",
    id: 1,
    path: "a",
    first_line: 42,
    lines: Array<string>(11).fill("x"),
    highlight: null,
  };
  assert.equal(surfaceHeight(code, tokens), 331);
});

test("chips wait for BoardAI to confirm out loud, then fly in a stagger", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "Chip", kind: "Note", text: "Cache-aside" }, 33_842);
  store.apply({ type: "Chip", kind: "Task", text: "Check TTLs" }, 35_477);
  store.apply({ type: "Caption", who: "Assistant", text: "Noted.", is_final: false }, 35_927);
  const [note, task] = store.chips.value;
  assert.equal(note?.flyAt, 35_927 + tokens.durationChipConfirmMs);
  assert.equal(task?.flyAt, 35_927 + tokens.durationChipConfirmMs + tokens.durationChipStaggerMs);
  assert.deepEqual([note?.slot, task?.slot], [0, 1]);
  store.apply({ type: "Chip", kind: "Note", text: "Unconfirmed" }, 50_000);
  assert.equal(store.chips.value[2]?.flyAt, 50_000 + tokens.durationChipHoldMaxMs, "never stuck");
});

test("a chip flight rises in, arcs to the target and shrinks into it", () => {
  const origin: [number, number] = [214, 632];
  const target: [number, number] = [1300, 410];
  const begin = chipFlight(origin, target, 0, 2000, 0, 420, 950, 12);
  assert.deepEqual([begin.x, begin.y, begin.opacity, begin.visible], [0, 12, 0, true]);
  const resting = chipFlight(origin, target, 0, 2000, 1000, 420, 950, 12);
  assert.deepEqual([resting.x, resting.y, resting.scale, resting.opacity], [0, 0, 1, 1]);
  const landed = chipFlight(origin, target, 0, 2000, 2950, 420, 950, 12);
  assert.equal(landed.visible, false);
  const middle = chipFlight(origin, target, 0, 2000, 2475, 420, 950, 12);
  assert.ok(middle.y < 0, "the arc rises above the straight line");
});

test("the suggestion rises in out of a blur, then blurs out once cleared", () => {
  const begin = suggestionLook(500, null, 500, tokens);
  assert.deepEqual(begin, { opacity: 0, rise: tokens.suggestionRise, blur: tokens.suggestionBlur });
  const shown = suggestionLook(500, null, 500 + tokens.durationSuggestionInMs, tokens);
  assert.deepEqual(shown, { opacity: 1, rise: 0, blur: 0 });
  const half = suggestionLook(500, 2050, 2050 + tokens.durationSuggestionOutMs / 2, tokens);
  assert.equal(half.opacity, 0.5);
  assert.equal(half.blur, tokens.suggestionBlur / 2);
  const gone = suggestionLook(500, 2050, 2050 + tokens.durationSuggestionOutMs, tokens);
  assert.equal(gone.opacity, 0);
});

test('each spoken "open settings" is one request, kept across a stage reset', () => {
  const store = new SessionStore(tokens);
  assert.equal(store.settingsRequests.value, 0);
  store.apply({ type: "OpenSettings" }, 100);
  store.reset();
  store.apply({ type: "OpenSettings" }, 0);
  assert.equal(store.settingsRequests.value, 2);
});
