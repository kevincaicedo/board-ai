// The keyboard (PRODUCT §8.9): hold Space to talk, M to mute, Esc to stop BoardAI. A pure mapping
// from a key event to the command it sends, so the rules are tested without a DOM. Esc closes
// what is open first: a replay started from a card (the UI's own), then the core's view; with
// nothing open it stops BoardAI.

import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { EscapeAction } from "./replays.ts";

export interface KeyPress {
  readonly code: string;
  readonly repeat: boolean;
  readonly modified: boolean;
  /** Typing into a field never controls BoardAI. */
  readonly editable: boolean;
}

export interface KeyContext {
  readonly muted: boolean;
  /** In a replay Space plays and pauses instead (ReplayControls owns it). */
  readonly replay: boolean;
  /** What Esc closes, from `escapeAction`: "close-replay" is the UI's own (no command). */
  readonly escape: EscapeAction;
}

/** The command a key press sends, or `null` when the key is not ours. */
export function keyDownCommand(key: KeyPress, context: KeyContext): UiCommand | null {
  if (key.editable || key.modified || key.repeat) {
    return null;
  }
  switch (key.code) {
    case "Space":
      return context.replay ? null : { type: "Mic", action: "TalkDown" };
    case "KeyM":
      return { type: "Mic", action: context.muted ? "Unmute" : "Mute" };
    case "Escape":
      return escapeCommand(context.escape);
    default:
      return null;
  }
}

/** Esc: nothing to send for a card's replay (the UI closes it), else close the view or stop. */
function escapeCommand(closing: EscapeAction): UiCommand | null {
  switch (closing) {
    case "close-replay":
      return null;
    case "close-view":
      return { type: "CloseView" };
    case "none":
      return { type: "Stop" };
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
