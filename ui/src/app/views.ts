// The view summoned by voice (LLD §3.7): the library, the Now list, memory, a past board or a
// replay. The core decides which one is open (`ViewShow` replaces, `ViewDismiss` closes); this
// store only keeps what it needs to animate that, stamped with session time like everything else
// (D-029), so a replay seek lands on the same frame.

import { signal } from "@preact/signals";
import { easeOut, ramp } from "../canvas/math.ts";
import type { View } from "../protocol/generated/View.ts";

/** Views kept while they leave; older ones are dropped (they finished leaving long ago). */
export const VIEW_ENTRIES_MAX = 4;
/** Session time a closed view is kept after it left, so its exit can play. */
export const VIEW_LEAVE_LINGER_MS = 1500;

export type ViewKindName = View["kind"];

export interface ViewEntry {
  /** Increments per shown view, so a replaced view of the same kind is a new element. */
  readonly serial: number;
  readonly view: View;
  /** The day the runtime read it (`YYYY-MM-DD`), for "today" labels. */
  readonly today: string;
  readonly shownAt: number;
  readonly leftAt: number | null;
}

/** How a view moves: the panels slide in from the right edge, the rest rise out of a blur. */
export type ViewEntrance = "panel" | "rise";

export function entranceOf(kind: ViewKindName): ViewEntrance {
  switch (kind) {
    case "Tasks":
    case "Memory":
      return "panel";
    case "Library":
    case "PastSession":
    case "Replay":
      return "rise";
  }
}

export interface ViewMotion {
  /** 0 → 1 as the view arrives. */
  readonly enter: number;
  /** 0 → 1 as it leaves. */
  readonly exit: number;
}

export interface ViewDurations {
  /** Panels arrive over a scene duration (the canvas's pearlPanelIn, 900 ms). */
  readonly sceneMs: number;
  /** Everything else arrives, and every view leaves, over the standard duration. */
  readonly standardMs: number;
}

export function viewMotion(entry: ViewEntry, at: number, durations: ViewDurations): ViewMotion {
  const enterMs =
    entranceOf(entry.view.kind) === "panel" ? durations.sceneMs : durations.standardMs;
  return {
    enter: easeOut(ramp(at, entry.shownAt, entry.shownAt + enterMs)),
    exit:
      entry.leftAt === null
        ? 0
        : easeOut(ramp(at, entry.leftAt, entry.leftAt + durations.standardMs)),
  };
}

/** Whether an entry is on screen at `at` (arrived, and not done leaving). */
export function viewVisible(entry: ViewEntry, at: number, durations: ViewDurations): boolean {
  return at >= entry.shownAt && viewMotion(entry, at, durations).exit < 1;
}

export class ViewStore {
  readonly entries = signal<readonly ViewEntry[]>([]);
  #serial = 0;

  reset(): void {
    this.entries.value = [];
  }

  /** The view the core has open (not leaving), if any. */
  get open(): ViewEntry | null {
    const entries = this.entries.value;
    const last = entries[entries.length - 1];
    return last !== undefined && last.leftAt === null ? last : null;
  }

  show(view: View, today: string, at: number): void {
    this.#serial += 1;
    const kept = this.#leaving(at);
    kept.push({ serial: this.#serial, view, today, shownAt: at, leftAt: null });
    this.entries.value = kept.slice(-VIEW_ENTRIES_MAX);
  }

  dismiss(at: number): void {
    if (this.open === null) {
      return;
    }
    this.entries.value = this.#leaving(at);
  }

  /** Every entry, the open one marked as leaving at `at`; long-gone ones are dropped. */
  #leaving(at: number): ViewEntry[] {
    return this.entries.value
      .filter((entry) => entry.leftAt === null || at - entry.leftAt < VIEW_LEAVE_LINGER_MS)
      .map((entry) => (entry.leftAt === null ? { ...entry, leftAt: at } : entry));
  }
}
