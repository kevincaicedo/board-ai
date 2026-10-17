// The keyboard (PRODUCT §8.9): hold Space to talk, M to mute, Esc to stop BoardAI. A pure mapping
// from a key event to the command it sends, so the rules are tested without a DOM. Esc closes
// what is open first: a replay started from a card (the UI's own), then the core's view; with
// nothing open it stops BoardAI. Drawing together (13 · You draw): D starts drawing, and while
// you draw Enter is Done, Esc leaves drawing and ⌘Z (Ctrl+Z) undoes your last shape.

import type { DrawAction } from "../protocol/generated/DrawAction.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { EscapeAction } from "./replays.ts";

export interface KeyPress {
  readonly code: string;
  readonly repeat: boolean;
  readonly modified: boolean;
  /** Typing into a field never controls BoardAI. */
  readonly editable: boolean;
  /** ⌘ (or Ctrl) without Alt: the platform's shortcut modifier, for ⌘Z. */
  readonly shortcut?: boolean;
}

export interface KeyContext {
  readonly muted: boolean;
  /** In a replay Space plays and pauses instead (ReplayControls owns it). */
  readonly replay: boolean;
  /** What Esc closes, from `escapeAction`: "close-replay" is the UI's own (no command). */
  readonly escape: EscapeAction;
  /** You are drawing (13 · You draw). */
  readonly drawing?: boolean;
}

function draw(action: DrawAction): UiCommand {
  return { type: "Draw", action };
}

/** The command a key press sends, or `null` when the key is not ours. */
export function keyDownCommand(key: KeyPress, context: KeyContext): UiCommand | null {
  if (key.editable || key.repeat) {
    return null;
  }
  const drawing = context.drawing === true;
  if (key.modified) {
    return drawing && key.shortcut === true && key.code === "KeyZ" ? draw({ kind: "Undo" }) : null;
  }
  switch (key.code) {
    case "Space":
      return context.replay ? null : { type: "Mic", action: "TalkDown" };
    case "KeyM":
      return { type: "Mic", action: context.muted ? "Unmute" : "Mute" };
    case "Escape":
      return escapeCommand(context.escape, drawing);
    case "KeyD":
      // Muted, you draw silently: the core starts drawing without its spoken line.
      return drawing || context.replay ? null : draw({ kind: "Start" });
    case "Enter":
      return drawing ? draw({ kind: "Done" }) : null;
    default:
      return null;
  }
}

/**
 * Esc: nothing to send for a card's replay (the UI closes it), else close the view; with nothing
 * open it leaves drawing, or stops BoardAI.
 */
function escapeCommand(closing: EscapeAction, drawing: boolean): UiCommand | null {
  switch (closing) {
    case "close-replay":
      return null;
    case "close-view":
      return { type: "CloseView" };
    case "none":
      return drawing ? draw({ kind: "Cancel" }) : { type: "Stop" };
  }
}

/** ⌘, opens the settings screen over the stage (and closes it again from inside). */
export function opensSettings(key: {
  readonly code: string;
  readonly meta: boolean;
  readonly repeat: boolean;
}): boolean {
  return key.code === "Comma" && key.meta && !key.repeat;
}

/**
 * Hold Space to talk: one TalkDown, then exactly one TalkUp however the hold ends — Space's
 * key-up, the window losing focus (⌘-Tab while holding sends the key-up to another app), the
 * page hiding, or the stage going away. A lost TalkUp leaves the core listening until its 30 s
 * utterance cap on every turn. It outlives the key handlers, so re-binding them (a mute) never
 * forgets a hold.
 */
export class PushToTalk {
  #held = false;

  get held(): boolean {
    return this.#held;
  }

  down(key: KeyPress, context: KeyContext): UiCommand | null {
    const command = keyDownCommand(key, context);
    if (command?.type === "Mic" && command.action === "TalkDown") {
      if (this.#held) {
        return null;
      }
      this.#held = true;
    }
    return command;
  }

  up(code: string): UiCommand | null {
    return code === "Space" ? this.release() : null;
  }

  /** The hold ends without its key-up (blur, hidden page, unmount). */
  release(): UiCommand | null {
    if (!this.#held) {
      return null;
    }
    this.#held = false;
    return { type: "Mic", action: "TalkUp" };
  }
}
