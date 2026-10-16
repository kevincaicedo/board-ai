import assert from "node:assert/strict";
import { test } from "node:test";
import { type KeyPress, keyDownCommand, opensSettings, PushToTalk } from "./keys.ts";

const press = (code: string, extra: Partial<KeyPress> = {}): KeyPress => ({
  code,
  repeat: false,
  modified: false,
  editable: false,
  ...extra,
});
const live = { muted: false, replay: false, escape: "none" } as const;

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

test("Esc closes what is open first: a card's replay, then the view, then it stops BoardAI", () => {
  const pressEscape = (context: "close-replay" | "close-view" | "none") =>
    keyDownCommand(press("Escape"), { ...live, escape: context });
  assert.equal(pressEscape("close-replay"), null, "the UI closes its own replay; nothing is sent");
  assert.deepEqual(pressEscape("close-view"), { type: "CloseView" });
  assert.deepEqual(pressEscape("none"), { type: "Stop" });
});

test("⌘, opens settings; a comma alone or a held key does not", () => {
  assert.equal(opensSettings({ code: "Comma", meta: true, repeat: false }), true);
  assert.equal(opensSettings({ code: "Comma", meta: false, repeat: false }), false);
  assert.equal(opensSettings({ code: "Comma", meta: true, repeat: true }), false);
  assert.equal(opensSettings({ code: "Period", meta: true, repeat: false }), false);
});

// Review finding 20: with no blur handler, ⌘-Tab while holding Space sent the key-up to another
// app, the TalkUp was lost, and the core then waited out the 30 s utterance cap on every turn.
test("a held Space ends with exactly one TalkUp, even when its key-up never comes", () => {
  const talk = new PushToTalk();
  assert.deepEqual(talk.down(press("Space"), live), { type: "Mic", action: "TalkDown" });
  assert.equal(talk.held, true);
  // The window lost focus (⌘-Tab while holding): the hold ends here.
  assert.deepEqual(talk.release(), { type: "Mic", action: "TalkUp" });
  assert.equal(talk.held, false);
  assert.equal(talk.up("Space"), null, "the late key-up sends nothing more");
  assert.equal(talk.release(), null);
});

test("other keys while holding Space don't end the hold; Space's key-up does", () => {
  const talk = new PushToTalk();
  talk.down(press("Space"), live);
  assert.deepEqual(talk.down(press("KeyM"), live), { type: "Mic", action: "Mute" });
  assert.equal(talk.down(press("Space", { repeat: true }), live), null);
  assert.equal(talk.up("KeyM"), null);
  assert.equal(talk.held, true);
  assert.deepEqual(talk.up("Space"), { type: "Mic", action: "TalkUp" });
  assert.equal(talk.up("Space"), null, "a key-up without a hold sends nothing");
});
