import assert from "node:assert/strict";
import { test } from "node:test";
import { type KeyPress, keyDownCommand } from "./keys.ts";

const press = (code: string, extra: Partial<KeyPress> = {}): KeyPress => ({
  code,
  repeat: false,
  modified: false,
  editable: false,
  ...extra,
});
const live = { muted: false, replay: false };

test("Esc stops BoardAI, Space talks, M toggles the mic", () => {
  assert.deepEqual(keyDownCommand(press("Escape"), live), { type: "Stop" });
  assert.deepEqual(keyDownCommand(press("Space"), live), { type: "Mic", action: "TalkDown" });
  assert.deepEqual(keyDownCommand(press("KeyM"), live), { type: "Mic", action: "Mute" });
  assert.deepEqual(keyDownCommand(press("KeyM"), { ...live, muted: true }), {
    type: "Mic",
    action: "Unmute",
  });
  assert.equal(keyDownCommand(press("KeyX"), live), null);
});

test("typing, shortcuts and held keys never control BoardAI; Space belongs to a replay", () => {
  for (const extra of [{ editable: true }, { modified: true }, { repeat: true }]) {
    for (const code of ["Escape", "Space", "KeyM"]) {
      assert.equal(
        keyDownCommand(press(code, extra), live),
        null,
        `${code} ${JSON.stringify(extra)}`,
      );
    }
  }
  assert.equal(keyDownCommand(press("Space"), { ...live, replay: true }), null);
  assert.deepEqual(keyDownCommand(press("Escape"), { ...live, replay: true }), { type: "Stop" });
});
