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

const drawingNow = { ...live, drawing: true } as const;

test("D starts drawing, muted too (you draw silently); not in a replay or already drawing", () => {
  assert.deepEqual(keyDownCommand(press("KeyD"), live), {
    type: "Draw",
    action: { kind: "Start" },
  });
  assert.deepEqual(keyDownCommand(press("KeyD"), { ...live, muted: true }), {
    type: "Draw",
    action: { kind: "Start" },
  });
  for (const context of [{ ...live, replay: true }, drawingNow]) {
    assert.equal(keyDownCommand(press("KeyD"), context), null, JSON.stringify(context));
  }
  assert.equal(keyDownCommand(press("KeyD", { editable: true }), live), null);
  assert.equal(keyDownCommand(press("KeyD", { repeat: true }), live), null);
});

test("while you draw, Enter is Done, Esc leaves drawing and ⌘Z undoes", () => {
  assert.deepEqual(keyDownCommand(press("Enter"), drawingNow), {
    type: "Draw",
    action: { kind: "Done" },
  });
  assert.equal(keyDownCommand(press("Enter"), live), null, "Enter means nothing otherwise");
  assert.deepEqual(keyDownCommand(press("Escape"), drawingNow), {
    type: "Draw",
    action: { kind: "Cancel" },
  });
  // A view open over the board still closes first.
  assert.deepEqual(keyDownCommand(press("Escape"), { ...drawingNow, escape: "close-view" }), {
    type: "CloseView",
  });
  const undo = press("KeyZ", { modified: true, shortcut: true });
  assert.deepEqual(keyDownCommand(undo, drawingNow), { type: "Draw", action: { kind: "Undo" } });
  assert.equal(keyDownCommand(undo, live), null, "⌘Z is not ours outside drawing");
  assert.equal(keyDownCommand(press("KeyZ", { modified: true }), drawingNow), null, "⌥Z");
  assert.equal(keyDownCommand(press("KeyZ"), drawingNow), null, "Z alone");
  assert.equal(keyDownCommand({ ...undo, editable: true }, drawingNow), null, "typing");
  // Space still talks while you draw: your labels.
  assert.deepEqual(keyDownCommand(press("Space"), drawingNow), {
    type: "Mic",
    action: "TalkDown",
  });
});

// Review 2: ⌘⇧Z is redo on macOS. KeyPress carried no Shift, so it read as ⌘Z and undid
// another shape.
test("⌘⇧Z (redo on macOS) undoes nothing while you draw", () => {
  const redo = press("KeyZ", { modified: true, shortcut: true, shift: true });
  assert.equal(keyDownCommand(redo, drawingNow), null);
});

// Review 2: Done is disabled while muted (the core drops a muted Done, D-163), but Enter sent it.
test("Enter while muted and drawing sends no Done, as the disabled button doesn't", () => {
  assert.equal(keyDownCommand(press("Enter"), { ...drawingNow, muted: true }), null);
});

// Review 2: the settings page listened on its own <main>; once the focused control went away,
// keys reached the stage behind it (Esc stopped BoardAI, Space talked, M muted, D drew).
test("no key reaches the stage while settings are open", () => {
  const settings = { ...live, settings: true } as const;
  const keys: KeyPress[] = [
    press("Escape"),
    press("Space"),
    press("KeyM"),
    press("KeyD"),
    press("Enter"),
    press("KeyZ", { modified: true, shortcut: true }),
  ];
  for (const key of keys) {
    assert.equal(keyDownCommand(key, settings), null, key.code);
    assert.equal(keyDownCommand(key, { ...settings, drawing: true }), null, `${key.code} drawing`);
    assert.equal(
      keyDownCommand(key, { ...settings, escape: "close-view" }),
      null,
      `${key.code} view`,
    );
  }
  const talk = new PushToTalk();
  assert.equal(talk.down(press("Space"), settings), null);
  assert.equal(talk.held, false, "no hold starts behind settings");
});
