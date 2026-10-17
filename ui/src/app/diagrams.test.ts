import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { type BoardEvent, BoardModel } from "../canvas/board.ts";
import { boxAt, detailBox, detailCard, detailOpen, screenBox } from "../canvas/detail.ts";
import { emptyCard } from "../canvas/detourCard.ts";
import { leadDecision, Presence } from "../canvas/presence.ts";
import { boardPush } from "../canvas/push.ts";
import type { DetailAnchor } from "../protocol/generated/DetailAnchor.ts";
import type { DiagramProvenance } from "../protocol/generated/DiagramProvenance.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import {
  creditLine,
  DETAIL_LEVELS_MAX,
  DiagramStore,
  detailTitle,
  headerView,
  MAIN_BOARD_CRUMB,
} from "./diagrams.ts";
import { SessionStore } from "./session.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const anchor: DetailAnchor = {
  layer: 0,
  element: "arena",
  rect: { x: 22, y: 14, w: 336, h: 640 },
  category: "Slate",
  display: null,
};
const push = (layer: number, title: string, on = 0): UiEvent => ({
  type: "LayerPush",
  layer,
  title,
  question: null,
  anchor: { ...anchor, layer: on },
});
const header = (
  layer: number,
  part: number,
  provenance: DiagramProvenance | null = null,
): UiEvent => ({
  type: "Diagram",
  layer,
  title: "inf-alloc · memory allocation",
  part,
  parts: 4,
  provenance,
});
const fromBrief: DiagramProvenance = {
  reader: "Claude Code",
  source: "inf-alloc",
  files_read: 23,
};

test("the header names the diagram and its part, then the path while drilling down", () => {
  const store = new DiagramStore();
  assert.equal(headerView(store.headers.value, store.details.value), null);
  store.apply(header(0, 0), 0);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "diagram",
    title: "inf-alloc · memory allocation",
    part: null,
    credit: null,
  });
  store.apply(header(0, 3), 100);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "diagram",
    title: "inf-alloc · memory allocation",
    part: "part 3 of 4",
    credit: null,
  });
  store.apply(push(1, "Arena"), 200);
  store.apply(header(1, 1), 250);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "detail",
    crumbs: ["inf-alloc · memory allocation", "Arena"],
    depth: `depth 2 of ${DETAIL_LEVELS_MAX}`,
    credit: null,
  });
  store.apply(push(2, "Free list", 1), 300);
  assert.deepEqual(headerView(store.headers.value, store.details.value)?.kind, "detail");
  store.apply({ type: "LayerPop", layer: 2 }, 400);
  store.apply({ type: "LayerPop", layer: 1 }, 500);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "diagram",
    title: "inf-alloc · memory allocation",
    part: "part 3 of 4",
    credit: null,
  });
  // A page turn leaves a fresh page with no diagram; the session's end clears everything.
  store.apply({ type: "PageTurn", page: 2, title: null }, 600);
  assert.equal(headerView(store.headers.value, store.details.value), null);
  store.apply(push(3, "Region"), 700);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "detail",
    crumbs: [MAIN_BOARD_CRUMB, "Region"],
    depth: `depth 2 of ${DETAIL_LEVELS_MAX}`,
    credit: null,
  });
  store.apply({ type: "SessionEnd" }, 800);
  assert.equal(headerView(store.headers.value, store.details.value), null);
  // Closed details are kept for their exit, within a bound.
  for (let layer = 10; layer < 20; layer += 1) {
    store.apply(push(layer, `d${layer}`), layer * 100);
    store.apply({ type: "LayerPop", layer }, layer * 100 + 50);
  }
  assert.ok(store.details.value.length <= DETAIL_LEVELS_MAX + 1);
});

test("a detail layer on a source's part shows its label in the crumbs, its title being its id", () => {
  // D-290: the core titles the layer by the part's id (the title reaches the talker's context,
  // I18) and sends the part's label for the breadcrumbs alone.
  const onPart = (layer: number, id: string, display: string | null, on = 0): UiEvent => ({
    type: "LayerPush",
    layer,
    title: id,
    question: null,
    anchor: { ...anchor, layer: on, element: id, display },
  });
  assert.equal(detailTitle("inf_alloc_arena", { ...anchor, display: "Arena" }), "Arena");
  assert.equal(detailTitle("Arena", anchor), "Arena", "no label: the title");
  const store = new DiagramStore();
  store.apply(header(0, 2, fromBrief), 0);
  store.apply(onPart(1, "inf_alloc_arena", "Arena allocator"), 100);
  store.apply(onPart(2, "free_list", null, 1), 200);
  assert.deepEqual(headerView(store.headers.value, store.details.value), {
    kind: "detail",
    crumbs: ["inf-alloc · memory allocation", "Arena allocator", "free_list"],
    depth: `depth 3 of ${DETAIL_LEVELS_MAX}`,
    credit: null,
  });
  assert.deepEqual(
    store.details.value.map((detail) => detail.title),
    ["Arena allocator", "free_list"],
    "the detail stage names it the same way",
  );
});

