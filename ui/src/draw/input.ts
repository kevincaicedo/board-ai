// Turning the pointer into shapes (13 · You draw): pen points sampled into a fixed buffer, the
// box and arrow tools' drags, screen to world, and the preview drawn while the pointer is down.
// Pure functions and one preallocated sampler, so pointer moves allocate nothing until the stroke
// is sent but the preview's new segment.

import type { DrawnShape } from "../protocol/generated/DrawnShape.ts";
import type { Point } from "../protocol/generated/Point.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import { PEN_SAMPLE_MIN_PX, USER_SHAPE_MIN_UNITS, USER_STROKE_POINTS_MAX } from "./limits.ts";
import type { Tool } from "./model.ts";

/** A world → screen mapping (the main layer's camera): uniform scale, then translation. */
export interface WorldTransform {
  readonly scale: number;
  readonly x: number;
  readonly y: number;
}

/**
 * One pen stroke's screen points, at least `minDistance` apart, in a buffer of `capacity`. When
 * the buffer fills, every other point goes (the first stays) and the spacing doubles, so a long
 * stroke always fits the core's bound and keeps its shape; the last point is always where the
 * pointer is.
 */
export class StrokeSampler {
  readonly #xs: Float64Array;
  readonly #ys: Float64Array;
  readonly #capacity: number;
  readonly #baseDistance: number;
  #minDistance: number;
  #count = 0;
  #generation = 0;

  constructor(capacity = USER_STROKE_POINTS_MAX, minDistance = PEN_SAMPLE_MIN_PX) {
    this.#capacity = Math.max(2, capacity);
    this.#xs = new Float64Array(this.#capacity);
    this.#ys = new Float64Array(this.#capacity);
    this.#baseDistance = minDistance;
    this.#minDistance = minDistance;
  }

  get count(): number {
    return this.#count;
  }

  /**
   * Moves whenever a kept point changes (a new stroke, a decimation, the last point moved by
   * `finish`); between two moves points are only appended, so a preview can append too.
   */
  get generation(): number {
    return this.#generation;
  }

  x(index: number): number {
    return this.#xs[index] ?? 0;
  }

  y(index: number): number {
    return this.#ys[index] ?? 0;
  }

  begin(x: number, y: number): void {
    this.#generation += 1;
    this.#minDistance = this.#baseDistance;
    this.#xs[0] = x;
    this.#ys[0] = y;
    this.#count = 1;
  }

  /** A pointer move; returns whether the point was kept. */
  add(x: number, y: number): boolean {
    if (this.#count === 0) {
      this.begin(x, y);
      return true;
    }
    const last = this.#count - 1;
    if (Math.hypot(x - (this.#xs[last] ?? 0), y - (this.#ys[last] ?? 0)) < this.#minDistance) {
      return false;
    }
    if (this.#count === this.#capacity) {
      this.#decimate();
    }
    this.#xs[this.#count] = x;
    this.#ys[this.#count] = y;
    this.#count += 1;
    return true;
  }

  /** The pointer came up at (x, y): the stroke ends exactly there. */
  finish(x: number, y: number): void {
    if (!this.add(x, y) && this.#count > 1) {
      this.#generation += 1;
      this.#xs[this.#count - 1] = x;
      this.#ys[this.#count - 1] = y;
    }
  }

  #decimate(): void {
    this.#generation += 1;
    let kept = 0;
    for (let index = 0; index < this.#count; index += 2) {
      this.#xs[kept] = this.#xs[index] ?? 0;
      this.#ys[kept] = this.#ys[index] ?? 0;
      kept += 1;
    }
    this.#count = kept;
    this.#minDistance *= 2;
  }
}

/** A screen point (relative to the board) in world units, rounded as the protocol carries them. */
export function screenToWorld(transform: WorldTransform, sx: number, sy: number): Point {
  const scale = transform.scale || 1;
  return {
    x: Math.round((sx - transform.x) / scale),
    y: Math.round((sy - transform.y) / scale),
  };
}

/** A world rectangle on screen. */
export function worldRectToScreen(transform: WorldTransform, rect: Rect): Rect {
  return {
    x: rect.x * transform.scale + transform.x,
    y: rect.y * transform.scale + transform.y,
    w: rect.w * transform.scale,
    h: rect.h * transform.scale,
  };
}

/** The rectangle two corners span. */
export function spanRect(a: Point, b: Point): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  };
}

