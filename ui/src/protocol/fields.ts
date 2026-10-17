// The wire's primitive values, checked once at the UI's trust boundary (LLD §3.7): integers in
// their Rust ranges, element ids as `ElementId::parse` takes them, points, rectangles, optional and
// nullable fields, lists, and tagged unions. parse.ts and its neighbours build every protocol type
// from these.

import type { Point } from "./generated/Point.ts";
import type { Rect } from "./generated/Rect.ts";

export type Fields = { readonly [key: string]: unknown };

/** Rows of a view (sessions, tasks, notes, memories, past ops) skipped because they didn't parse. */
export interface RowSkips {
  rows: number;
}

export const U8_MAX = 255;
export const U16_MAX = 65_535;
export const U32_MAX = 4_294_967_295;
export const I32_MIN = -2_147_483_648;
export const I32_MAX = 2_147_483_647;
/** `SessionTime` is a u64 on the wire; JavaScript numbers are exact up to 2^53. */
export const SESSION_TIME_MAX = Number.MAX_SAFE_INTEGER;

export function fields(value: unknown): Fields | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Fields)
    : null;
}

export function integer(value: unknown, low: number, high: number): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= low && value <= high
    ? value
    : null;
}

export const u8 = (value: unknown): number | null => integer(value, 0, U8_MAX);
export const u32 = (value: unknown): number | null => integer(value, 0, U32_MAX);
export const i32 = (value: unknown): number | null => integer(value, I32_MIN, I32_MAX);

export function text(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

export function flag(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

export function oneOf<T extends string>(value: unknown, allowed: readonly T[]): T | null {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : null;
}

/** `null` stays `null`; anything else must parse. `undefined` marks a parse failure. */
export function nullable<T>(
  value: unknown,
  parse: (value: unknown) => T | null,
): T | null | undefined {
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
export function optional<T>(
  value: unknown,
  parse: (value: unknown) => T | null,
): T | null | undefined {
  return value === undefined ? null : nullable(value, parse);
}

export function list<T>(value: unknown, parse: (value: unknown) => T | null): T[] | null {
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
export function rows<T>(
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
export const ELEMENT_ID = /^[A-Za-z0-9_\-.:>]{1,64}$/;

export function elementId(value: unknown): string | null {
  return typeof value === "string" && ELEMENT_ID.test(value) ? value : null;
}

export function point(value: unknown): Point | null {
  const f = fields(value);
  const x = i32(f?.x);
  const y = i32(f?.y);
  return x === null || y === null ? null : { x, y };
}

export function rect(value: unknown): Rect | null {
  const f = fields(value);
  const x = i32(f?.x);
  const y = i32(f?.y);
  const w = u32(f?.w);
  const h = u32(f?.h);
  return x === null || y === null || w === null || h === null ? null : { x, y, w, h };
}

export type Tagged<U, Tag extends string> = {
  [K in U extends Record<Tag, infer N extends string> ? N : never]: (
    f: Fields,
    skips: RowSkips,
  ) => Extract<U, Record<Tag, K>> | null;
};

export function tagged<U>(
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
