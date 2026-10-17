// The keyboard (PRODUCT §8.9): hold Space to talk, M to mute, Esc to stop BoardAI. A pure mapping
// from a key event to the command it sends, so the rules are tested without a DOM. Esc closes
// what is open first: a replay started from a card (the UI's own), then the core's view; with
// nothing open it stops BoardAI. Drawing together (13 · You draw): D starts drawing, and while
// you draw Enter is Done (not while muted, like the button), Esc leaves drawing and ⌘Z (Ctrl+Z)
// undoes your last shape. While the settings page is open no key reaches the stage.

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
  /** Shift is held: ⌘⇧Z is redo on macOS, never undo. */
  readonly shift?: boolean;
}

export interface KeyContext {
  readonly muted: boolean;
  /** In a replay Space plays and pauses instead (ReplayControls owns it). */
  readonly replay: boolean;
  /** What Esc closes, from `escapeAction`: "close-replay" is the UI's own (no command). */
  readonly escape: EscapeAction;
  /** You are drawing (13 · You draw). */
  readonly drawing?: boolean;
  /**
   * The settings page is open over the stage: it owns the keyboard. Checked here rather than by
   * the page stopping propagation, which a key pressed after its focused control went away
   * (focus back on the body) never passed through.
   */
  readonly settings?: boolean;
}

function draw(action: DrawAction): UiCommand {
  return { type: "Draw", action };
}

/** The command a key press sends, or `null` when the key is not ours. */
export function keyDownCommand(key: KeyPress, context: KeyContext): UiCommand | null {
  if (key.editable || key.repeat || context.settings === true) {
    return null;
  }
  const drawing = context.drawing === true;
  if (key.modified) {
    const undo = key.shortcut === true && key.shift !== true && key.code === "KeyZ";
    return drawing && undo ? draw({ kind: "Undo" }) : null;
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
      // Muted, Done is disabled (the core drops a muted Done, D-163); Enter follows the button.
      return drawing && !context.muted ? draw({ kind: "Done" }) : null;
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
