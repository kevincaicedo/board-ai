// Layout math for the stage (PRODUCT §8.7): the surface column in the right half, captions under
// the orb, captured chips and their flight to the brain edge. Pure functions of session time,
// tested with node, so a replay seek lands on the same layout.

import { easeOut, lerp, ramp, spring } from "../canvas/math.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import type { Tokens } from "../styles/tokens.ts";

/**
 * No more than two surfaces sit beside the canvas at once (PRODUCT §7.2): the core's
 * `SURFACES_BESIDE_CANVAS_MAX`, which counts code, compare and recap cards only. The core sends
 * the dismiss itself; past this the UI steps the oldest surface out, so it never shows more.
 * A detour card is not a surface.
 */
export const SURFACES_VISIBLE_MAX = 2;

export type ColumnTokens = Pick<
  Tokens,
  | "surfaceTop"
  | "surfaceCenter"
  | "surfaceGap"
  | "detourCardHeight"
  | "codeHead"
  | "codeLine"
  | "codeFoot"
  | "compareHead"
  | "compareRow"
  | "compareFoot"
  | "recapHead"
  | "recapRow"
  | "recapFoot"
  | "durationSceneMs"
>;

/** A span in which a nested detour's card hides its outer detour's card; `to` is its close. */
export interface Cover {
  readonly from: number;
  readonly to: number | null;
}

export interface Placed {
  readonly key: string;
  /** When the column made room for it (others start moving). */
  readonly shownAt: number;
  /** When it left, or was pushed out by a newer surface. */
  readonly leftAt: number | null;
  /** The latest span an inner detour's card hid this one (detour cards only). */
  readonly cover: Cover | null;
  readonly height: number;
}

export function surfaceHeight(surface: Surface, tokens: ColumnTokens): number {
  switch (surface.kind) {
    case "Code":
      return tokens.codeHead + surface.lines.length * tokens.codeLine + tokens.codeFoot;
    case "Compare":
      return (
        tokens.compareHead + (surface.criteria.length + 1) * tokens.compareRow + tokens.compareFoot
      );
    case "Recap":
      return tokens.recapHead + surface.bullets.length * tokens.recapRow + tokens.recapFoot;
  }
}

function coveredAt(item: Pick<Placed, "cover">, at: number): boolean {
  const cover = item.cover;
  return cover !== null && at >= cover.from && (cover.to === null || at < cover.to);
}

function visibleAt<T extends Placed>(items: readonly T[], at: number): T[] {
  return items.filter(
    (item) =>
      item.shownAt <= at && (item.leftAt === null || at < item.leftAt) && !coveredAt(item, at),
  );
}

/**
 * Tops for a set of items: one is centered on the column's middle, but never above the column's
 * top (a detour card grown for its drawing, D-151); more stack from the top.
 */
function stackTops<T extends Placed>(
  visible: readonly T[],
  tokens: ColumnTokens,
  height: (item: T) => number,
  out: Map<string, number>,
): void {
  const only = visible[0];
  if (visible.length === 1 && only !== undefined) {
    out.set(only.key, Math.max(tokens.surfaceTop, tokens.surfaceCenter - height(only) / 2));
    return;
  }
  let top = tokens.surfaceTop;
  for (const item of visible) {
    out.set(item.key, top);
    top += height(item) + tokens.surfaceGap;
  }
}

/** An item's height as the store placed it; a grown detour card's comes from its drawing. */
const placedHeight = (item: Placed): number => item.height;

/**
 * Where each item's top is at `at`. When the set of visible items changes, items that stay move
 * to their new place on a spring over a scene duration; leaving items stay where they were.
 */
