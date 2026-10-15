// The UI's trust boundary for the wire (LLD §3.7). Replay files come from disk and Tauri batches
// can come from a different build, so every event is checked once here, into the generated types.
// Unknown or malformed events become `null`; callers count and skip them and never crash.
// The parser tables are typed by the generated unions, so a new protocol variant fails typecheck
// until it is handled here.

import type { CellMark } from "./generated/CellMark.ts";
import type { ChipKind } from "./generated/ChipKind.ts";
import type { DrawOp } from "./generated/DrawOp.ts";
import type { EdgePath } from "./generated/EdgePath.ts";
import type { LineStyle } from "./generated/LineStyle.ts";
import type { Mark } from "./generated/Mark.ts";
import type { NodeShape } from "./generated/NodeShape.ts";
import type { OrbState } from "./generated/OrbState.ts";
import type { Point } from "./generated/Point.ts";
import type { Rect } from "./generated/Rect.ts";
import type { Speaker } from "./generated/Speaker.ts";
import type { Surface } from "./generated/Surface.ts";
import type { SurfaceUpdate } from "./generated/SurfaceUpdate.ts";
import type { UiBatch } from "./generated/UiBatch.ts";
import type { UiEvent } from "./generated/UiEvent.ts";
import type { UiLogLine } from "./generated/UiLogLine.ts";

type Fields = { readonly [key: string]: unknown };

const U8_MAX = 255;
const U32_MAX = 4_294_967_295;
const I32_MIN = -2_147_483_648;
const I32_MAX = 2_147_483_647;
/** `SessionTime` is a u64 on the wire; JavaScript numbers are exact up to 2^53. */
const SESSION_TIME_MAX = Number.MAX_SAFE_INTEGER;

function fields(value: unknown): Fields | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Fields)
    : null;
}

function integer(value: unknown, low: number, high: number): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= low && value <= high
    ? value
    : null;
}

const u8 = (value: unknown): number | null => integer(value, 0, U8_MAX);
const u32 = (value: unknown): number | null => integer(value, 0, U32_MAX);
const i32 = (value: unknown): number | null => integer(value, I32_MIN, I32_MAX);

function text(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function flag(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[]): T | null {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : null;
}

/** `null` stays `null`; anything else must parse. `undefined` marks a parse failure. */
function nullable<T>(value: unknown, parse: (value: unknown) => T | null): T | null | undefined {
  if (value === null) {
    return null;
  }
  const parsed = parse(value);
  return parsed === null ? undefined : parsed;
}

function list<T>(value: unknown, parse: (value: unknown) => T | null): T[] | null {
  if (!Array.isArray(value)) {
    return null;
  }
  const out: T[] = [];
  for (const entry of value) {
    const parsed = parse(entry);
    if (parsed === null) {
      return null;
    }
    out.push(parsed);
  }
  return out;
}

/** Element ids follow `ElementId::parse`: 1 to 64 bytes of letters, digits and `_ - . : >`. */
const ELEMENT_ID = /^[A-Za-z0-9_\-.:>]{1,64}$/;

function elementId(value: unknown): string | null {
  return typeof value === "string" && ELEMENT_ID.test(value) ? value : null;
}

function point(value: unknown): Point | null {
  const f = fields(value);
  const x = i32(f?.x);
  const y = i32(f?.y);
  return x === null || y === null ? null : { x, y };
}

function rect(value: unknown): Rect | null {
  const f = fields(value);
  const x = i32(f?.x);
  const y = i32(f?.y);
  const w = u32(f?.w);
  const h = u32(f?.h);
  return x === null || y === null || w === null || h === null ? null : { x, y, w, h };
}

function edgePath(value: unknown): EdgePath | null {
  const f = fields(value);
  const start = point(f?.start);
  const end = point(f?.end);
  const control = nullable(f?.control, point);
  return start === null || end === null || control === undefined ? null : { start, control, end };
}

const ORB_STATES: readonly OrbState[] = [
  "Idle",
  "Listening",
  "Thinking",
  "Speaking",
  "Drawing",
  "Muted",
];
const SPEAKERS: readonly Speaker[] = ["User", "Assistant"];
const NODE_SHAPES: readonly NodeShape[] = ["Service", "Db", "Queue", "Actor", "Group", "Note"];
const LINE_STYLES: readonly LineStyle[] = ["Solid", "Dashed"];
const MARKS: readonly Mark[] = ["Circle", "Underline", "Arrow", "Bracket"];
const CHIP_KINDS: readonly ChipKind[] = ["Note", "Task", "Memory", "Source"];

type Tagged<U, Tag extends string> = {
  [K in U extends Record<Tag, infer N extends string> ? N : never]: (
    f: Fields,
  ) => Extract<U, Record<Tag, K>> | null;
};

const DRAW_OPS: Tagged<DrawOp, "kind"> = {
  Node: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const shape = oneOf(f.shape, NODE_SHAPES);
    const label = text(f.label);
    const r = rect(f.rect);
    const stack = u8(f.stack);
    return id === null ||
      op === null ||
      shape === null ||
      label === null ||
      r === null ||
      stack === null
      ? null
      : { kind: "Node", id, op, shape, label, rect: r, stack };
  },
  Edge: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const from = elementId(f.from);
    const to = elementId(f.to);
    const path = edgePath(f.path);
    const label = nullable(f.label, text);
    const style = oneOf(f.style, LINE_STYLES);
    return id === null ||
      op === null ||
      from === null ||
      to === null ||
      path === null ||
      label === undefined ||
      style === null
      ? null
      : { kind: "Edge", id, op, from, to, path, label, style };
  },
  Annotate: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const target = elementId(f.target);
    const mark = oneOf(f.mark, MARKS);
    const r = rect(f.rect);
    const label = nullable(f.text, text);
    return id === null ||
      op === null ||
      target === null ||
      mark === null ||
      r === null ||
      label === undefined
      ? null
      : { kind: "Annotate", id, op, target, mark, rect: r, text: label };
  },
  Strike: (f) => {
    const op = u32(f.op);
    const target = elementId(f.target);
    const label = text(f.label);
    const r = rect(f.rect);
    return op === null || target === null || label === null || r === null
      ? null
      : { kind: "Strike", op, target, label, rect: r };
  },
  Trace: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const points = list(f.points, point);
    return id === null || op === null || points === null ? null : { kind: "Trace", id, op, points };
  },
};

