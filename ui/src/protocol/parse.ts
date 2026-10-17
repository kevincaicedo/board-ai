// The UI's trust boundary for the wire (LLD §3.7). Replay files come from disk and Tauri batches
// can come from a different build, so every event is checked once here, into the generated types.
// Unknown or malformed events become `null`; callers count and skip them and never crash. A view
// lists what the vault holds, hand-written files included, so its rows parse one by one: a bad
// row is skipped and counted (`RowSkips`) and the rest of the view is kept.
// The parser tables are typed by the generated unions, so a new protocol variant fails typecheck
// until it is handled here.

import type { CellBox } from "./generated/CellBox.ts";
import type { CellMark } from "./generated/CellMark.ts";
import type { ChipKind } from "./generated/ChipKind.ts";
import type { ColumnRange } from "./generated/ColumnRange.ts";
import type { CoverShape } from "./generated/CoverShape.ts";
import type { DigestCard } from "./generated/DigestCard.ts";
import type { DrawOp } from "./generated/DrawOp.ts";
import type { EdgePath } from "./generated/EdgePath.ts";
import type { Fill } from "./generated/Fill.ts";
import type { Lifeline } from "./generated/Lifeline.ts";
import type { LineStyle } from "./generated/LineStyle.ts";
import type { Mark } from "./generated/Mark.ts";
import type { MemoryRow } from "./generated/MemoryRow.ts";
import type { NodeShape } from "./generated/NodeShape.ts";
import type { NoteRow } from "./generated/NoteRow.ts";
import type { OrbState } from "./generated/OrbState.ts";
import type { PastLayer } from "./generated/PastLayer.ts";
import type { Point } from "./generated/Point.ts";
import type { Rect } from "./generated/Rect.ts";
import type { SessionCard } from "./generated/SessionCard.ts";
import type { SessionRef } from "./generated/SessionRef.ts";
import type { SourceRef } from "./generated/SourceRef.ts";
import type { Speaker } from "./generated/Speaker.ts";
import type { Surface } from "./generated/Surface.ts";
import type { SurfaceUpdate } from "./generated/SurfaceUpdate.ts";
import type { TaskRow } from "./generated/TaskRow.ts";
import type { UiBatch } from "./generated/UiBatch.ts";
import type { UiEvent } from "./generated/UiEvent.ts";
import type { UiLogLine } from "./generated/UiLogLine.ts";
import type { View } from "./generated/View.ts";

type Fields = { readonly [key: string]: unknown };

/** Rows of a view (sessions, tasks, notes, memories, past ops) skipped because they didn't parse. */
export interface RowSkips {
  rows: number;
}

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

/**
 * A field protocol v2 added: a missing key reads as `null`, so event logs written before it
 * still replay; a present value must parse like a `nullable` one. `undefined` marks a failure.
 */