export function columnTops<T extends Placed>(
  items: readonly T[],
  at: number,
  tokens: ColumnTokens,
  height: (item: T) => number = placedHeight,
): Map<string, number> {
  const changes = new Set<number>();
  for (const item of items) {
    for (const change of [item.shownAt, item.leftAt, item.cover?.from, item.cover?.to]) {
      if (change !== null && change !== undefined && change <= at) {
        changes.add(change);
      }
    }
  }
  const ordered = [...changes].sort((a, b) => a - b);
  const latest = ordered[ordered.length - 1];
  const before = ordered[ordered.length - 2];
  const now = new Map<string, number>();
  const previous = new Map<string, number>();
  stackTops(visibleAt(items, at), tokens, height, now);
  if (latest !== undefined && before !== undefined) {
    stackTops(visibleAt(items, latest - 0.5), tokens, height, previous);
  }
  const tops = new Map<string, number>();
  const move = latest === undefined ? 1 : spring(ramp(at, latest, latest + tokens.durationSceneMs));
  for (const item of items) {
    const target = now.get(item.key);
    const from = previous.get(item.key);
    if (target === undefined) {
      // Leaving (or not yet shown): hold the last place it had.
      const held = from ?? holdTop(items, item, tokens, height);
      tops.set(item.key, held);
    } else {
      tops.set(item.key, from === undefined ? target : lerp(from, target, move));
    }
  }
  return tops;
}

function holdTop<T extends Placed>(
  items: readonly T[],
  item: T,
  tokens: ColumnTokens,
  height: (item: T) => number,
): number {
  // The last moment it was in the column: before it left, before a nested detour covered it.
  const hidden =
    item.leftAt ?? (item.cover !== null && item.cover.to === null ? item.cover.from : null);
  const at = hidden === null ? item.shownAt : hidden - 0.5;
  const tops = new Map<string, number>();
  stackTops(visibleAt(items, at), tokens, height, tops);
  return tops.get(item.key) ?? Math.max(tokens.surfaceTop, tokens.surfaceCenter - height(item) / 2);
}

/** When the column stops moving: its last change plus the spring's scene duration. */
function settledAt(items: readonly Placed[], tokens: ColumnTokens): number {
  let last = Number.NEGATIVE_INFINITY;
  for (const item of items) {
    last = Math.max(last, item.shownAt, item.leftAt ?? last, item.cover?.from ?? last);
    last = Math.max(last, item.cover?.to ?? last);
  }
  return last + tokens.durationSceneMs;
}

/**
 * The column's tops frame after frame. Once every change is a scene duration behind, the tops are
 * the same at every later time (`columnTops` depends on time only through the changes and the
 * spring), so the same map is handed back until the items or a card's height change: a still
 * column costs no layout and no allocation per frame. A seek back lays it out again.
 */
export class ColumnTopsCache<T extends Placed = Placed> {
  #items: readonly T[] | null = null;
  readonly #heights: number[] = [];
  #settledAt = Number.POSITIVE_INFINITY;
  #settled: Map<string, number> | null = null;

  tops(
    items: readonly T[],
    at: number,
    tokens: ColumnTokens,
    height: (item: T) => number,
  ): Map<string, number> {
    if (!this.#same(items, height)) {
      this.#items = items;
      this.#heights.length = items.length;
      items.forEach((item, index) => {
        this.#heights[index] = height(item);
      });
      this.#settledAt = settledAt(items, tokens);
      this.#settled = null;
    }
    if (this.#settled !== null && at >= this.#settledAt) {
      return this.#settled;
    }
    const tops = columnTops(items, at, tokens, height);
    this.#settled = at >= this.#settledAt ? tops : null;
    return tops;
  }

  #same(items: readonly T[], height: (item: T) => number): boolean {
    if (items !== this.#items || items.length !== this.#heights.length) {
      return false;
    }
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      if (item === undefined || height(item) !== this.#heights[index]) {
        return false;
      }
    }
    return true;
  }
}

export interface Motion {
  /** 0 → 1 as the surface rises out of its blur. */
  readonly enter: number;
  /** 0 → 1 as it blurs out. */
  readonly exit: number;
}

/**
 * A card's motion at `at`: it rises in at `enterAt` and blurs out when it leaves. A detour card a
 * nested detour covers blurs out the same way, and rises in again when that detour closes.
 */
export function surfaceMotion(
  item: Pick<Placed, "leftAt" | "cover"> & { readonly enterAt: number },
  at: number,
  durationMs: number,
): Motion {
  const cover = item.cover;
  const back = cover !== null && cover.to !== null && at >= cover.to ? cover.to : null;
  const from = back ?? item.enterAt;
  const hide =
    cover !== null && coveredAt(item, at)
      ? easeOut(ramp(at, cover.from, cover.from + durationMs))
      : 0;
  const leave = item.leftAt === null ? 0 : easeOut(ramp(at, item.leftAt, item.leftAt + durationMs));
  return {
    enter: easeOut(ramp(at, from, from + durationMs)),
    exit: Math.max(hide, leave),
  };
}

