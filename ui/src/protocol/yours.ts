// The protocol of drawing together (13 · You draw): your shapes as the core echoes them and the
// label hint, parsed at the boundary within the core's bounds (points per stroke, strokes per
// shape).

import { USER_SHAPE_STROKES_MAX, USER_STROKE_POINTS_MAX } from "../draw/limits.ts";
import {
  elementId,
  fields,
  list,
  nullable,
  point,
  rect,
  type Tagged,
  tagged,
  text,
} from "./fields.ts";
import type { LabelHint } from "./generated/LabelHint.ts";
import type { Point } from "./generated/Point.ts";
import type { UserShape } from "./generated/UserShape.ts";
import type { UserShapeKind } from "./generated/UserShapeKind.ts";

/** A list of at most `max` entries, each parsed; longer or with a bad entry is malformed. */
function bounded<T>(value: unknown, max: number, parse: (value: unknown) => T | null): T[] | null {
  return Array.isArray(value) && value.length <= max ? list(value, parse) : null;
}

/** One pen stroke: 1 to `USER_STROKE_POINTS_MAX` points. */
function stroke(value: unknown): Point[] | null {
  const points = bounded(value, USER_STROKE_POINTS_MAX, point);
  return points === null || points.length === 0 ? null : points;
}

const USER_SHAPE_KINDS: Tagged<UserShapeKind, "kind"> = {
  Box: (f) => {
    const r = rect(f.rect);
    return r === null ? null : { kind: "Box", rect: r };
  },
  Arrow: (f) => {
    const start = point(f.start);
    const end = point(f.end);
    const from = nullable(f.from, elementId);
    const to = nullable(f.to, elementId);
    return start === null || end === null || from === undefined || to === undefined
      ? null
      : { kind: "Arrow", start, end, from, to };
  },
  Ink: (f) => {
    const strokes = bounded(f.strokes, USER_SHAPE_STROKES_MAX, stroke);
    const bounds = rect(f.bounds);
    return strokes === null || strokes.length === 0 || bounds === null
      ? null
      : { kind: "Ink", strokes, bounds };
  },
};

/** A shape the user drew, as the core echoes it (13 · You draw). */
export function userShape(value: unknown): UserShape | null {
  const f = fields(value);
  const id = elementId(f?.id);
  const kind = tagged<UserShapeKind>(USER_SHAPE_KINDS, "kind", f?.kind);
  const label = nullable(f?.label, text);
  const note = nullable(f?.note, text);
  return id === null || kind === null || label === undefined || note === undefined
    ? null
    : { id, kind, label, note };
}

export function labelHint(value: unknown): LabelHint | null {
  const f = fields(value);
  const target = elementId(f?.target);
  const words = text(f?.words);
  return target === null || words === null ? null : { target, words };
}