/** The pen's points in world units, repeats dropped (rounding can land two samples on one). */
export function penPoints(sampler: StrokeSampler, transform: WorldTransform): Point[] {
  const points: Point[] = [];
  for (let index = 0; index < sampler.count; index += 1) {
    const point = screenToWorld(transform, sampler.x(index), sampler.y(index));
    const last = points[points.length - 1];
    if (last === undefined || last.x !== point.x || last.y !== point.y) {
      points.push(point);
    }
  }
  return points;
}

/**
 * The shape a finished gesture makes, in world units; `null` for a click (smaller than the core
 * keeps). The box and arrow use the drag's first and last points; the pen every sample.
 */
export function drawnShape(
  tool: Tool,
  sampler: StrokeSampler,
  transform: WorldTransform,
): DrawnShape | null {
  if (sampler.count === 0) {
    return null;
  }
  const last = sampler.count - 1;
  const start = screenToWorld(transform, sampler.x(0), sampler.y(0));
  const end = screenToWorld(transform, sampler.x(last), sampler.y(last));
  switch (tool) {
    case "box": {
      const rect = spanRect(start, end);
      return rect.w >= USER_SHAPE_MIN_UNITS && rect.h >= USER_SHAPE_MIN_UNITS
        ? { tool: "Box", rect }
        : null;
    }
    case "arrow":
      return reach([start, end]) >= USER_SHAPE_MIN_UNITS ? { tool: "Arrow", start, end } : null;
    case "pen": {
      const points = penPoints(sampler, transform);
      return points.length >= 2 && reach(points) >= USER_SHAPE_MIN_UNITS
        ? { tool: "Pen", points }
        : null;
    }
  }
}

/** How far the points reach along the wider of the two axes. */
function reach(points: readonly Point[]): number {
  let left = Number.POSITIVE_INFINITY;
  let top = Number.POSITIVE_INFINITY;
  let right = Number.NEGATIVE_INFINITY;
  let bottom = Number.NEGATIVE_INFINITY;
  for (const point of points) {
    left = Math.min(left, point.x);
    top = Math.min(top, point.y);
    right = Math.max(right, point.x);
    bottom = Math.max(bottom, point.y);
  }
  return Math.max(right - left, bottom - top);
}

/** How long the open arrowhead's barbs are in the preview, in screen pixels. */
const PREVIEW_HEAD_PX = 13;

/** The preview's SVG path, in screen pixels, while the pointer is down. */
export function previewPath(tool: Tool, sampler: StrokeSampler): string {
  if (sampler.count === 0) {
    return "";
  }
  const last = sampler.count - 1;
  const x0 = round1(sampler.x(0));
  const y0 = round1(sampler.y(0));
  const x1 = round1(sampler.x(last));
  const y1 = round1(sampler.y(last));
  switch (tool) {
    case "box":
      return `M${x0} ${y0}H${x1}V${y1}H${x0}Z`;
    case "arrow": {
      const angle = Math.atan2(y1 - y0, x1 - x0);
      const barb = (turn: number): string =>
        `${round1(x1 + Math.cos(angle + Math.PI + turn) * PREVIEW_HEAD_PX)} ${round1(y1 + Math.sin(angle + Math.PI + turn) * PREVIEW_HEAD_PX)}`;
      return `M${x0} ${y0}L${x1} ${y1}M${barb(-0.46)}L${x1} ${y1}L${barb(0.46)}`;
    }
    case "pen":
      return new PenPath().path(sampler);
  }
}

/**
 * The live pen preview, kept between samples: each sample appends its own segment, and the path
 * is written again only when the sampler's points change in place (a decimation halves them),
 * so a stroke's preview costs work in proportion to its points, not their square.
 */
export class PenPath {
  #path = "";
  #points = 0;
  #generation = -1;

  path(sampler: StrokeSampler): string {
    if (sampler.generation !== this.#generation || sampler.count < this.#points) {
      this.#generation = sampler.generation;
      this.#points = 0;
      this.#path = "";
    }
    for (let index = this.#points; index < sampler.count; index += 1) {
      const command = index === 0 ? "M" : "L";
      this.#path += `${command}${round1(sampler.x(index))} ${round1(sampler.y(index))}`;
    }
    this.#points = sampler.count;
    return this.#path;
  }
}

function round1(value: number): number {
  return Math.round(value * 10) / 10;
}
