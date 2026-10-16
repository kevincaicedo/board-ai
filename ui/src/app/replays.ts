// Replays of past sessions opened from a card (the library's play buttons, the recall view's
// "Session replay" card) and how far each has been watched in this app run. This is the UI's own
// navigation inside a view the core opened, like a scroll position: the core only knows which
// view is open, and closing that view closes these too.

import { signal } from "@preact/signals";
import type { SessionCard } from "../protocol/generated/SessionCard.ts";

/** Sessions whose progress is remembered; past this the oldest is forgotten. */
export const REPLAY_PROGRESS_MAX = 500;
/** Progress moves in steps of this much, so the library re-renders rarely. */
const PROGRESS_STEP = 0.01;

/** Where a replay was started from: Esc goes back there. */
export type ReplayOrigin = "library" | "recall";

export interface LocalReplay {
  readonly card: SessionCard;
  readonly from: ReplayOrigin;
}

export class Replays {
  readonly local = signal<LocalReplay | null>(null);
  /** The furthest point reached in each session's replay, 0 to 1. */
  readonly progress = signal<ReadonlyMap<string, number>>(new Map());

  open(card: SessionCard, from: ReplayOrigin): void {
    this.local.value = { card, from };
  }

  close(): void {
    this.local.value = null;
  }

  /** Records how far a replay got; it only ever moves forward. */
  record(id: string, fraction: number): void {
    if (!Number.isFinite(fraction)) {
      return;
    }
    const stepped = Math.floor(Math.min(1, Math.max(0, fraction)) / PROGRESS_STEP) * PROGRESS_STEP;
    const known = this.progress.value.get(id) ?? 0;
    if (stepped <= known) {
      return;
    }
    const next = new Map(this.progress.value);
    next.delete(id);
    next.set(id, Math.round(stepped * 100) / 100);
    while (next.size > REPLAY_PROGRESS_MAX) {
      const oldest = next.keys().next().value;
      if (oldest === undefined) {
        break;
      }
      next.delete(oldest);
    }
    this.progress.value = next;
  }

  progressOf(id: string): number {
    return this.progress.value.get(id) ?? 0;
  }
}

/** What Esc does: leave a card's replay first, then ask the core to close its view. */
export type EscapeAction = "close-replay" | "close-view" | "none";

export function escapeAction(localReplayOpen: boolean, coreViewOpen: boolean): EscapeAction {
  if (localReplayOpen) {
    return "close-replay";
  }
  return coreViewOpen ? "close-view" : "none";
}
