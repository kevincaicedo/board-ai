// Strokes and handwriting with human pacing, ported from the design prototype (mk, revealLen,
// pointAt, textItem). A stroke is revealed along its length; each leg (a side, a hachure line)
// accelerates out of its start and slows into its corner, which is what makes it read as a hand.

import type { Vec } from "./geometry.ts";
import { clamp, easeInOutCubic, lerp, round1 } from "./math.ts";

export type StrokeKind = "line" | "curve" | "ellipse" | "head" | "hatch";

/** Colors are tokens; the renderer maps a tone to a CSS class, never to a hex value. */
export type Tone =
  | "ink"
  | "ink2"
  | "ink3"
  | "grey"
  | "annotate"
  | "signal"
  | "lilac"
  | "sky"
  | "mint"
  | "blush"
  /** The user's own ink (13 · You draw). */
  | "you";

export interface Stroke {
  readonly type: "stroke";
  readonly kind: StrokeKind;
  /** Flat x, y pairs. */
  readonly points: Float64Array;
  /** Cumulative length at each point. */
  readonly cumulative: Float64Array;
  readonly length: number;
  /** The SVG path data, written once when the stroke is created. */
  readonly path: string;
  readonly tone: Tone;
  readonly width: number;
  readonly dashed: boolean;
  /** Point indexes where legs begin and end; pacing restarts at each. */
  readonly legs: Int32Array;
  /** Fraction of the stroke's time at which each leg boundary is reached. */
  readonly legTimes: Float64Array;
  readonly durationMs: number;
}

export interface TextMark {
  readonly type: "text";
  readonly x: number;
  readonly y: number;
  readonly text: string;
  readonly size: number;
  readonly tone: Tone;
  readonly align: TextAlign;
  readonly durationMs: number;
  /** Characters, counted as code points so a letter is never split. */
  readonly count: number;
  /** UTF-16 offset where each letter ends, so a reveal slices without splitting a letter. */
  readonly letterEnds: Int32Array;
  /** The handwriting nib's advance per letter: half the font size, as in the design. */
  readonly letterWidth: number;
  readonly start: Vec;
  readonly end: Vec;
}

export type Item = Stroke | TextMark;

/** Where `x` sits on a line of handwriting: its start, its middle, or its end. */
export type TextAlign = "center" | "left" | "right";

export interface StrokeOptions {
  readonly kind?: StrokeKind;
  readonly legs?: readonly number[];
  readonly dashed?: boolean;
  /** Multiplies the natural duration; the design draws group frames faster (0.55). */
  readonly pace?: number;
}

/** Natural stroke durations from the design: hachure is quick, ellipses slow, lines by length. */
function naturalDurationMs(kind: StrokeKind, length: number): number {
  switch (kind) {
    case "hatch":
      return clamp(length / 5200, 0.24, 0.55) * 1000;
    case "ellipse":
      return (0.08 + length / 640) * 1000;
    case "head":
      return 120;
    case "line":
    case "curve":
      return (0.05 + length / 760) * 1000;
  }
}

/** Relative weight of a leg's share of the stroke's time: a fixed cost per corner plus length. */
function legWeight(kind: StrokeKind, legLength: number): number {
  return kind === "hatch" ? 0.03 + legLength / 900 : 0.06 + legLength / 500;
}

export function makeStroke(
  points: readonly Vec[],
  tone: Tone,
  width: number,
  options: StrokeOptions = {},
): Stroke {
  const kind = options.kind ?? "line";
  const count = points.length;
  const flat = new Float64Array(count * 2);
  const cumulative = new Float64Array(count);
  let path = "";
  for (let index = 0; index < count; index += 1) {
    const point = points[index];
    if (point === undefined) {
      continue;
    }
    flat[index * 2] = point[0];
    flat[index * 2 + 1] = point[1];
    path += `${index === 0 ? "M" : "L"}${round1(point[0])} ${round1(point[1])}`;
    if (index > 0) {
      const previous = cumulative[index - 1] ?? 0;
      cumulative[index] =
        previous +
        Math.hypot(point[0] - (flat[index * 2 - 2] ?? 0), point[1] - (flat[index * 2 - 1] ?? 0));
    }
  }
  const length = cumulative[count - 1] || 1;
  const legIndexes = options.legs ?? [0, count - 1];
  const legs = Int32Array.from(legIndexes);
  const weights: number[] = [];
  let total = 0;
  for (let leg = 0; leg < legs.length - 1; leg += 1) {
    const from = cumulative[legs[leg] ?? 0] ?? 0;
    const to = cumulative[legs[leg + 1] ?? 0] ?? 0;
    const weight = legWeight(kind, to - from);
    weights.push(weight);
    total += weight;
  }
  const legTimes = new Float64Array(legs.length);
  let accumulated = 0;
  for (let leg = 0; leg < weights.length; leg += 1) {
    accumulated += (weights[leg] ?? 0) / (total || 1);
    legTimes[leg + 1] = accumulated;
  }
  return {
    type: "stroke",
    kind,
    points: flat,
    cumulative,
    length,
    path,
    tone,
    width,
    dashed: options.dashed ?? false,
    legs,
    legTimes,
    durationMs: naturalDurationMs(kind, length) * (options.pace ?? 1),
  };
}

/** A hachure fill: every point is a leg, so each line accelerates and settles on its own. */
export function makeHatch(points: readonly Vec[], tone: Tone): Stroke {
  const legs: number[] = [];
  for (let index = 0; index < points.length; index += 1) {
    legs.push(index);
  }
  return makeStroke(points, tone, 1.7, { kind: "hatch", legs });
}

