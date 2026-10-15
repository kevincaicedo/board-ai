// Maps session time to an op's own "natural" time, honoring its budget and any freezes. An op
// draws at the design's human pace; when that pace would overrun `budget_ms`, the whole op is
// compressed to fit, so drawing never piles up (LLD §2.4). `Freeze` holds natural time still and
// `Resume` continues from exactly that point with a fresh budget for the rest (LLD §2.7).

/** Freezes kept per op; one more is ignored rather than growing the list without bound. */
export const OP_FREEZES_MAX = 16;

interface Segment {
  /** Session time the segment begins. */
  readonly from: number;
  /** Natural time at `from`. */
  readonly naturalAt: number;
  /** Natural ms per session ms; 0 while frozen. */
  readonly rate: number;
}

export interface OpClock {
  readonly startAt: number;
  /** Natural length of the whole op: the pen's lead-in plus every item. */
  readonly naturalMs: number;
  readonly segments: Segment[];
}

function rateFor(remainingMs: number, budgetMs: number): number {
  if (budgetMs <= 0) {
    return remainingMs > 0 ? Number.POSITIVE_INFINITY : 1;
  }
  return remainingMs > budgetMs ? remainingMs / budgetMs : 1;
}

export function startClock(startAt: number, naturalMs: number, budgetMs: number): OpClock {
  return {
    startAt,
    naturalMs,
    segments: [{ from: startAt, naturalAt: 0, rate: rateFor(naturalMs, budgetMs) }],
  };
}

/** Natural time at session time `time`, in [0, naturalMs]; negative before the op starts. */
export function naturalTime(clock: OpClock, time: number): number {
  if (time < clock.startAt) {
    return -1;
  }
  let segment = clock.segments[0];
  for (let index = clock.segments.length - 1; index >= 0; index -= 1) {
    const candidate = clock.segments[index];
    if (candidate !== undefined && candidate.from <= time) {
      segment = candidate;
      break;
    }
  }
  if (segment === undefined) {
    return -1;
  }
  if (segment.rate === Number.POSITIVE_INFINITY) {
    return clock.naturalMs;
  }
  return Math.min(clock.naturalMs, segment.naturalAt + (time - segment.from) * segment.rate);
}

export function isFrozen(clock: OpClock, time: number): boolean {
  for (let index = clock.segments.length - 1; index >= 0; index -= 1) {
    const segment = clock.segments[index];
    if (segment !== undefined && segment.from <= time) {
      return segment.rate === 0;
    }
  }
  return false;
}

/** Session time at which the op finishes, or null while it is frozen for good. */
export function endTime(clock: OpClock): number | null {
  const last = clock.segments[clock.segments.length - 1];
  if (last === undefined || last.rate === 0) {
    return null;
  }
  if (last.rate === Number.POSITIVE_INFINITY) {
    return last.from;
  }
  return last.from + (clock.naturalMs - last.naturalAt) / last.rate;
}

export function isDone(clock: OpClock, time: number): boolean {
  return naturalTime(clock, time) >= clock.naturalMs;
}

/** Holds the op where it is at `time`. Returns false when there was nothing in progress. */
export function freezeClock(clock: OpClock, time: number): boolean {
  if (time < clock.startAt || isFrozen(clock, time) || isDone(clock, time)) {
    return false;
  }
  if (clock.segments.length >= OP_FREEZES_MAX * 2) {
    return false;
  }
  clock.segments.push({ from: time, naturalAt: naturalTime(clock, time), rate: 0 });
  return true;
}

/** Continues a frozen op from where it stopped, fitting what is left into `budgetMs`. */
export function resumeClock(clock: OpClock, time: number, budgetMs: number): boolean {
  if (!isFrozen(clock, time)) {
    return false;
  }
  const at = naturalTime(clock, time);
  clock.segments.push({ from: time, naturalAt: at, rate: rateFor(clock.naturalMs - at, budgetMs) });
  return true;
}
