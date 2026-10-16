// The pen mark on a code line (frame 5: the orange underline under `s.cache.Get(`). The core says
// which characters; the card measures where they sit once, and this turns that box into a
// hand-drawn stroke, seeded so a replay draws the same wobble.

import type { CodeMark } from "../app/session.ts";
import { roughEllipse, roughLine } from "../canvas/geometry.ts";
import { randomFor } from "../canvas/rng.ts";
import { makeStroke, type Stroke } from "../canvas/stroke.ts";
import type { Mark } from "../protocol/generated/Mark.ts";

/** The pen's width on a code card, as on the canvas (stroke-width 2.2). */
const PEN_WIDTH = 2.2;
/**
 * Where the underline sits against the text's box (its bottom is the font's descent, below the
 * baseline): 2px inside it, as on the canvas. A circle stands this far off the text.
 */
const UNDERLINE_DROP = -2;
const CIRCLE_PAD = 6;

/** Where the marked characters sit, in the card's coordinates. */
export interface TextBox {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

export function codeMarkSeed(surface: number, mark: CodeMark): string {
  return `code:${surface}:${mark.line}:${mark.columns.begin}-${mark.columns.end}`;
}

/** Circles go around the text; every other mark reads as an underline on a line of code. */
export function codeMarkStroke(mark: Mark, box: TextBox, seed: string): Stroke {
  const random = randomFor(seed);
  switch (mark) {
    case "Circle":
      return makeStroke(
        roughEllipse(
          random,
          box.x + box.w / 2,
          box.y + box.h / 2,
          box.w / 2 + CIRCLE_PAD,
          box.h / 2 + CIRCLE_PAD * 0.6,
          { start: -2.2, span: Math.PI * 2 + 0.5, grow: 0.06 },
        ),
        "annotate",
        PEN_WIDTH,
        { kind: "ellipse" },
      );
    case "Underline":
    case "Arrow":
    case "Bracket": {
      const y = box.y + box.h + UNDERLINE_DROP;
      return makeStroke(
        roughLine(random, box.x, y, box.x + box.w, y + 1, { overshoot: 3, bow: 1.5 }),
        "annotate",
        PEN_WIDTH,
      );
    }
  }
}

/**
 * The UTF-16 offset of a character column in `text`: columns count Unicode scalar values, as the
 * core does, so an emoji before the mark doesn't shift it.
 */
export function utf16Offset(text: string, column: number): number {
  let offset = 0;
  let seen = 0;
  for (const ch of text) {
    if (seen === column) {
      return offset;
    }
    offset += ch.length;
    seen += 1;
  }
  return offset;
}
