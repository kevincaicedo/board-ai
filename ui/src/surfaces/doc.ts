// The Doc card (PRODUCT §7.2, artboard 14b): a passage or a figure of a shared document, with the
// checked citation as its chip ("p. 3 · Figure 1") and the sentence being explained highlighted in
// sync with the voice. The card never shows the figure's image (D-260): its caption, and, when no
// model read it, that it needs one. Whether it is redrawn on the board is the core's note, which
// knows whether the digest holds its diagram. Pure functions of the surface and session time,
// tested with node.

import { clamp, easeOut } from "../canvas/math.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import type { Tokens } from "../styles/tokens.ts";

export type DocSurface = Extract<Surface, { kind: "Doc" }>;

/** What the figure box says, under its caption, about a figure no model read. */
export const FIGURE_UNREAD = "Not read: it needs a model that reads files";

export interface DocSentence {
  readonly text: string;
  /** Its place in the passage, from 0: what `SurfaceUpdate::Sentence` names. */
  readonly index: number;
  /** Sentences may repeat word for word and never reorder once shown, so each is keyed by place. */
  readonly key: string;
}

export interface FigureLines {
  readonly label: string;
  readonly caption: string;
  /** Said only for a figure no model read; `null` once one did (the core's note says the rest). */
  readonly status: string | null;
}

/** A move of the card's highlight: the sentence it went to, and when. */
export interface SentenceMove {
  readonly index: number;
  readonly at: number;
}

/** A sentence's highlight strength, 0–1, while it eases in or out. */
export interface SentenceMark {
  readonly index: number;
  readonly strength: number;
}

export type DocTokens = Pick<
  Tokens,
  | "docPadding"
  | "docGap"
  | "docHead"
  | "docLine"
  | "docCharsPerLine"
  | "docFigurePadding"
  | "docFigureGap"
  | "docFigureLabelLine"
  | "docFigureLine"
  | "docFigureCharsPerLine"
  | "docNoteLine"
  | "docNoteCharsPerLine"
>;

export function docSentences(surface: DocSurface): DocSentence[] {
  return surface.sentences.map((text, index) => ({ text, index, key: `sentence-${index}` }));
}

/** The figure box's three lines, or `null` when the citation names no figure. */
export function figureLines(surface: DocSurface): FigureLines | null {
  const figure = surface.figure;
  if (figure === null) {
    return null;
  }
  return {
    label: figure.label,
    caption: figure.caption,
    status: figure.read ? null : FIGURE_UNREAD,
  };
}

/**
 * The highlight as it eases between sentences: the newest move fades in over `durationMs` while
 * the sentence before it fades out, so the highlight seems to travel. A sentence index past the
 * passage marks nothing (the core never sends one; a replay of another build might).
 */
export function sentenceMarks(
  moves: readonly SentenceMove[],
  count: number,
  at: number,
  durationMs: number,
): SentenceMark[] {
  // A seek shows the moves made by then, so a replay lands on the same frame.
  const made = moves.filter((move) => move.at <= at);
  const latest = made[made.length - 1];
  if (latest === undefined) {
    return [];
  }
  const f = durationMs <= 0 ? 1 : easeOut(clamp((at - latest.at) / durationMs));
  const marks: SentenceMark[] = [];
  const previous = made[made.length - 2];
  if (previous !== undefined && previous.index !== latest.index && previous.index < count) {
    marks.push({ index: previous.index, strength: 1 - f });
  }
  if (latest.index < count) {
    const fadeIn = previous === undefined || previous.index !== latest.index ? f : 1;
    marks.push({ index: latest.index, strength: fadeIn });
  }
  return marks.filter((mark) => mark.strength > 0);
}

/**
 * `sentenceMarks` written into `out` (one strength per sentence, 0 for the unmarked): the frame
 * loop's form, which allocates nothing (LLD §4). `out` holds a strength for each of the
 * passage's sentences.
 */
export function writeSentenceStrengths(
  moves: readonly SentenceMove[],
  at: number,
  durationMs: number,
  out: Float64Array,
): void {
  out.fill(0);
  // The latest two moves made by `at`, as `sentenceMarks` reads them.
  let latest: SentenceMove | undefined;
  let previous: SentenceMove | undefined;
  for (let index = 0; index < moves.length; index += 1) {
    const move = moves[index];
    if (move !== undefined && move.at <= at) {
      previous = latest;
      latest = move;
    }
  }
  if (latest === undefined) {
    return;
  }
  const f = durationMs <= 0 ? 1 : easeOut(clamp((at - latest.at) / durationMs));
  if (previous !== undefined && previous.index !== latest.index && previous.index < out.length) {
    out[previous.index] = 1 - f;
  }
  if (latest.index < out.length) {
    out[latest.index] = previous === undefined || previous.index !== latest.index ? f : 1;
  }
}

/** Lines `text` takes at `perLine` characters a line; at least one when it has any text. */
function lineCount(text: string, perLine: number): number {
  const length = Array.from(text).length;
  return length === 0 ? 0 : Math.max(1, Math.ceil(length / Math.max(1, perLine)));
}

/**
 * The card's height in the column, estimated from its text: padding, the head, the figure box,
 * the passage and the note, with a gap between the parts that are there.
 */
export function docCardHeight(surface: DocSurface, tokens: DocTokens): number {
  const parts: number[] = [tokens.docHead];
  const figure = figureLines(surface);
  if (figure !== null) {
    const captionLines = lineCount(figure.caption, tokens.docFigureCharsPerLine);
    const labelLines = figure.status === null ? 1 : 2;
    parts.push(
      2 * tokens.docFigurePadding +
        labelLines * tokens.docFigureLabelLine +
        captionLines * tokens.docFigureLine +
        (captionLines > 0 ? labelLines : labelLines - 1) * tokens.docFigureGap,
    );
  }
  const passage = lineCount(surface.sentences.join(" "), tokens.docCharsPerLine);
  if (passage > 0) {
    parts.push(passage * tokens.docLine);
  }
  const note = lineCount(surface.note, tokens.docNoteCharsPerLine);
  if (note > 0) {
    parts.push(note * tokens.docNoteLine);
  }
  return (
    2 * tokens.docPadding +
    parts.reduce((sum, part) => sum + part, 0) +
    (parts.length - 1) * tokens.docGap
  );
}
