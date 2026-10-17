// The surface column against the core's caps (review finding 7). The core counts only code,
// compare, recap and doc cards toward its two surfaces (`SURFACES_BESIDE_CANVAS_MAX`) and dismisses them
// itself; a detour card is not a surface, so it stays while its layer is open.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Surface } from "../protocol/generated/Surface.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import {
  ColumnTopsCache,
  columnTops,
  type Placed,
  SURFACES_VISIBLE_MAX,
  surfaceMotion,
} from "./layout.ts";
import { type ColumnItem, DETOUR_CARDS_MAX, SessionStore } from "./session.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);
const code: Surface = {
  kind: "Code",
  id: 0,
  path: "a.go",
  first_line: 1,
  lines: ["x", "y"],
  highlight: null,
};
const compare: Surface = {
  kind: "Compare",
  id: 1,
  options: ["A", "B"],
  criteria: ["c"],
  title: null,
};

const doc: Surface = {
  kind: "Doc",
  id: 2,
  title: "FoundationDB · SIGMOD ’21",
  chip: "p. 3 · Figure 1",
  figure: { label: "Figure 1", caption: "The architecture.", read: true },
  sentences: ["A client asks a proxy.", "The proxy asks the sequencer.", "Reads go to storage."],
  highlight: 0,
  note: "Figure read by Gemini 3.8 Flash.",
};

const detourCard = (store: SessionStore, layer: number): ColumnItem | undefined =>
  store.column.value.find((item) => item.kind === "detour" && item.layer === layer);

/** Whether a card is on screen at `at`, as the column lays it out and fades it. */
function shown(item: ColumnItem | undefined, at: number): boolean {
  if (item === undefined) {
    return false;
  }
  const motion = surfaceMotion(item, at, tokens.durationStandardMs);
  return at >= item.enterAt && motion.exit < 1;
}

test("an open detour keeps its card while the core shows two surfaces", () => {
  const store = new SessionStore(tokens);
  store.apply(
    { type: "LayerPush", layer: 1, title: "cache-aside", question: null, anchor: null },
    1000,
  );
  store.apply({ type: "SurfaceShow", surface: code }, 2000);
  // The core has two surfaces open and sends no dismiss.
  store.apply({ type: "SurfaceShow", surface: compare }, 3000);
  assert.equal(detourCard(store, 1)?.leftAt, null, "the detour still draws into this card");
  assert.equal(shown(detourCard(store, 1), 9000), true);
});

test("a highlight on a code card the core still has open is applied", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "SurfaceShow", surface: code }, 1000);
  store.apply({ type: "SurfaceShow", surface: compare }, 2000);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null, anchor: null }, 3000);
  store.apply({ type: "SurfaceUpdate", id: 0, update: { kind: "Highlight", line: 2 } }, 4000);
  const card = store.column.value.find((item) => item.kind === "surface" && item.surface.id === 0);
  assert.equal(card?.leftAt, null);
  assert.equal(card?.kind === "surface" ? card.highlights.length : -1, 1);
});

test("nested detours cover the outer card and bring it back when they close", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null, anchor: null }, 1000);
  store.apply({ type: "LayerPush", layer: 2, title: "d2", question: null, anchor: null }, 2000);
  store.apply({ type: "LayerPush", layer: 3, title: "d3", question: null, anchor: null }, 3000);
  assert.equal(shown(detourCard(store, 1), 3900), false, "only the innermost detour shows");
  assert.equal(shown(detourCard(store, 2), 4900), false);
  assert.equal(shown(detourCard(store, 3), 4900), true);
  store.apply({ type: "LayerPop", layer: 3 }, 5000);
  assert.equal(shown(detourCard(store, 2), 6000), true, "layer 2 is the active layer again");
  assert.equal(shown(detourCard(store, 1), 6000), false);
  store.apply({ type: "LayerPop", layer: 2 }, 7000);
  const outer = detourCard(store, 1);
  assert.equal(outer?.leftAt, null, "layer 1 is the active layer again");
  assert.equal(shown(outer, 8000), true);
  const tops = columnTops(store.column.value, 9000, tokens);
  assert.equal(
    tops.get("layer-1"),
    tokens.surfaceCenter - tokens.detourCardHeight / 2,
    "alone again, the outer card is centered",
  );
});

test("a covered card leaves its place: the nested detour's card sits alone, centered", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null, anchor: null }, 1000);
  store.apply({ type: "LayerPush", layer: 2, title: "d2", question: null, anchor: null }, 2000);
  const tops = columnTops(store.column.value, 5000, tokens);
  assert.equal(tops.get("layer-2"), tokens.surfaceCenter - tokens.detourCardHeight / 2);
});

test("a third surface steps the oldest surface out, never the detour card", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null, anchor: null }, 0);
  for (let id = 0; id < SURFACES_VISIBLE_MAX + 1; id += 1) {
    store.apply({ type: "SurfaceShow", surface: { ...code, id } }, 1000 * (id + 1));
  }
  assert.deepEqual(
    store.column.value.map((item) => [item.key, item.leftAt]),
    [
      ["layer-1", null],
      ["surface-0", 3000],
      ["surface-1", null],
      ["surface-2", null],
    ],
  );
});