/**
 * The live caption's text: the tail that fits, with an ellipsis when the head was cut (the
 * design keeps about 54 letters). The last word is split off so it can fade in.
 */
export function captionText(text: string, maxLetters: number): { head: string; last: string } {
  const words = text.split(" ").filter((word) => word !== "");
  const last = words.pop() ?? "";
  let head = words.join(" ");
  let cut = false;
  while (head.length + 1 + last.length > maxLetters && head.includes(" ")) {
    head = head.slice(head.indexOf(" ") + 1);
    cut = true;
  }
  return { head: `${cut ? "…" : ""}${head === "" ? "" : `${head} `}`, last };
}

export interface ChipFlight {
  readonly x: number;
  readonly y: number;
  readonly scale: number;
  readonly opacity: number;
  readonly blur: number;
  readonly visible: boolean;
}

/** A captured chip rising in, then flying on an arc to the brain edge and shrinking into it. */
export function chipFlight(
  origin: readonly [number, number],
  target: readonly [number, number],
  shownAt: number,
  flyAt: number,
  at: number,
  enterMs: number,
  flyMs: number,
  rise: number,
): ChipFlight {
  const enter = easeOut(ramp(at, shownAt, shownAt + enterMs));
  const f = ramp(at, flyAt, flyAt + flyMs);
  const e = f < 0.5 ? 4 * f * f * f : 1 - (-2 * f + 2) ** 3 / 2;
  const cx = (origin[0] + target[0]) / 2 + 120;
  const cy = Math.min(origin[1], target[1]) - 170;
  const a = (1 - e) * (1 - e);
  const b = 2 * (1 - e) * e;
  const c = e * e;
  return {
    x: a * origin[0] + b * cx + c * target[0] - origin[0],
    y: a * origin[1] + b * cy + c * target[1] - origin[1] + rise * (1 - enter),
    scale: lerp(1, 0.28, e),
    opacity: enter * (1 - ramp(f, 0.72, 1)),
    blur: 8 * (1 - enter) + 5 * ramp(f, 0.45, 1),
    visible: at >= shownAt && f < 1,
  };
}

/**
 * How far a source chip has faded, from 0 (shown) to 1 (gone): over `fadeMs` from its deadline.
 * While held it doesn't fade, and one caught fading eases back over `fadeMs` from where it was.
 */
export function sourceChipFade(
  flyAt: number,
  hold: { readonly since: number } | null,
  at: number,
  fadeMs: number,
): number {
  const faded = (time: number): number => easeOut(ramp(time, flyAt, flyAt + fadeMs));
  if (hold === null) {
    return faded(at);
  }
  return faded(hold.since) * (1 - easeOut(ramp(at, hold.since, hold.since + fadeMs)));
}

export type SuggestionTokens = Pick<
  Tokens,
  "durationSuggestionInMs" | "durationSuggestionOutMs" | "suggestionRise" | "suggestionBlur"
>;

export interface SuggestionLook {
  readonly opacity: number;
  /** Pixels below its resting place (it rises in). */
  readonly rise: number;
  readonly blur: number;
}

/**
 * The idle suggestion at `at` (frame 1): it rises in out of a blur, then, once the user speaks,
 * blurs out. The design's `sIn` and `sug`, with the clear taken from the core's event.
 */
export function suggestionLook(
  shownAt: number,
  clearedAt: number | null,
  at: number,
  tokens: SuggestionTokens,
): SuggestionLook {
  const enter = easeOut(ramp(at, shownAt, shownAt + tokens.durationSuggestionInMs));
  const leave =
    clearedAt === null ? 0 : ramp(at, clearedAt, clearedAt + tokens.durationSuggestionOutMs);
  const shown = enter * (1 - leave);
  return {
    opacity: shown,
    rise: (1 - enter) * tokens.suggestionRise,
    blur: (1 - shown) * tokens.suggestionBlur,
  };
}
