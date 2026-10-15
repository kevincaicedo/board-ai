// Layout math for the stage (PRODUCT §8.7): the surface column in the right half, captions under
// the orb, captured chips and their flight to the brain edge. Pure functions of session time,
// tested with node, so a replay seek lands on the same layout.

import { easeOut, lerp, ramp, spring } from "../canvas/math.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import type { Tokens } from "../styles/tokens.ts";

/** No more than two surfaces sit beside the canvas at once (PRODUCT §7.2). */
export const COLUMN_VISIBLE_MAX = 2;

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

export interface Placed {
  readonly key: string;
  /** When the column made room for it (others start moving). */
  readonly shownAt: number;
  /** When it left, or was pushed out by a newer surface. */
  readonly leftAt: number | null;
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

function visibleAt(items: readonly Placed[], at: number): Placed[] {
  return items.filter((item) => item.shownAt <= at && (item.leftAt === null || at < item.leftAt));
}

/** Tops for a set of items: one is centered on the column's middle; more stack from the top. */
function stackTops(
  visible: readonly Placed[],
  tokens: ColumnTokens,
  out: Map<string, number>,
): void {
  const only = visible[0];
  if (visible.length === 1 && only !== undefined) {
    out.set(only.key, tokens.surfaceCenter - only.height / 2);
    return;
  }
  let top = tokens.surfaceTop;
  for (const item of visible) {
    out.set(item.key, top);
    top += item.height + tokens.surfaceGap;
  }
}

/**
 * Where each item's top is at `at`. When the set of visible items changes, items that stay move
 * to their new place on a spring over a scene duration; leaving items stay where they were.
 */
export function columnTops(
  items: readonly Placed[],
  at: number,
  tokens: ColumnTokens,
): Map<string, number> {
  const changes = new Set<number>();
  for (const item of items) {
    if (item.shownAt <= at) {
      changes.add(item.shownAt);
    }
    if (item.leftAt !== null && item.leftAt <= at) {
      changes.add(item.leftAt);
    }
  }
  const ordered = [...changes].sort((a, b) => a - b);
  const latest = ordered[ordered.length - 1];
  const before = ordered[ordered.length - 2];
  const now = new Map<string, number>();
  const previous = new Map<string, number>();
  stackTops(visibleAt(items, at), tokens, now);
  if (latest !== undefined && before !== undefined) {
    stackTops(visibleAt(items, latest - 0.5), tokens, previous);
  }
  const tops = new Map<string, number>();
  const move = latest === undefined ? 1 : spring(ramp(at, latest, latest + tokens.durationSceneMs));
  for (const item of items) {
    const target = now.get(item.key);
    const from = previous.get(item.key);
    if (target === undefined) {
      // Leaving (or not yet shown): hold the last place it had.
      const held = from ?? holdTop(items, item, tokens);
      tops.set(item.key, held);
    } else {
      tops.set(item.key, from === undefined ? target : lerp(from, target, move));
    }
  }
  return tops;
}

function holdTop(items: readonly Placed[], item: Placed, tokens: ColumnTokens): number {
  const at = item.leftAt === null ? item.shownAt : item.leftAt - 0.5;
  const tops = new Map<string, number>();
  stackTops(visibleAt(items, at), tokens, tops);
  return tops.get(item.key) ?? tokens.surfaceCenter - item.height / 2;
}

export interface Motion {
  /** 0 → 1 as the surface rises out of its blur. */
  readonly enter: number;
  /** 0 → 1 as it blurs out. */
  readonly exit: number;
}

export function surfaceMotion(
  enterAt: number,
  leftAt: number | null,
  at: number,
  durationMs: number,
): Motion {
  return {
    enter: easeOut(ramp(at, enterAt, enterAt + durationMs)),
    exit: leftAt === null ? 0 : easeOut(ramp(at, leftAt, leftAt + durationMs)),
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
