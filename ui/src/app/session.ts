// What the Preact surfaces show, as signals: captions, the thread, the surface column and captured
// chips. Built only from protocol events stamped with session time; the UI holds no state the
// core owns, just the timestamps it needs to animate what the core said (LLD law 1).

import { signal } from "@preact/signals";
import type { CellMark } from "../protocol/generated/CellMark.ts";
import type { ChipKind } from "../protocol/generated/ChipKind.ts";
import type { OrbState } from "../protocol/generated/OrbState.ts";
import type { Speaker } from "../protocol/generated/Speaker.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { Tokens } from "../styles/tokens.ts";
import { COLUMN_VISIBLE_MAX, type ColumnTokens, type Placed, surfaceHeight } from "./layout.ts";

/** Chips kept on screen or in flight; older ones are dropped (they have long since landed). */
export const CHIPS_MAX = 8;
/** Stack slots for chips waiting to fly. */
export const CHIP_SLOTS = 4;
/** Highlight moves kept per code card; older ones no longer affect the band's easing. */
export const HIGHLIGHTS_MAX = 64;
/** Session time an item lingers in the store after it left, so its exit can play. */
export const LEAVE_LINGER_MS = 1500;

export interface CaptionLine {
  readonly who: Speaker;
  readonly text: string;
  readonly startAt: number;
  /** When the last word arrived, so it can fade in. */
  readonly wordAt: number;
  readonly finalAt: number | null;
  /** A spoken failure (Problem): its detail shows on the older line. */
  readonly detail: string | null;
}

export interface Captions {
  readonly current: CaptionLine | null;
  readonly previous: CaptionLine | null;
}

export interface ThreadView {
  readonly steps: readonly string[];
  readonly current: number | null;
  readonly shownAt: number;
  /** When each step became current (or was passed), for the progress line's easing. */
  readonly reachedAt: readonly (number | null)[];
  readonly currentAt: number;
  readonly detours: readonly {
    readonly title: string;
    readonly openAt: number;
    readonly closedAt: number | null;
  }[];
}

export interface Highlight {
  readonly line: number;
  readonly at: number;
}

export interface CellView {
  readonly mark: CellMark;
  readonly at: number;
}

export type ColumnItem =
  | (Placed & {
      readonly kind: "detour";
      readonly enterAt: number;
      readonly layer: number;
      readonly title: string;
    })
  | (Placed & {
      readonly kind: "surface";
      readonly enterAt: number;
      readonly surface: Surface;
      readonly highlights: readonly Highlight[];
      readonly cells: ReadonlyMap<string, CellView>;
    });

export interface Chip {
  readonly serial: number;
  readonly kind: ChipKind;
  readonly text: string;
  readonly at: number;
  readonly slot: number;
  /** When it flies to the brain edge (a source chip fades instead). */
  readonly flyAt: number;
  /** Set once BoardAI confirmed out loud; until then flyAt is the fallback deadline. */
  readonly confirmed: boolean;
}

export interface OrbStatus {
  readonly state: OrbState;
  readonly previous: OrbState;
  readonly at: number;
}

export type SessionTokens = ColumnTokens &
  Pick<
    Tokens,
    | "durationDetourCardDelayMs"
    | "durationSurfaceDelayMs"
    | "durationChipConfirmMs"
    | "durationChipStaggerMs"
    | "durationChipHoldMaxMs"
    | "durationChipFlyMs"
  >;

function key(item: { kind: "detour"; layer: number } | { kind: "surface"; id: number }): string {
  return item.kind === "detour" ? `layer-${item.layer}` : `surface-${item.id}`;
}

export class SessionStore {
  readonly captions = signal<Captions>({ current: null, previous: null });
  readonly thread = signal<ThreadView | null>(null);
  readonly column = signal<readonly ColumnItem[]>([]);
  readonly chips = signal<readonly Chip[]>([]);
  readonly orb = signal<OrbStatus>({ state: "Idle", previous: "Idle", at: 0 });
  readonly #tokens: SessionTokens;
  #chipSerial = 0;

