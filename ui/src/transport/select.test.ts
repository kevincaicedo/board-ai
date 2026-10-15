import assert from "node:assert/strict";
import { test } from "node:test";
import { chooseTransport, DEMO_REPLAY_URL } from "./select.ts";

test("inside Tauri with no replay, the UI is live", () => {
  assert.deepEqual(chooseTransport("", true), { kind: "tauri" });
});

test("a replay URL wins, even inside Tauri", () => {
  assert.deepEqual(chooseTransport("?replay=/s/events.jsonl&t=12000", true), {
    kind: "replay",
    url: "/s/events.jsonl",
    seekMs: 12000,
    speed: 1,
  });
});

test("outside Tauri with nothing chosen, the demo plays", () => {
  assert.deepEqual(chooseTransport("", false), {
    kind: "replay",
    url: DEMO_REPLAY_URL,
    seekMs: null,
    speed: 1,
  });
  assert.deepEqual(chooseTransport("?replay=", false), {
    kind: "replay",
    url: DEMO_REPLAY_URL,
    seekMs: null,
    speed: 1,
  });
});

test("bad seek times and speeds fall back instead of failing", () => {
  for (const search of ["?t=", "?t=soon", "?t=-5", "?t=NaN"]) {
    const choice = chooseTransport(search, false);
    assert.equal(choice.kind === "replay" ? choice.seekMs : "live", null, search);
  }
  const fast = chooseTransport("?speed=2", false);
  assert.equal(fast.kind === "replay" ? fast.speed : 0, 2);
  const silly = chooseTransport("?speed=37", false);
  assert.equal(silly.kind === "replay" ? silly.speed : 0, 1);
});
