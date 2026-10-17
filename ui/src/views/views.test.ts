import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fitFrame } from "../canvas/camera.ts";
import type { CoverShape } from "../protocol/generated/CoverShape.ts";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { COVER_BLOBS_MAX, coverFromOps, coverLayout, sketchLayout } from "./cover.ts";
import {
  capitalized,
  clockLabel,
  dayLabel,
  digestCounts,
  durationLabel,
  LIBRARY_HINT_FALLBACK,
  libraryHint,
  noteMeta,
  replaySubtitle,
  sessionMeta,
  shortTitle,
  taskMeta,
} from "./labels.ts";
import {
  captionLetters,
  memoryPanelWidth,
  orbSpot,
  placeForView,
  tasksPanelWidth,
} from "./layout.ts";
import {
  boardBounds,
  framePastBoard,
  PAST_BOARD_MARGIN,
  pastArea,
  pastBoardModel,
  pastFrame,
} from "./pastBoard.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const rect = (x: number, y: number, w: number, h: number) => ({ x, y, w, h });
const card = {
  id: "2026-10-12-160200",
  title: "Raft · log compaction",
  date: "2026-10-12",
  duration_ms: 31 * 60_000,
  cover: [],
};

test("days read relative to the day the view was read", () => {
  assert.equal(dayLabel("2026-10-15", "2026-10-15"), "today");
  assert.equal(dayLabel("2026-10-12", "2026-10-15"), "Oct 12");
  assert.equal(dayLabel("2025-12-31", "2026-01-02"), "Dec 31, 2025");
  assert.equal(dayLabel("", "2026-10-15"), "");
  assert.equal(dayLabel("2026-13-01", "2026-10-15"), "");
  assert.equal(dayLabel("yesterday", "2026-10-15"), "");
  assert.equal(capitalized("today"), "Today");
  assert.equal(capitalized(""), "");
});

test("lengths and replay clocks read like the canvas", () => {
  assert.equal(durationLabel(18 * 60_000), "18 min");
  assert.equal(durationLabel(20_000), "1 min");
  assert.equal(durationLabel(65 * 60_000), "1 h 5 min");
  assert.equal(durationLabel(120 * 60_000), "2 h");
  assert.equal(durationLabel(0), "");
  assert.equal(durationLabel(Number.NaN), "");
  assert.equal(clockLabel(14 * 60_000 + 32_000), "14:32");
  assert.equal(clockLabel(3_725_000), "1:02:05");
  assert.equal(clockLabel(-5), "0:00");
  assert.equal(sessionMeta(card, "2026-10-15"), "Oct 12 · 31 min");
  assert.equal(sessionMeta({ ...card, date: "2026-10-15" }, "2026-10-15"), "Today · 31 min");
  assert.equal(sessionMeta({ ...card, date: "", duration_ms: 0 }, "2026-10-15"), "");
});

test("a replay says what it is, when it was, and that your voice isn't kept (7b)", () => {
  assert.equal(
    replaySubtitle(card, "2026-10-15"),
    "Replay · Oct 12 · your voice isn't kept, only captions",
  );
  assert.equal(
    replaySubtitle({ ...card, date: "" }, "2026-10-15"),
    "Replay · your voice isn't kept, only captions",
  );
  assert.equal(
    replaySubtitle(card, ""),
    "Replay · Oct 12, 2026 · your voice isn't kept, only captions",
  );
});

test("the library names a session you could ask for by its topic", () => {
  assert.equal(shortTitle("Raft · log compaction"), "Raft");
  assert.equal(shortTitle("Request path · API"), "Request path");
  assert.equal(shortTitle("Linux VFS"), "Linux VFS");
  assert.equal(shortTitle("Kafka: consumer groups and offsets"), "Kafka");
  assert.equal(shortTitle("one two three four"), "one two three");
  assert.equal(libraryHint([card]), "Say “play the Raft one,” or pick a card.");
  assert.equal(libraryHint([]), LIBRARY_HINT_FALLBACK);
  assert.equal(libraryHint([{ ...card, title: " " }]), LIBRARY_HINT_FALLBACK);
});