test("a brief's diagram names who read the code, on the board and on its detail layer", () => {
  assert.equal(creditLine(null), null);
  assert.equal(creditLine(fromBrief), "from Claude Code's brief · 23 files read");
  assert.equal(
    creditLine({ ...fromBrief, files_read: 1 }),
    "from Claude Code's brief · 1 file read",
  );
  assert.equal(creditLine({ ...fromBrief, files_read: 0 }), "from Claude Code's brief");
  assert.equal(
    creditLine({ ...fromBrief, files_read: null }),
    "from Claude Code's brief · read earlier",
  );
  const store = new DiagramStore();
  store.apply(header(0, 0, fromBrief), 0);
  assert.equal(
    headerView(store.headers.value, store.details.value)?.credit,
    "from Claude Code's brief · 23 files read",
  );
  // The detail layer in front shows its own diagram's chip; the board's stays in the crumbs.
  store.apply(push(1, "Arena"), 100);
  assert.equal(headerView(store.headers.value, store.details.value)?.credit, null);
  store.apply(header(1, 0, { ...fromBrief, files_read: 4 }), 150);
  assert.equal(
    headerView(store.headers.value, store.details.value)?.credit,
    "from Claude Code's brief · 4 files read",
  );
});

test("a detour's push carries no detail, so the header stays the diagram's", () => {
  const store = new DiagramStore();
  store.apply(header(0, 2), 0);
  store.apply({ type: "LayerPush", layer: 1, title: "Why", question: null, anchor: null }, 10);
  assert.equal(headerView(store.headers.value, store.details.value)?.kind, "diagram");
});

test("a detail layer is no detour: no card in the column, no detour push-back span", () => {
  const session = new SessionStore(tokens);
  session.apply(push(1, "Arena"), 0);
  assert.deepEqual(session.column.value, []);
  session.apply({ type: "LayerPop", layer: 1 }, 100);
  assert.deepEqual(session.column.value, []);
  const board = new BoardModel(tokens);
  const lead = leadDecision(board, new Presence(tokens));
  board.apply(push(1, "Arena") as BoardEvent, 0, lead);
  assert.equal(board.detours.length, 0);
  assert.deepEqual(
    board.openDetails().map((layer) => layer.id),
    [1],
  );
  assert.deepEqual(board.openDetours(), []);
  board.apply({ type: "LayerPop", layer: 1 }, 100, lead);
  assert.deepEqual(board.openDetails(), []);
});

test("the detail card fills 15b's board area and grows out of its anchor", () => {
  const stage = { width: 1440, height: 900 };
  assert.deepEqual(detailBox(stage, tokens, { x: 0, y: 0, w: 0, h: 0 }), {
    x: 60,
    y: 70,
    w: 1320,
    h: 660,
  });
  const card = detailCard(stage, tokens, emptyCard());
  assert.equal(card.width, 1320);
  assert.equal(card.height, 660);
  assert.deepEqual(card.area, {
    x: tokens.detailBoardInset,
    y: tokens.detailBoardInset,
    w: 1320 - 2 * tokens.detailBoardInset,
    h: 660 - 2 * tokens.detailBoardInset,
    maxScale: tokens.cameraScaleMax,
  });
  const scene = tokens.durationSceneMs;
  assert.equal(detailOpen(1000, null, 1000, scene), 0);
  assert.equal(detailOpen(1000, null, 1000 + scene, scene), 1);
  assert.equal(detailOpen(1000, 5000, 5000 + scene, scene), 0);
  const from = screenBox(anchor.rect, { scale: 0.5, x: 100, y: 120 }, { x: 0, y: 0, w: 0, h: 0 });
  assert.deepEqual(from, { x: 111, y: 127, w: 168, h: 320 });
  const to = detailBox(stage, tokens, { x: 0, y: 0, w: 0, h: 0 });
  const out = { x: 0, y: 0, w: 0, h: 0 };
  assert.deepEqual(boxAt(from, to, 0, out), from);
  assert.deepEqual(boxAt(from, to, 1, out), to);
});

test("the board steps back around a detail's anchor, and as before for a detour", () => {
  const board = new BoardModel(tokens);
  const lead = leadDecision(board, new Presence(tokens));
  const out = new Float64Array(5);
  const scratch = { x: 0, y: 0, w: 0, h: 0 };
  const identity = { scale: 1, x: 0, y: 0 };
  const scene = tokens.durationSceneMs;
  board.apply(push(1, "Arena") as BoardEvent, 0, lead);
  boardPush(board, scene, tokens, false, identity, scratch, out);
  assert.deepEqual(Array.from(out), [0, 0, 1, 190, 334]);
  board.apply({ type: "LayerPop", layer: 1 }, scene, lead);
  boardPush(board, 2 * scene, tokens, false, identity, scratch, out);
  assert.equal(out[2], 0, "closed, the board comes back");
  board.apply(
    { type: "LayerPush", layer: 2, title: "Why", question: null, anchor: null },
    3 * scene,
    lead,
  );
  boardPush(board, 4 * scene, tokens, false, identity, scratch, out);
  assert.equal(out[0], 1);
  boardPush(board, 4 * scene, tokens, true, identity, scratch, out);
  assert.equal(out[0], 0, "the board you draw on stays sharp");
});
