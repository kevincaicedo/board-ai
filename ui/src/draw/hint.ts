// Where the label hint sits and what it says (13 · You draw): under the shape your words will
// label, its left edge a little left of the shape's, as the artboard places “…and this one is
// the cache” → will label this shape.

import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import { type WorldTransform, worldRectToScreen } from "./input.ts";

export type HintTokens = Pick<Tokens, "drawHintLeft" | "drawHintBelow">;

/** The pill's top-left corner in board (screen) pixels, under the shape at `bounds` (world). */
export function hintPlacement(
  bounds: Rect,
  transform: WorldTransform,
  tokens: HintTokens,
): { left: number; top: number } {
  const screen = worldRectToScreen(transform, bounds);
  return {
    left: Math.round(screen.x - tokens.drawHintLeft),
    top: Math.round(screen.y + screen.h + tokens.drawHintBelow),
  };
}

/**
 * The words as the pill quotes them: still being said, so they trail in from an ellipsis with no
 * closing mark, and a sentence's capital goes ("And this one…" reads “…and this one…”), an
 * acronym's stays.
 */
export function hintWords(words: string): string {
  const trimmed = words
    .trim()
    .replace(/^[.…\s]+/, "")
    .replace(/[.,;:!?…]+$/, "");
  const [first = "", second = ""] = Array.from(trimmed);
  const lowered =
    first !== first.toLowerCase() && second === second.toLowerCase() && second !== ""
      ? first.toLowerCase() + trimmed.slice(first.length)
      : trimmed;
  return `“…${lowered}”`;
}