test("rows say where a task or note came from", () => {
  const from = { id: "s", title: "Request path · API", date: "2026-10-15" };
  const task = { id: "t1", title: "Check TTLs", project: "Harness", from };
  assert.equal(taskMeta(task, "2026-10-15"), "Request path · today");
  assert.equal(taskMeta({ ...task, from: null }, "2026-10-15"), "Harness");
  assert.equal(
    taskMeta({ ...task, from: { id: "s", title: "", date: "" } }, "2026-10-15"),
    "Harness",
  );
  const note = { id: "n1", text: "x", date: "2026-10-15", from, links: [] };
  assert.equal(noteMeta(note, "2026-10-15"), "Today · Request path");
  assert.equal(noteMeta({ ...note, from: null, date: "" }, "2026-10-15"), "");
  assert.equal(
    digestCounts({
      session: { ...card, cover: [{ rect: rect(0, 0, 1, 1), shape: "Service" }] },
      notes: 2,
      tasks: 1,
      open_questions: 1,
    }),
    "1 diagram · 2 notes · 1 task · 1 open question",
  );
  assert.equal(digestCounts({ session: card, notes: 0, tasks: 3, open_questions: 0 }), "3 tasks");
});

const SHAPES: CoverShape[] = [
  { rect: rect(0, 0, 400, 200), shape: "Group" },
  { rect: rect(20, 20, 120, 60), shape: "Service" },
  { rect: rect(200, 20, 120, 60), shape: "Service" },
  { rect: rect(20, 120, 80, 80), shape: "Db" },
  { rect: rect(200, 120, 160, 40), shape: "Queue" },
  { rect: rect(0, 0, 0, 10), shape: "Service" },
];

test("cover art is deterministic per session and keeps the board's tones", () => {
  const first = coverLayout(SHAPES, "s1", 400, 300);
  assert.deepEqual(coverLayout(SHAPES, "s1", 400, 300), first);
  assert.notDeepEqual(coverLayout(SHAPES, "s2", 400, 300), first, "another session, another crop");
  assert.deepEqual(
    first.blobs.map((blob) => blob.tone),
    ["porcelain", "sky", "lilac", "sky", "mint"],
    "groups under the rest; services take sky, lilac…; a db is sky, a queue mint (D-031)",
  );
  for (const blob of first.blobs) {
    assert.ok(blob.w > 0 && blob.h > 0 && blob.r <= Math.min(blob.w, blob.h) / 2);
  }
  const many = Array.from({ length: 40 }, (_, index) => ({
    rect: rect(index * 10, 0, 8, 8),
    shape: "Service" as const,
  }));
  assert.equal(coverLayout(many, "s", 400, 300).blobs.length, COVER_BLOBS_MAX);
  assert.deepEqual(coverLayout([], "s", 400, 300).blobs, []);
});

test("a line thumbnail fits the whole board inside its box", () => {
  const boxes = sketchLayout(SHAPES, 76, 56, 7);
  assert.equal(boxes.length, 5);
  for (const box of boxes) {
    assert.ok(box.x >= 6.9 && box.y >= 6.9, JSON.stringify(box));
    assert.ok(box.x + box.w <= 69.1 && box.y + box.h <= 49.1, JSON.stringify(box));
  }
  assert.ok(
    boxes.some((box) => box.round),
    "a database is round",
  );
  assert.deepEqual(sketchLayout([], 76, 56, 7), []);
});

const RAFT: DrawOp[] = [
  {
    kind: "Node",
    id: "leader",
    op: 1,
    shape: "Service",
    label: "leader",
    rect: rect(120, 250, 150, 64),
    stack: 1,
    fill: null,
    link: null,
  },
  {
    kind: "Node",
    id: "follower",
    op: 2,
    shape: "Service",
    label: "follower",
    rect: rect(600, 560, 150, 58),
    stack: 1,
    fill: null,
    link: null,
  },
];

test("a past board is drawn at once, main layer only", () => {
  const board = pastBoardModel(
    [
      { title: null, ops: RAFT },
      { title: "cache-aside", ops: [RAFT[0] as DrawOp] },
    ],
    tokens,
    5000,
  );
  assert.equal(board.layers.size, 1, "detours of the past session are left out");
  assert.equal(board.main.runs.length, 2);
  for (const run of board.main.runs) {
    assert.equal(run.clock.startAt, 5000);
    const end = run.clock.segments[0];
    assert.ok(end !== undefined && end.rate === Number.POSITIVE_INFINITY, "no pacing");
  }
  assert.deepEqual(
    coverFromOps(RAFT).map((shape) => shape.rect),
    [rect(120, 250, 150, 64), rect(600, 560, 150, 58)],
  );
  const bounds = boardBounds(board);
  assert.ok(bounds !== null && bounds.x <= 120 && bounds.x + bounds.w >= 750);
  assert.equal(boardBounds(pastBoardModel([], tokens, 0)), null);
  const camera = () => board.main.camera;
  framePastBoard(board, { width: 0, height: 0 }, 5000);
  assert.equal(camera().frame, null, "not framed before it has a size");
  framePastBoard(board, { width: 400, height: 300 }, 5000);
  framePastBoard(board, { width: 848, height: 900 }, 9000);
  assert.equal(camera().previous, null, "a resize reframes at once, with no move");
  assert.equal(camera().frame?.w, 848);
});

