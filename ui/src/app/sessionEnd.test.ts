// The stage after the session ends (D-145): the board fades out, cards and the view leave, the
// thread and the suggestion go, and the orb grows back to the center (frame 1) until the next
// session's start resets the stage.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { orbPlacement } from "../canvas/presence.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { Scene } from "./scene.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const anchors = { center: [720, 388] as [number, number], dock: [720, 857] as [number, number] };

function placed(scene: Scene, at: number) {
  return orbPlacement(scene.board, scene.presence, at, anchors, {
    x: 0,
    y: 0,
    size: 0,
    opacity: 0,
    docked: 0,
    condense: 0,
  });
}

/** A session that drew, opened a detour and a recap, and has a view and a suggestion. */
function busyScene(): Scene {
  const scene = new Scene(tokens);
  const events: [number, UiEvent][] = [
    [0, { type: "Suggestion", text: "Pick up Raft where we left off?" }],
    [100, { type: "Orb", state: "Drawing", level_percent: 0 }],
    [
      200,
      {
        type: "OpStart",
        layer: 0,
        op: {
          kind: "Node",
          id: "lb",
          op: 0,
          shape: "Service",
          label: "LB",
          rect: { x: 0, y: 0, w: 150, h: 64 },
          stack: 1,
          fill: null,
          link: null,
        },
        budget_ms: 600,
      },
    ],
    [400, { type: "Thread", steps: ["Client", "LB"], current: 1, detours: ["cache-aside"] }],
    [500, { type: "LayerPush", layer: 1, title: "cache-aside", question: null, anchor: null }],
    [600, { type: "SurfaceShow", surface: { kind: "Recap", id: 0, bullets: ["One", "Two"] } }],
    [
      700,
      { type: "ViewShow", view: { kind: "Library", sessions: [], more_sessions: 0 }, today: "" },
    ],
    [800, { type: "Orb", state: "Idle", level_percent: 0 }],
  ];
  for (const [at, event] of events) {
    scene.apply(event, at);
  }
  return scene;
}

test("the end clears the stage: cards, the view, the thread and the suggestion leave", () => {
  const scene = busyScene();
  const docked = placed(scene, 5_000);
  assert.equal(docked.docked, 1, "the orb is docked while the board is active");
  scene.apply({ type: "SessionEnd" }, 6_000);
  const store = scene.store;
  assert.ok(store.column.value.every((item) => item.leftAt === 6_000));
  assert.equal(store.thread.value, null);
  assert.equal(store.suggestion.value?.clearedAt, 6_000);
  assert.equal(scene.views.open, null);
  assert.equal(scene.board.endedAmount(5_999, tokens.durationSceneMs), 0);
  assert.equal(scene.board.endedAmount(6_000 + tokens.durationSceneMs, tokens.durationSceneMs), 1);
});

test("after the end the orb grows back to the center, as on the idle stage", () => {
  const scene = busyScene();
  scene.apply({ type: "SessionEnd" }, 6_000);
  const leaving = placed(scene, 6_000 + tokens.durationOrbCondenseMs / 2);
  assert.ok(leaving.docked > 0 && leaving.docked < 1, "on its way");
  const back = placed(scene, 6_000 + tokens.durationOrbCondenseMs);
  assert.deepEqual(
    [back.x, back.y, back.size, back.docked],
    [anchors.center[0], anchors.center[1], tokens.orbSize, 0],
  );
});

test("a later end changes nothing, and the next session's start resets the stage", () => {
  const scene = busyScene();
  scene.apply({ type: "SessionEnd" }, 6_000);
  scene.apply({ type: "SessionEnd" }, 9_000);
  assert.equal(scene.board.endedAt, 6_000, "the first end counts");
  assert.equal(scene.presence.restAt, 6_000);
  scene.reset();
  assert.equal(scene.board.endedAt, null);
  assert.equal(scene.presence.restAt, null);
  assert.equal(placed(scene, 10).docked, 0);
});
