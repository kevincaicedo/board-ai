// The surface column against the core's caps (review finding 7). The core counts only code,
// compare and recap toward its two surfaces (`SURFACES_BESIDE_CANVAS_MAX`) and dismisses them
// itself; a detour card is not a surface, so it stays while its layer is open.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Surface } from "../protocol/generated/Surface.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { columnTops, SURFACES_VISIBLE_MAX, surfaceMotion } from "./layout.ts";
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
  store.apply({ type: "LayerPush", layer: 1, title: "cache-aside", question: null }, 1000);
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
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null }, 3000);
  store.apply({ type: "SurfaceUpdate", id: 0, update: { kind: "Highlight", line: 2 } }, 4000);
  const card = store.column.value.find((item) => item.kind === "surface" && item.surface.id === 0);
  assert.equal(card?.leftAt, null);
  assert.equal(card?.kind === "surface" ? card.highlights.length : -1, 1);
});

test("nested detours cover the outer card and bring it back when they close", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null }, 1000);
  store.apply({ type: "LayerPush", layer: 2, title: "d2", question: null }, 2000);
  store.apply({ type: "LayerPush", layer: 3, title: "d3", question: null }, 3000);
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
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null }, 1000);
  store.apply({ type: "LayerPush", layer: 2, title: "d2", question: null }, 2000);
  const tops = columnTops(store.column.value, 5000, tokens);
  assert.equal(tops.get("layer-2"), tokens.surfaceCenter - tokens.detourCardHeight / 2);
});

test("a third surface steps the oldest surface out, never the detour card", () => {
  const store = new SessionStore(tokens);
  store.apply({ type: "LayerPush", layer: 1, title: "d1", question: null }, 0);
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

test("detour cards stay bounded even if pops never come", () => {
  const store = new SessionStore(tokens);
  for (let layer = 1; layer <= DETOUR_CARDS_MAX + 2; layer += 1) {
    store.apply({ type: "LayerPush", layer, title: `d${layer}`, question: null }, layer * 100);
  }
  const open = store.column.value.filter((item) => item.kind === "detour" && item.leftAt === null);
  assert.equal(open.length, DETOUR_CARDS_MAX);
});