test("a past board is framed to fit, centered, and never enlarged", () => {
  const viewport = { width: 848, height: 900 };
  const small = rect(120, 250, 630, 368);
  const frame = pastFrame(small, viewport);
  const transform = fitFrame(frame, pastArea(viewport), { scale: 0, x: 0, y: 0 });
  assert.ok(Math.abs(transform.scale - 1) < 0.01, "a board that fits keeps its size");
  assert.ok(Math.abs(frame.x + frame.w / 2 - (small.x + small.w / 2)) <= 1, "centered");
  const large = rect(-2000, 0, 6000, 900);
  const wide = pastFrame(large, viewport);
  const shrunk = fitFrame(wide, pastArea(viewport), { scale: 0, x: 0, y: 0 });
  assert.ok(shrunk.scale < 0.15, "a large board is shrunk to fit");
  assert.ok(wide.w >= large.w + 2 * PAST_BOARD_MARGIN);
  assert.ok(
    Math.abs(wide.w / wide.h - viewport.width / viewport.height) < 0.01,
    "the view's aspect",
  );
});

test("views move the orb: the library docks it, panels set it beside them", () => {
  const stage = { width: 1440, height: 900 };
  const dock = orbSpot("Library", stage, tokens);
  assert.deepEqual(dock, { kind: "dock", size: tokens.orbDockSize + tokens.orbDockSwell });
  const beside = orbSpot("Tasks", stage, tokens);
  assert.equal(beside.kind, "aside");
  if (beside.kind === "aside") {
    // The canvas's 9 · Brain: a 440 px panel 32 px from the right; the orb in the middle of the
    // 968 px left of it (D-187).
    assert.equal(beside.free, 968);
    assert.ok(Math.abs(beside.x - 484) < 1);
  }
  const memory = orbSpot("Memory", stage, tokens);
  if (memory.kind === "aside") {
    assert.equal(memory.free, 640);
    assert.ok(Math.abs(memory.x - 320) < 1);
  }
  // The owner's window: still in the middle of the free space, not drifting toward the panels.
  const owner = { width: 2000, height: 1283 };
  for (const kind of ["Tasks", "Memory"] as const) {
    const spot = orbSpot(kind, owner, tokens);
    assert.ok(spot.kind === "aside" && Math.abs(spot.x - spot.free / 2) < 1, kind);
  }
  const narrow = { width: 960, height: 640 };
  assert.ok(tasksPanelWidth(narrow, tokens) <= 960 - tokens.panelRight - tokens.viewFreeMin);
  assert.ok(memoryPanelWidth(narrow, tokens) < tokens.panelMemoryWidth);
  const orb = { x: 720, y: 388, size: 236, opacity: 1, docked: 0, condense: 0 };
  const captions = { shift: 0, width: tokens.captionWidth };
  placeForView(beside, 1, stage, [720, 857], tokens, orb, captions);
  assert.ok(Math.abs(orb.x - 484) < 1 && orb.size === tokens.viewOrbTasks);
  assert.ok(Math.abs(captions.shift + 236) < 1 && captions.width <= 968);
  const docked = { x: 720, y: 857, size: 48, opacity: 1, docked: 1, condense: 0 };
  placeForView(beside, 1, stage, [720, 857], tokens, docked, captions);
  assert.equal(docked.x, 720, "an orb docked for drawing stays docked beside a panel");
  const centered = { x: 720, y: 388, size: 236, opacity: 1, docked: 0, condense: 0 };
  placeForView(dock, 1, stage, [720, 857], tokens, centered, { shift: 0, width: 1000 });
  assert.deepEqual([centered.x, centered.y, centered.size, centered.docked], [720, 857, 48, 1]);
  const untouched = { ...centered, x: 1 };
  placeForView(dock, 0, stage, [720, 857], tokens, untouched, { shift: 0, width: 1000 });
  assert.equal(untouched.x, 1);
});

test("captions keep the design's length, and shorten beside a panel on a small window", () => {
  assert.equal(captionLetters(tokens.captionWidth, tokens), tokens.captionMaxLetters);
  assert.equal(captionLetters(640, tokens), tokens.captionMaxLetters, "the memory frame at 1440");
  assert.ok(captionLetters(360, tokens) < tokens.captionMaxLetters);
  assert.ok(captionLetters(360, tokens) * tokens.typeCaption * 0.52 <= 360);
  assert.equal(captionLetters(10, tokens), 12);
  assert.equal(captionLetters(0, tokens), tokens.captionMaxLetters, "no view: the design's");
});