function tagged<U>(
  table: Record<string, (f: Fields) => U | null>,
  tag: string,
  value: unknown,
): U | null {
  const f = fields(value);
  const name = f?.[tag];
  if (f === null || typeof name !== "string" || !Object.hasOwn(table, name)) {
    return null;
  }
  const parse = table[name];
  return parse === undefined ? null : parse(f);
}

export function parseDrawOp(value: unknown): DrawOp | null {
  return tagged<DrawOp>(DRAW_OPS, "kind", value);
}

const CELL_MARKS: Tagged<CellMark, "kind"> = {
  Yes: () => ({ kind: "Yes" }),
  No: () => ({ kind: "No" }),
  Depends: () => ({ kind: "Depends" }),
  Text: (f) => {
    const value = text(f.text);
    return value === null ? null : { kind: "Text", text: value };
  },
};

const SURFACES: Tagged<Surface, "kind"> = {
  Code: (f) => {
    const id = u32(f.id);
    const path = text(f.path);
    const firstLine = u32(f.first_line);
    const lines = list(f.lines, text);
    const highlight = nullable(f.highlight, u32);
    return id === null ||
      path === null ||
      firstLine === null ||
      lines === null ||
      highlight === undefined
      ? null
      : { kind: "Code", id, path, first_line: firstLine, lines, highlight };
  },
  Compare: (f) => {
    const id = u32(f.id);
    const options = list(f.options, text);
    const criteria = list(f.criteria, text);
    return id === null || options === null || criteria === null
      ? null
      : { kind: "Compare", id, options, criteria };
  },
  Recap: (f) => {
    const id = u32(f.id);
    const bullets = list(f.bullets, text);
    return id === null || bullets === null ? null : { kind: "Recap", id, bullets };
  },
};

const SURFACE_UPDATES: Tagged<SurfaceUpdate, "kind"> = {
  Highlight: (f) => {
    const line = u32(f.line);
    return line === null ? null : { kind: "Highlight", line };
  },
  Cell: (f) => {
    const row = u8(f.row);
    const col = u8(f.col);
    const mark = tagged<CellMark>(CELL_MARKS, "kind", f.mark);
    return row === null || col === null || mark === null ? null : { kind: "Cell", row, col, mark };
  },
};

