// The keyboard (PRODUCT §8.9): hold Space to talk, M to mute, Esc to stop BoardAI. A pure mapping
// from a key event to the command it sends, so the rules are tested without a DOM.

import type { UiCommand } from "../protocol/generated/UiCommand.ts";

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
      return { type: "Stop" };
    default:
      return null;
  }
}