  constructor(tokens: SessionTokens) {
    this.#tokens = tokens;
  }

  reset(): void {
    this.captions.value = { current: null, previous: null };
    this.thread.value = null;
    this.column.value = [];
    this.chips.value = [];
    this.orb.value = { state: "Idle", previous: "Idle", at: 0 };
  }

  apply(event: UiEvent, at: number): void {
    switch (event.type) {
      case "Orb":
        if (event.state !== this.orb.value.state) {
          this.orb.value = { state: event.state, previous: this.orb.value.state, at };
        }
        break;
      case "Caption":
        this.#caption(event.who, event.text, event.is_final, at, null);
        break;
      case "Problem":
        this.#caption("Assistant", event.spoken, true, at, event.detail);
        break;
      case "Thread":
        this.#thread(event.steps, event.current, event.detours, at);
        break;
      case "LayerPush":
        this.#show({
          kind: "detour",
          key: key({ kind: "detour", layer: event.layer }),
          layer: event.layer,
          title: event.title,
          shownAt: at + this.#tokens.durationDetourCardDelayMs,
          enterAt: at + this.#tokens.durationDetourCardDelayMs,
          leftAt: null,
          height: this.#tokens.detourCardHeight,
        });
        break;
      case "LayerPop":
        this.#leave(key({ kind: "detour", layer: event.layer }), at);
        break;
      case "SurfaceShow":
        this.#showSurface(event.surface, at);
        break;
      case "SurfaceUpdate":
        this.#updateSurface(event.id, event.update, at);
        break;
      case "SurfaceDismiss":
        this.#leave(key({ kind: "surface", id: event.id }), at);
        break;
      case "Chip":
        this.#chip(event.kind, event.text, at);
        break;
      case "Acknowledge":
      case "OpStart":
      case "Freeze":
      case "Resume":
      case "Camera":
        break;
    }
  }

  #caption(who: Speaker, text: string, isFinal: boolean, at: number, detail: string | null): void {
    const { current } = this.captions.value;
    const continues =
      current !== null && current.finalAt === null && current.who === who && detail === null;
    if (continues) {
      const grew = text !== current.text;
      this.captions.value = {
        current: {
          ...current,
          text,
          wordAt: grew ? at : current.wordAt,
          finalAt: isFinal ? at : null,
        },
        previous: this.captions.value.previous,
      };
      return;
    }
    this.captions.value = {
      current: { who, text, startAt: at, wordAt: at, finalAt: isFinal ? at : null, detail },
      previous: current,
    };
    if (who === "Assistant") {
      this.#confirmChips(at);
    }
  }

  #thread(
    steps: readonly string[],
    current: number | null,
    detours: readonly string[],
    at: number,
  ): void {
    const previous = this.thread.value;
    const sameSteps =
      previous !== null &&
      previous.steps.length === steps.length &&
      previous.steps.every((step, index) => step === steps[index]);
    const reachedAt: (number | null)[] = steps.map((_, index) =>
      sameSteps ? (previous?.reachedAt[index] ?? null) : null,
    );
    if (current !== null) {
      for (let index = 0; index <= current && index < reachedAt.length; index += 1) {
        if (reachedAt[index] === null) {
          reachedAt[index] = at;
        }
      }
    }
    const open = (previous?.detours ?? []).filter((detour) => detour.closedAt === null);
    const kept = (previous?.detours ?? []).filter(
      (detour) => detour.closedAt !== null && at - detour.closedAt < LEAVE_LINGER_MS,
    );
    const nextDetours: ThreadView["detours"][number][] = [...kept];
    detours.forEach((title, index) => {
      const existing = open[index];
      nextDetours.push(
        existing !== undefined && existing.title === title
          ? existing
          : { title, openAt: at, closedAt: null },
      );
    });
    for (let index = detours.length; index < open.length; index += 1) {
      const closing = open[index];
      if (closing !== undefined) {
        nextDetours.push({ ...closing, closedAt: at });
      }
    }
    this.thread.value = {
      steps,
      current,
      shownAt: sameSteps && previous !== null ? previous.shownAt : at,
      reachedAt,
      currentAt:
        previous !== null && previous.current === current && sameSteps ? previous.currentAt : at,
      detours: nextDetours,
    };
  }

  #show(item: ColumnItem): void {
    const at = item.shownAt;
    const items = this.column.value.filter(
      (existing) => existing.leftAt === null || at - existing.leftAt < LEAVE_LINGER_MS,
    );
    const next: ColumnItem[] = items.map((existing) =>
      existing.key === item.key && existing.leftAt === null
        ? { ...existing, leftAt: at }
        : existing,
    );
    // Only two surfaces beside the canvas: the oldest steps out for the newest.
    const visible = next.filter((existing) => existing.leftAt === null);
    for (let index = 0; index <= visible.length - COLUMN_VISIBLE_MAX; index += 1) {
      const oldest = visible[index];
      if (oldest !== undefined) {
        const position = next.indexOf(oldest);
        next[position] = { ...oldest, leftAt: at };
      }
    }
    next.push(item);
    this.column.value = next;
  }

  #showSurface(surface: Surface, at: number): void {
    const others = this.column.value.some(
      (existing) => existing.leftAt === null || existing.leftAt > at,
    );
    this.#show({
      kind: "surface",
      key: key({ kind: "surface", id: surface.id }),
      surface,
      shownAt: at,
      enterAt: others ? at + this.#tokens.durationSurfaceDelayMs : at,
      leftAt: null,
      height: surfaceHeight(surface, this.#tokens),
      highlights:
        surface.kind === "Code" && surface.highlight !== null
          ? [{ line: surface.highlight, at }]
          : [],
      cells: new Map(),
    });
  }

  #updateSurface(
    id: number,
    update: Extract<UiEvent, { type: "SurfaceUpdate" }>["update"],
    at: number,
  ): void {
    const target = key({ kind: "surface", id });
    this.column.value = this.column.value.map((item) => {
      if (item.key !== target || item.kind !== "surface" || item.leftAt !== null) {
        return item;
      }
      if (update.kind === "Highlight") {
        return {
          ...item,
          highlights: [...item.highlights.slice(-(HIGHLIGHTS_MAX - 1)), { line: update.line, at }],
        };
      }
      const cells = new Map(item.cells);
      cells.set(`${update.row}:${update.col}`, { mark: update.mark, at });
      return { ...item, cells };
    });
  }

  #leave(target: string, at: number): void {
    this.column.value = this.column.value.map((item) =>
      item.key === target && item.leftAt === null ? { ...item, leftAt: at } : item,
    );
  }

  #chip(kind: ChipKind, text: string, at: number): void {
    const tokens = this.#tokens;
    const waiting = this.chips.value.filter((chip) => chip.flyAt + tokens.durationChipFlyMs > at);
    const taken = new Set(waiting.map((chip) => chip.slot));
    let slot = 0;
    while (taken.has(slot) && slot < CHIP_SLOTS - 1) {
      slot += 1;
    }
    this.#chipSerial += 1;
    const chip: Chip = {
      serial: this.#chipSerial,
      kind,
      text,
      at,
      slot,
      flyAt: at + tokens.durationChipHoldMaxMs,
      confirmed: false,
    };
    this.chips.value = [...this.chips.value, chip].slice(-CHIPS_MAX);
  }

  /** BoardAI started speaking: waiting chips fly once it has said "Noted" (PRODUCT §6.2 capture). */
  #confirmChips(at: number): void {
    const tokens = this.#tokens;
    let order = 0;
    let changed = false;
    const chips = this.chips.value.map((chip) => {
      if (chip.confirmed || chip.kind === "Source" || chip.at > at || at > chip.flyAt) {
        return chip;
      }
      const flyAt = Math.min(
        chip.flyAt,
        at + tokens.durationChipConfirmMs + order * tokens.durationChipStaggerMs,
      );
      order += 1;
      changed = true;
      return { ...chip, flyAt, confirmed: true };
    });
    if (changed) {
      this.chips.value = chips;
    }
  }
}