const EVENTS: Tagged<UiEvent, "type"> = {
  Orb: (f) => {
    const state = oneOf(f.state, ORB_STATES);
    const level = integer(f.level_percent, 0, 100);
    return state === null || level === null ? null : { type: "Orb", state, level_percent: level };
  },
  Acknowledge: () => ({ type: "Acknowledge" }),
  Caption: (f) => {
    const who = oneOf(f.who, SPEAKERS);
    const value = text(f.text);
    const isFinal = flag(f.is_final);
    return who === null || value === null || isFinal === null
      ? null
      : { type: "Caption", who, text: value, is_final: isFinal };
  },
  Thread: (f) => {
    const steps = list(f.steps, text);
    const current = nullable(f.current, u8);
    const detours = list(f.detours, text);
    return steps === null || current === undefined || detours === null
      ? null
      : { type: "Thread", steps, current, detours };
  },
  OpStart: (f) => {
    const layer = u8(f.layer);
    const op = parseDrawOp(f.op);
    const budget = u32(f.budget_ms);
    return layer === null || op === null || budget === null
      ? null
      : { type: "OpStart", layer, op, budget_ms: budget };
  },
  Freeze: (f) => {
    const layer = u8(f.layer);
    return layer === null ? null : { type: "Freeze", layer };
  },
  Resume: (f) => {
    const layer = u8(f.layer);
    const budget = u32(f.budget_ms);
    return layer === null || budget === null ? null : { type: "Resume", layer, budget_ms: budget };
  },
  LayerPush: (f) => {
    const layer = u8(f.layer);
    const title = text(f.title);
    return layer === null || title === null ? null : { type: "LayerPush", layer, title };
  },
  LayerPop: (f) => {
    const layer = u8(f.layer);
    return layer === null ? null : { type: "LayerPop", layer };
  },
  Camera: (f) => {
    const layer = u8(f.layer);
    const frame = rect(f.frame);
    const focus = nullable(f.focus, elementId);
    return layer === null || frame === null || focus === undefined
      ? null
      : { type: "Camera", layer, frame, focus };
  },
  SurfaceShow: (f) => {
    const surface = tagged<Surface>(SURFACES, "kind", f.surface);
    return surface === null ? null : { type: "SurfaceShow", surface };
  },
  SurfaceUpdate: (f) => {
    const id = u32(f.id);
    const update = tagged<SurfaceUpdate>(SURFACE_UPDATES, "kind", f.update);
    return id === null || update === null ? null : { type: "SurfaceUpdate", id, update };
  },
  SurfaceDismiss: (f) => {
    const id = u32(f.id);
    return id === null ? null : { type: "SurfaceDismiss", id };
  },
  Chip: (f) => {
    const kind = oneOf(f.kind, CHIP_KINDS);
    const value = text(f.text);
    return kind === null || value === null ? null : { type: "Chip", kind, text: value };
  },
  Problem: (f) => {
    const spoken = text(f.spoken);
    const detail = text(f.detail);
    return spoken === null || detail === null ? null : { type: "Problem", spoken, detail };
  },
};

export function parseUiEvent(value: unknown): UiEvent | null {
  return tagged<UiEvent>(EVENTS, "type", value);
}

function sessionTime(value: unknown): number | null {
  return integer(value, 0, SESSION_TIME_MAX);
}

/** A batch from the live transport. Malformed events are dropped and counted, never fatal. */
export function parseUiBatch(value: unknown): { batch: UiBatch; skipped: number } | null {
  const f = fields(value);
  const at = sessionTime(f?.at);
  if (f === null || at === null || !Array.isArray(f.events)) {
    return null;
  }
  const events: UiEvent[] = [];
  let skipped = 0;
  for (const entry of f.events) {
    const event = parseUiEvent(entry);
    if (event === null) {
      skipped += 1;
    } else {
      events.push(event);
    }
  }
  return { batch: { at, events }, skipped };
}

export type LogLineResult =
  | { readonly kind: "ui"; readonly line: UiLogLine }
  /** A well-formed line of another kind (`"k":"input"`, for re-simulation): not for the UI. */
  | { readonly kind: "other" }
  | { readonly kind: "malformed"; readonly reason: string };

/** The event-log line versions this UI understands. */
export const LOG_VERSION = 1;

export function parseLogLine(line: string): LogLineResult {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return { kind: "malformed", reason: "not JSON" };
  }
  const f = fields(value);
  if (f === null) {
    return { kind: "malformed", reason: "not an object" };
  }
  if (f.v !== LOG_VERSION) {
    return { kind: "malformed", reason: "unsupported version" };
  }
  const t = sessionTime(f.t);
  if (t === null) {
    return { kind: "malformed", reason: "bad time" };
  }
  if (typeof f.k === "string" && f.k !== "ui") {
    return { kind: "other" };
  }
  if (f.k !== "ui") {
    return { kind: "malformed", reason: "missing kind" };
  }
  const event = parseUiEvent(f.event);
  if (event === null) {
    return { kind: "malformed", reason: "unknown or malformed event" };
  }
  return { kind: "ui", line: { v: LOG_VERSION, t, k: "ui", event } };
}