test("a doc card is a surface: beside a code and a compare card, it steps the oldest out", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "SurfaceShow", surface: code }, 1000);
  store.apply({ type: "SurfaceShow", surface: compare }, 2000);
  store.apply({ type: "SurfaceShow", surface: doc }, 3000);
  assert.deepEqual(
    store.column.value.map((item) => [item.key, item.leftAt]),
    [
      ["surface-0", 3000],
      ["surface-1", null],
      ["surface-2", null],
    ],
  );
  // And a fourth steps the compare card out, leaving the doc card.
  store.apply({ type: "SurfaceShow", surface: { ...code, id: 3 } }, 4000);
  assert.deepEqual(
    store.column.value.filter((item) => item.leftAt === null).map((item) => item.key),
    ["surface-2", "surface-3"],
  );
});

test("a doc card's highlight follows Sentence updates; a code card ignores them", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "SurfaceShow", surface: code }, 1000);
  store.apply({ type: "SurfaceShow", surface: doc }, 2000);
  store.apply({ type: "SurfaceUpdate", id: 2, update: { kind: "Sentence", index: 1 } }, 3000);
  store.apply({ type: "SurfaceUpdate", id: 0, update: { kind: "Sentence", index: 1 } }, 3000);
  const card = (id: number) =>
    store.column.value.find((item) => item.kind === "surface" && item.surface.id === id);
  const docCard = card(2);
  assert.deepEqual(docCard?.kind === "surface" ? docCard.highlights : null, [
    { line: 0, at: 2000 },
    { line: 1, at: 3000 },
  ]);
  const codeCard = card(0);
  assert.deepEqual(codeCard?.kind === "surface" ? codeCard.highlights : null, []);
});

test("detour cards stay bounded even if pops never come", () => {
  const store = new SessionStore(tokens);
  for (let layer = 1; layer <= DETOUR_CARDS_MAX + 2; layer += 1) {
    store.apply(
      { type: "LayerPush", layer, title: `d${layer}`, question: null, anchor: null },
      layer * 100,
    );
  }
  const open = store.column.value.filter((item) => item.kind === "detour" && item.leftAt === null);
  assert.equal(open.length, DETOUR_CARDS_MAX);
});

// D-151: a detour card grown for a tall drawing is centered like any lone card, but its top never
// rises above the column's top, so its head stays on the stage.
test("a lone card taller than the column's middle allows starts at the column's top", () => {
  const tall = {
    key: "layer-1",
    shownAt: 0,
    leftAt: null,
    cover: null,
    height: 2 * tokens.surfaceCenter,
  };
  assert.equal(columnTops([tall], 10, tokens).get("layer-1"), tokens.surfaceTop);
  const small = { ...tall, height: tokens.detourCardHeight };
  assert.equal(
    columnTops([small], 10, tokens).get("layer-1"),
    tokens.surfaceCenter - tokens.detourCardHeight / 2,
    "frame 4's card stays centered",
  );
});

// Review 2 (CLAUDE.md, per-frame allocation): the column copied every item to set a detour
// card's grown height, and laid the column out again, on every frame. The height is now looked
// up, and once the column has settled the same tops are handed back until something changes.
test("a settled column hands back the same tops each frame; a change lays it out again", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null, anchor: null }, 1000);
  store.apply({ type: "SurfaceShow", surface: code }, 2000);
  const items = store.column.value;
  const grown = new Map<string, number>([["layer-1", 500]]);
  const height = (item: Placed): number => grown.get(item.key) ?? item.height;
  const cache = new ColumnTopsCache();
  const settled = 2000 + tokens.durationSceneMs + 100;
  const first = cache.tops(items, settled, tokens, height);
  assert.deepEqual(first, columnTops(items, settled, tokens, height));
  assert.equal(cache.tops(items, settled + 16, tokens, height), first, "the same map: no work");
  assert.equal(first.get("surface-0"), tokens.surfaceTop + 500 + tokens.surfaceGap);
  // While cards move, every frame is laid out.
  const moving = 2000 + tokens.durationSceneMs / 2;
  assert.notEqual(cache.tops(items, moving, tokens, height), first);
  // The detour's drawing grew its card: laid out again with the new height.
  grown.set("layer-1", 640);
  const taller = cache.tops(items, settled + 32, tokens, height);
  assert.notEqual(taller, first);
  assert.equal(taller.get("surface-0"), tokens.surfaceTop + 640 + tokens.surfaceGap);
  // A new item list (a card arrived) too.
  store.apply({ type: "SurfaceShow", surface: compare }, 9000);
  const more = cache.tops(store.column.value, 9000 + tokens.durationSceneMs, tokens, height);
  assert.equal(more.has("surface-1"), true);
});