function optional<T>(value: unknown, parse: (value: unknown) => T | null): T | null | undefined {
  return value === undefined ? null : nullable(value, parse);
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

/** A view's rows: each one parses on its own, and one that doesn't is skipped and counted. */
function rows<T>(
  value: unknown,
  parse: (value: unknown, skips: RowSkips) => T | null,
  skips: RowSkips,
): T[] | null {
  if (!Array.isArray(value)) {
    return null;
  }
  const out: T[] = [];
  for (const entry of value) {
    const parsed = parse(entry, skips);
    if (parsed === null) {
      skips.rows += 1;
    } else {
      out.push(parsed);
    }
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

/** `[begin, end)` in characters; an empty or reversed range is malformed. */
function columnRange(value: unknown): ColumnRange | null {
  const f = fields(value);
  const begin = u32(f?.begin);
  const end = u32(f?.end);
  return begin === null || end === null || begin >= end ? null : { begin, end };
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
const NODE_SHAPES: readonly NodeShape[] = [
  "Service",
  "Db",
  "Queue",
  "Actor",
  "Group",
  "Note",
  "Circle",
];
const LINE_STYLES: readonly LineStyle[] = ["Solid", "Dashed"];
const MARKS: readonly Mark[] = ["Circle", "Underline", "Arrow", "Bracket", "Highlight"];
const CHIP_KINDS: readonly ChipKind[] = ["Note", "Task", "Memory", "Source"];
const FILLS: readonly Fill[] = ["Lilac", "Sky", "Mint", "Blush"];

const fill = (value: unknown): Fill | null => oneOf(value, FILLS);

function lifeline(value: unknown): Lifeline | null {
  const f = fields(value);
  const participant = elementId(f?.participant);
  const x = i32(f?.x);
  const top = i32(f?.top);
  const bottom = i32(f?.bottom);
  return participant === null || x === null || top === null || bottom === null
    ? null
    : { participant, x, top, bottom };
}

function cellBox(value: unknown): CellBox | null {
  const f = fields(value);
  const id = elementId(f?.id);
  const r = rect(f?.rect);
  const content = text(f?.value);
  const index = nullable(f?.index, text);
  return id === null || r === null || content === null || index === undefined
    ? null
    : { id, rect: r, value: content, index };
}

function sourceRef(value: unknown): SourceRef | null {
  const f = fields(value);
  const path = filled(f?.path);
  const line = integer(f?.line, 1, U32_MAX);
  return path === null || line === null ? null : { path, line };
}

type Tagged<U, Tag extends string> = {
  [K in U extends Record<Tag, infer N extends string> ? N : never]: (
    f: Fields,
    skips: RowSkips,
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
    const tone = optional(f.fill, fill);
    const link = optional(f.link, edgePath);
    return id === null ||
      op === null ||
      shape === null ||
      label === null ||
      r === null ||
      stack === null ||
      tone === undefined ||
      link === undefined
      ? null
      : { kind: "Node", id, op, shape, label, rect: r, stack, fill: tone, link };
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
    const tone = optional(f.fill, fill);
    return id === null ||
      op === null ||
      target === null ||
      mark === null ||
      r === null ||
      label === undefined ||
      tone === undefined
      ? null
      : { kind: "Annotate", id, op, target, mark, rect: r, text: label, fill: tone };
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
  Participant: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const label = text(f.label);
    const r = rect(f.rect);
    const bottom = i32(f.lifeline_bottom);
    return id === null || op === null || label === null || r === null || bottom === null
      ? null
      : { kind: "Participant", id, op, label, rect: r, lifeline_bottom: bottom };
  },
  Message: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const from = elementId(f.from);
    const to = elementId(f.to);
    const points = list(f.points, point);
    const label = nullable(f.label, text);
    const style = oneOf(f.style, LINE_STYLES);
    const extend = list(f.extend, lifeline);
    return id === null ||
      op === null ||
      from === null ||
      to === null ||
      points === null ||
      points.length < 2 ||
      label === undefined ||
      style === null ||
      extend === null
      ? null
      : { kind: "Message", id, op, from, to, points, label, style, extend };
  },
  Cells: (f) => {
    const id = elementId(f.id);
    const op = u32(f.op);
    const label = text(f.label);
    const cells = list(f.cells, cellBox);
    const vertical = flag(f.vertical);
    return id === null ||
      op === null ||
      label === null ||
      cells === null ||
      cells.length === 0 ||
      vertical === null
      ? null
      : { kind: "Cells", id, op, label, cells, vertical };
  },
  Write: (f) => {
    const op = u32(f.op);
    const target = elementId(f.target);
    const label = text(f.label);
    const r = rect(f.rect);
    return op === null || target === null || label === null || r === null
      ? null
      : { kind: "Write", op, target, label, rect: r };
  },
};

function tagged<U>(
  table: Record<string, (f: Fields, skips: RowSkips) => U | null>,
  tag: string,
  value: unknown,
  skips: RowSkips = { rows: 0 },
): U | null {
  const f = fields(value);
  const name = f?.[tag];
  if (f === null || typeof name !== "string" || !Object.hasOwn(table, name)) {
    return null;
  }
  const parse = table[name];
  return parse === undefined ? null : parse(f, skips);
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
    const title = optional(f.title, text);
    return id === null || options === null || criteria === null || title === undefined
      ? null
      : { kind: "Compare", id, options, criteria, title };
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
  Mark: (f) => {
    const line = u32(f.line);
    const columns = columnRange(f.columns);
    const mark = oneOf(f.mark, MARKS);
    return line === null || columns === null || mark === null
      ? null
      : { kind: "Mark", line, columns, mark };
  },
};

/** Dates on the wire are `YYYY-MM-DD` (UTC), or empty when unknown. */
const DATE = /^(\d{4}-\d{2}-\d{2})?$/;

function date(value: unknown): string | null {
  return typeof value === "string" && DATE.test(value) ? value : null;
}

/** A non-empty string: ids and titles are never blank. */
function filled(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

function coverShape(value: unknown): CoverShape | null {
  const f = fields(value);
  const r = rect(f?.rect);
  const shape = oneOf(f?.shape, NODE_SHAPES);
  return r === null || shape === null ? null : { rect: r, shape };
}

function sessionCard(value: unknown, skips: RowSkips): SessionCard | null {
  const f = fields(value);
  const id = filled(f?.id);
  const title = text(f?.title);
  const day = date(f?.date);
  const duration = u32(f?.duration_ms);
  const cover = rows(f?.cover, coverShape, skips);
  return id === null || title === null || day === null || duration === null || cover === null
    ? null
    : { id, title, date: day, duration_ms: duration, cover };
}

function sessionRef(value: unknown): SessionRef | null {
  const f = fields(value);
  const id = filled(f?.id);
  const title = text(f?.title);
  const day = date(f?.date);
  return id === null || title === null || day === null ? null : { id, title, date: day };
}

function taskRow(value: unknown): TaskRow | null {
  const f = fields(value);
  const id = filled(f?.id);
  const title = text(f?.title);
  const project = text(f?.project);
  const from = nullable(f?.from, sessionRef);
  return id === null || title === null || project === null || from === undefined
    ? null
    : { id, title, project, from };
}

function noteRow(value: unknown, skips: RowSkips): NoteRow | null {
  const f = fields(value);
  const id = filled(f?.id);
  const body = text(f?.text);
  const day = date(f?.date);
  const from = nullable(f?.from, sessionRef);
  const links = rows(f?.links, text, skips);
  return id === null || body === null || day === null || from === undefined || links === null
    ? null
    : { id, text: body, date: day, from, links };
}

function memoryRow(value: unknown): MemoryRow | null {
  const f = fields(value);
  const id = filled(f?.id);
  const body = text(f?.text);
  return id === null || body === null ? null : { id, text: body };
}

function digestCard(value: unknown, skips: RowSkips): DigestCard | null {
  const f = fields(value);
  const session = sessionCard(f?.session, skips);
  const notes = u32(f?.notes);
  const tasks = u32(f?.tasks);
  const questions = u32(f?.open_questions);
  return session === null || notes === null || tasks === null || questions === null
    ? null
    : { session, notes, tasks, open_questions: questions };
}

function pastLayer(value: unknown, skips: RowSkips): PastLayer | null {
  const f = fields(value);
  const title = nullable(f?.title, text);
  const ops = rows(f?.ops, parseDrawOp, skips);
  return title === undefined || ops === null ? null : { title, ops };
}

/** The session digest a Memory view shows; one that doesn't parse is skipped, as a row is. */
function latestDigest(value: unknown, skips: RowSkips): DigestCard | null | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (value === null) {
    return null;
  }
  const digest = digestCard(value, skips);
  if (digest === null) {
    skips.rows += 1;
  }
  return digest;
}

const VIEWS: Tagged<View, "kind"> = {
  Library: (f, skips) => {
    const sessions = rows(f.sessions, sessionCard, skips);
    const more = u32(f.more_sessions);
    return sessions === null || more === null
      ? null
      : { kind: "Library", sessions, more_sessions: more };
  },
  Tasks: (f, skips) => {
    const project = nullable(f.project, text);
    const tasks = rows(f.tasks, taskRow, skips);
    const more = u32(f.more_tasks);
    const notes = rows(f.notes, noteRow, skips);
    return project === undefined || tasks === null || more === null || notes === null
      ? null
      : { kind: "Tasks", project, tasks, more_tasks: more, notes };
  },
  Memory: (f, skips) => {
    const aboutYou = rows(f.about_you, memoryRow, skips);
    const systems = rows(f.systems, memoryRow, skips);
    const notes = rows(f.notes, noteRow, skips);
    const latest = latestDigest(f.latest, skips);
    return aboutYou === null || systems === null || notes === null || latest === undefined
      ? null
      : { kind: "Memory", about_you: aboutYou, systems, notes, latest };
  },
  PastSession: (f, skips) => {
    const session = sessionCard(f.session, skips);
    const recap = text(f.recap);
    const layers = rows(f.layers, pastLayer, skips);
    const leftOut = u32(f.ops_left_out);
    return session === null || recap === null || layers === null || leftOut === null
      ? null
      : { kind: "PastSession", session, recap, layers, ops_left_out: leftOut };
  },
  Replay: (f, skips) => {
    const session = sessionCard(f.session, skips);
    return session === null ? null : { kind: "Replay", session };
  },
};

export function parseView(value: unknown, skips: RowSkips = { rows: 0 }): View | null {
  return tagged<View>(VIEWS, "kind", value, skips);
}

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
    const question = optional(f.question, text);
    return layer === null || title === null || question === undefined
      ? null
      : { type: "LayerPush", layer, title, question };
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
    const source = optional(f.source, sourceRef);
    return kind === null || value === null || source === undefined
      ? null
      : { type: "Chip", kind, text: value, source };
  },
  Problem: (f) => {
    const spoken = text(f.spoken);
    const detail = text(f.detail);
    return spoken === null || detail === null ? null : { type: "Problem", spoken, detail };
  },
  Suggestion: (f) => {
    const value = nullable(f.text, text);
    return value === undefined ? null : { type: "Suggestion", text: value };
  },
  SessionStart: (f) => {
    const session = nullable(f.session, filled);
    return session === undefined ? null : { type: "SessionStart", session };
  },
  ViewShow: (f, skips) => {
    const view = parseView(f.view, skips);
    // An unknown day ("") only costs the "today" labels: days then read in full.
    const today = date(f.today);
    return view === null || today === null ? null : { type: "ViewShow", view, today };
  },
  ViewDismiss: () => ({ type: "ViewDismiss" }),
  OpenSettings: () => ({ type: "OpenSettings" }),
  SessionEnd: () => ({ type: "SessionEnd" }),
};

export function parseUiEvent(value: unknown, skips: RowSkips = { rows: 0 }): UiEvent | null {
  return tagged<UiEvent>(EVENTS, "type", value, skips);
}

function sessionTime(value: unknown): number | null {
  return integer(value, 0, SESSION_TIME_MAX);
}

/**
 * A batch from the live transport. Malformed events are dropped and counted, never fatal, and so
 * are the rows views skipped.
 */
export function parseUiBatch(
  value: unknown,
): { batch: UiBatch; skipped: number; skippedRows: number } | null {
  const f = fields(value);
  const at = sessionTime(f?.at);
  if (f === null || at === null || !Array.isArray(f.events)) {
    return null;
  }
  const events: UiEvent[] = [];
  const skips: RowSkips = { rows: 0 };
  let skipped = 0;
  for (const entry of f.events) {
    const event = parseUiEvent(entry, skips);
    if (event === null) {
      skipped += 1;
    } else {
      events.push(event);
    }
  }
  return { batch: { at, events }, skipped, skippedRows: skips.rows };
}

export type LogLineResult =
  | { readonly kind: "ui"; readonly line: UiLogLine; readonly skippedRows: number }
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
  const skips: RowSkips = { rows: 0 };
  const event = parseUiEvent(f.event, skips);
  if (event === null) {
    return { kind: "malformed", reason: "unknown or malformed event" };
  }
  return { kind: "ui", line: { v: LOG_VERSION, t, k: "ui", event }, skippedRows: skips.rows };
}