/** Reveal-curve blend per kind: hachure and ellipses are steadier, outlines snap into corners. */
function legEase(kind: StrokeKind, local: number): number {
  if (kind === "hatch") {
    return 0.45 * local + 0.55 * easeInOutCubic(local);
  }
  if (kind === "ellipse") {
    return 0.55 * local + 0.45 * easeInOutCubic(local);
  }
  return 0.22 * local + 0.78 * easeInOutCubic(local);
}

/** How much of the stroke is drawn at time fraction `fraction` of its duration. */
export function revealLength(stroke: Stroke, fraction: number): number {
  if (fraction <= 0) {
    return 0;
  }
  if (fraction >= 1) {
    return stroke.length;
  }
  const times = stroke.legTimes;
  let leg = 0;
  while (leg < times.length - 2 && fraction > (times[leg + 1] ?? 1)) {
    leg += 1;
  }
  const legBegin = times[leg] ?? 0;
  const legEnd = times[leg + 1] ?? 1;
  const local = (fraction - legBegin) / (legEnd - legBegin || 1);
  const from = stroke.cumulative[stroke.legs[leg] ?? 0] ?? 0;
  const to = stroke.cumulative[stroke.legs[leg + 1] ?? 0] ?? stroke.length;
  return from + (to - from) * legEase(stroke.kind, local);
}

/** Reduced motion: whole legs appear one after another instead of flowing (PRODUCT §8.5). */
export function revealLengthStepped(stroke: Stroke, fraction: number): number {
  if (fraction >= 1) {
    return stroke.length;
  }
  const times = stroke.legTimes;
  let leg = 0;
  while (leg < times.length - 2 && fraction > (times[leg + 1] ?? 1)) {
    leg += 1;
  }
  return stroke.cumulative[stroke.legs[leg] ?? 0] ?? 0;
}

/** The point `length` along the stroke, written into `out` so per-frame calls don't allocate. */
export function pointAtLength(stroke: Stroke, length: number, out: Vec): Vec {
  const points = stroke.points;
  const cumulative = stroke.cumulative;
  const last = cumulative.length - 1;
  if (length <= 0) {
    out[0] = points[0] ?? 0;
    out[1] = points[1] ?? 0;
    return out;
  }
  if (length >= stroke.length) {
    out[0] = points[last * 2] ?? 0;
    out[1] = points[last * 2 + 1] ?? 0;
    return out;
  }
  let low = 0;
  let high = last;
  while (high - low > 1) {
    const middle = (low + high) >> 1;
    if ((cumulative[middle] ?? 0) < length) {
      low = middle;
    } else {
      high = middle;
    }
  }
  const from = cumulative[low] ?? 0;
  const to = cumulative[high] ?? 0;
  const f = (length - from) / (to - from || 1);
  out[0] = lerp(points[low * 2] ?? 0, points[high * 2] ?? 0, f);
  out[1] = lerp(points[low * 2 + 1] ?? 0, points[high * 2 + 1] ?? 0, f);
  return out;
}

export function makeText(
  x: number,
  y: number,
  text: string,
  size: number,
  tone: Tone,
  align: TextAlign = "center",
): TextMark {
  const letters = Array.from(text);
  const count = letters.length;
  const letterEnds = new Int32Array(count);
  let offset = 0;
  for (let index = 0; index < count; index += 1) {
    offset += (letters[index] ?? "").length;
    letterEnds[index] = offset;
  }
  const letterWidth = size * 0.5;
  const width = count * letterWidth;
  const startX = align === "left" ? x : align === "right" ? x - width : x - width / 2;
  return {
    type: "text",
    x,
    y,
    text,
    size,
    tone,
    align,
    durationMs: (0.05 + count * 0.046) * 1000,
    count,
    letterEnds,
    letterWidth,
    start: [startX, y],
    end: [startX + width, y],
  };
}

/** Letters written by time fraction `fraction` of the text's duration. */
export function lettersAt(mark: TextMark, fraction: number): number {
  if (fraction >= 1) {
    return mark.count;
  }
  if (fraction <= 0) {
    return 0;
  }
  return Math.floor(fraction * mark.count + 0.0001);
}

/** The UTF-16 length of the first `letters` letters. */
export function textOffset(mark: TextMark, letters: number): number {
  return letters <= 0
    ? 0
    : (mark.letterEnds[Math.min(letters, mark.count) - 1] ?? mark.text.length);
}

export function itemStart(item: Item, out: Vec): Vec {
  if (item.type === "text") {
    out[0] = item.start[0];
    out[1] = item.start[1];
    return out;
  }
  out[0] = item.points[0] ?? 0;
  out[1] = item.points[1] ?? 0;
  return out;
}

export function itemEnd(item: Item, out: Vec): Vec {
  if (item.type === "text") {
    out[0] = item.end[0];
    out[1] = item.end[1];
    return out;
  }
  const last = item.cumulative.length - 1;
  out[0] = item.points[last * 2] ?? 0;
  out[1] = item.points[last * 2 + 1] ?? 0;
  return out;
}

/** The pen lifts between items: a fixed beat plus distance at 2600 px/s (the design's Seq). */
export function travelMs(from: Vec, to: Vec): number {
  return (0.03 + Math.hypot(to[0] - from[0], to[1] - from[1]) / 2600) * 1000;
}
