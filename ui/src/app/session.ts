// What the Preact surfaces show, as signals: captions, the thread, the surface column and captured
// chips. Built from protocol events stamped with session time, plus the one interaction that
// changes timing, holding a source chip; the UI holds no state the core owns, just the timestamps
// it needs to animate what the core said (LLD law 1).

import { signal } from "@preact/signals";
import type { CellMark } from "../protocol/generated/CellMark.ts";
import type { ChipKind } from "../protocol/generated/ChipKind.ts";
import type { ColumnRange } from "../protocol/generated/ColumnRange.ts";
import type { Mark } from "../protocol/generated/Mark.ts";
import type { OrbState } from "../protocol/generated/OrbState.ts";
import type { SourceRef } from "../protocol/generated/SourceRef.ts";
import type { Speaker } from "../protocol/generated/Speaker.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { Tokens } from "../styles/tokens.ts";
import { type ColumnTokens, type Placed, SURFACES_VISIBLE_MAX, surfaceHeight } from "./layout.ts";

/** Chips kept on screen or in flight; older ones are dropped (they have long since landed). */
export const CHIPS_MAX = 8;
/** Stack slots for chips waiting to fly. */
export const CHIP_SLOTS = 4;
/** Highlight moves kept per code card; older ones no longer affect the band's easing. */
export const HIGHLIGHTS_MAX = 64;
/** Pen marks kept per code card; the oldest is dropped first (a card shows a few lines at most). */
export const CODE_MARKS_MAX = 16;
/** Session time an item lingers in the store after it left, so its exit can play. */
export const LEAVE_LINGER_MS = 1500;
/**
 * Detour cards open at once: the core's `DETOUR_DEPTH_MAX`. Only the innermost shows; the outer
 * ones wait under it. A push past this (the core never sends one) steps the outermost out.
 */
export const DETOUR_CARDS_MAX = 3;

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

/** A compare cell, keyed `${row}:${col}`: the row is a criterion and the column an option. */
export interface CellView {
  readonly mark: CellMark;
  readonly at: number;
}

/** An orange pen mark on part of a code line (frame 5), drawn from `at`. */
export interface CodeMark {
  readonly line: number;
  readonly columns: ColumnRange;
  readonly mark: Mark;
  readonly at: number;
  /** Its place in the card's marks, in arrival order (its key: marks can repeat). */
  readonly serial: number;
}

/** The idle suggestion under the orb (frame 1): shown, then cleared when the user speaks. */
export interface SuggestionView {
  readonly text: string;
  readonly shownAt: number;
  readonly clearedAt: number | null;
}

export type ColumnItem =
  | (Placed & {
      readonly kind: "detour";
      readonly enterAt: number;
      readonly layer: number;
      readonly title: string;
      /** The question that opened the detour, for the subtitle "Detour · why Redis first?". */
      readonly question: string | null;
    })
  | (Placed & {
      readonly kind: "surface";
      readonly enterAt: number;
      readonly surface: Surface;
      readonly highlights: readonly Highlight[];
      readonly cells: ReadonlyMap<string, CellView>;
      readonly marks: readonly CodeMark[];
    });

export interface Chip {
  readonly serial: number;
  readonly kind: ChipKind;
  readonly text: string;
  /** The code line a source chip opens when clicked; `null` for other chips. */
  readonly source: SourceRef | null;
  readonly at: number;
  readonly slot: number;
  /**
   * When it flies to the brain edge. A source chip fades instead, and never while held: let go,
   * it fades at this deadline, or as it is let go once the deadline has passed.
   */
  readonly flyAt: number;
  /** Set once BoardAI confirmed out loud; until then flyAt is the fallback deadline. */
  readonly confirmed: boolean;
  /** A source chip under the pointer or keyboard focus; `null` when neither holds it. */
  readonly hold: ChipHold | null;
}

/** What holds a source chip in place: the pointer over it, keyboard focus on it, or both. */
export type ChipHolder = "pointer" | "focus";

export interface ChipHold {
  readonly since: number;
  readonly pointer: boolean;
  readonly focus: boolean;
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
  readonly suggestion = signal<SuggestionView | null>(null);
  /**
   * "Open settings" requests heard so far. The live app opens the settings screen on each new
   * one; a replay never does. Not reset with the stage: a request is a one-shot.
   */
  readonly settingsRequests = signal(0);
  /**
   * The settings page is over the stage (UI-local, like a scroll position): the stage's keys
   * wait while it is open. Set by `installSettings`; not reset with the stage.
   */
  readonly settingsOpen = signal(false);
  readonly #tokens: SessionTokens;
  #chipSerial = 0;
  /** Open detail layers, which take no card in the column. */
  readonly #details = new Set<number>();

  constructor(tokens: SessionTokens) {
    this.#tokens = tokens;
  }

  reset(): void {
    this.captions.value = { current: null, previous: null };
    this.thread.value = null;
    this.column.value = [];
    this.chips.value = [];
    this.orb.value = { state: "Idle", previous: "Idle", at: 0 };
    this.suggestion.value = null;
    this.#details.clear();
  }

  apply(event: UiEvent, at: number): void {
    switch (event.type) {
      case "Orb":
        if (event.state !== this.orb.value.state) {
          this.orb.value = { state: event.state, previous: this.orb.value.state, at };
          this.#closeCaption(event.state, at);
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
        // A detail layer (D-188) fills the stage; it is no card in the column.
        if (event.anchor !== null) {
          this.#details.add(event.layer);
          break;
        }
        this.#show({
          kind: "detour",
          key: key({ kind: "detour", layer: event.layer }),
          layer: event.layer,
          title: event.title,
          question: event.question,
          shownAt: at + this.#tokens.durationDetourCardDelayMs,
          enterAt: at + this.#tokens.durationDetourCardDelayMs,
          leftAt: null,
          cover: null,
          height: this.#tokens.detourCardHeight,
        });
        break;
      case "LayerPop":
        if (this.#details.delete(event.layer)) {
          break;
        }
        this.#leave(key({ kind: "detour", layer: event.layer }), at);
        this.#uncover(at);
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
        this.#chip(event.kind, event.text, event.source, at);
        break;
      case "Suggestion":
        this.#suggest(event.text, at);
        break;
      case "OpenSettings":
        this.settingsRequests.value += 1;
        break;
      case "SessionEnd":
        this.#end(at);
        break;
      case "Acknowledge":
      case "OpStart":
      case "Freeze":
      case "Resume":
      case "Camera":
      case "SessionStart":
      case "ViewShow":
      case "ViewDismiss":
      case "DrawMode":
      case "UserShape":
      case "UserShapeRemove":
      case "LabelHint":
      // The core closes the detours and cards a page turn takes away with their own events.
      case "PageTurn":
      // Staged diagrams' headers and detail layers are the diagram store's.
      case "Diagram":
      // The tray and the drop overlay have their own store (sources/model.ts).
      case "Sources":
      case "SourceDrop":
        break;
    }
  }

  /**
   * The session ended (D-145): every card leaves as a dismissed one does, the thread goes, the
   * suggestion is cleared and the lines close so they fade.
   */
  #end(at: number): void {
    this.column.value = this.column.value.map((item) =>
      item.leftAt === null ? { ...item, leftAt: at } : item,
    );
    this.thread.value = null;
    this.#suggest(null, at);
    const { current, previous } = this.captions.value;
    if (current !== null && current.finalAt === null) {
      this.captions.value = { current: { ...current, finalAt: at }, previous };
    }
  }

  #suggest(text: string | null, at: number): void {
    const current = this.suggestion.value;
    if (text !== null) {
      this.suggestion.value = { text, shownAt: at, clearedAt: null };
    } else if (current !== null && current.clearedAt === null) {
      this.suggestion.value = { ...current, clearedAt: at };
    }
  }

  #caption(who: Speaker, text: string, isFinal: boolean, at: number, detail: string | null): void {
    const { current } = this.captions.value;
    // The core captions the user as the whole utterance so far, final once per voiced segment
    // (D-035): a later segment of the same turn (the orb hasn't moved since) extends the line.
    const nextSegment =
      current !== null &&
      who === "User" &&
      current.who === "User" &&
      current.startAt >= this.orb.value.at &&
      text.startsWith(current.text);
    const continues =
      current !== null &&
      current.who === who &&
      detail === null &&
      (current.finalAt === null || nextSegment);
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

  /**
   * A line that will get no more words is closed, so it lingers and fades like a finished one:
   * your line once the floor leaves you, and a clause a stop cut once BoardAI is no longer
   * speaking. A duck (the orb listening over the voice) may still resume, so it stays open.
   */
  #closeCaption(state: OrbState, at: number): void {
    const { current, previous } = this.captions.value;
    if (current === null || current.finalAt !== null) {
      return;
    }
    const over =
      current.who === "User"
        ? state !== "Listening"
        : state === "Idle" || state === "Thinking" || state === "Muted";
    if (over) {
      this.captions.value = { current: { ...current, finalAt: at }, previous };
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
    // Surfaces and detour cards are capped apart, as the core caps them: the core dismisses its
    // own surfaces, so stepping the oldest out here only bounds what the column can hold.
    const max = item.kind === "detour" ? DETOUR_CARDS_MAX : SURFACES_VISIBLE_MAX;
    const open = next.filter((existing) => existing.kind === item.kind && existing.leftAt === null);
    for (let index = 0; index <= open.length - max; index += 1) {
      const oldest = open[index];
      if (oldest !== undefined) {
        next[next.indexOf(oldest)] = { ...oldest, leftAt: at };
      }
    }
    if (item.kind === "detour") {
      // A nested detour's card takes the outer one's place until it closes.
      next.forEach((existing, index) => {
        const covered = existing.cover !== null && existing.cover.to === null;
        if (existing.kind === "detour" && existing.leftAt === null && !covered) {
          next[index] = { ...existing, cover: { from: at, to: null } };
        }
      });
    }
    next.push(item);
    this.column.value = next;
  }

  /** A detour closed: the innermost one still open comes back from under it. */
  #uncover(at: number): void {
    const items = this.column.value;
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const item = items[index];
      if (item === undefined || item.kind !== "detour" || item.leftAt !== null) {
        continue;
      }
      if (item.cover !== null && item.cover.to === null) {
        const next = [...items];
        next[index] = { ...item, cover: { from: item.cover.from, to: at } };
        this.column.value = next;
      }
      return;
    }
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
      cover: null,
      height: surfaceHeight(surface, this.#tokens),
      highlights:
        surface.kind === "Code" && surface.highlight !== null
          ? [{ line: surface.highlight, at }]
          : [],
      cells: new Map(),
      marks: [],
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
      return updated(item, update, at);
    });
  }

  #leave(target: string, at: number): void {
    this.column.value = this.column.value.map((item) =>
      item.key === target && item.leftAt === null ? { ...item, leftAt: at } : item,
    );
  }

  #chip(kind: ChipKind, text: string, source: SourceRef | null, at: number): void {
    const tokens = this.#tokens;
    // A held chip keeps its slot past its deadline, so a new chip never lands on it.
    const waiting = this.chips.value.filter(
      (chip) => chip.hold !== null || chip.flyAt + tokens.durationChipFlyMs > at,
    );
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
      source,
      at,
      slot,
      flyAt: at + tokens.durationChipHoldMaxMs,
      confirmed: false,
      hold: null,
    };
    this.chips.value = [...this.chips.value, chip].slice(-CHIPS_MAX);
  }

  /** The pointer or keyboard focus came onto a source chip (`on`), or left it. */
  holdChip(serial: number, holder: ChipHolder, on: boolean, at: number): void {
    let changed = false;
    const chips = this.chips.value.map((chip) => {
      if (chip.serial !== serial) {
        return chip;
      }
      const next = chipHeld(chip, holder, on, at);
      changed ||= next !== chip;
      return next;
    });
    if (changed) {
      this.chips.value = chips;
    }
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

/**
 * A chip with a holder on or off. Only a source chip holds (the others take no pointer). The hold
 * keeps its start while a second holder joins, and letting go of the last one moves the fade
 * deadline to that moment if it had passed. The same chip comes back when nothing changed.
 */
export function chipHeld(chip: Chip, holder: ChipHolder, on: boolean, at: number): Chip {
  if (chip.kind !== "Source") {
    return chip;
  }
  const hold = chip.hold;
  const pointer = holder === "pointer" ? on : (hold?.pointer ?? false);
  const focus = holder === "focus" ? on : (hold?.focus ?? false);
  if (pointer || focus) {
    if (hold !== null && hold.pointer === pointer && hold.focus === focus) {
      return chip;
    }
    return { ...chip, hold: { since: hold?.since ?? at, pointer, focus } };
  }
  if (hold === null) {
    return chip;
  }
  return { ...chip, hold: null, flyAt: Math.max(chip.flyAt, at) };
}

/**
 * What clicking a chip asks of the core: a source chip opens its code line (the core shows it only
 * if that line was read this session); any other chip asks nothing.
 */
export function chipCommand(chip: Pick<Chip, "kind" | "source">): UiCommand | null {
  if (chip.kind !== "Source" || chip.source === null) {
    return null;
  }
  return { type: "OpenSource", path: chip.source.path, line: chip.source.line };
}

/** A surface card with one update applied: a highlight move, a compare cell or a pen mark. */
function updated(
  item: Extract<ColumnItem, { kind: "surface" }>,
  update: Extract<UiEvent, { type: "SurfaceUpdate" }>["update"],
  at: number,
): ColumnItem {
  switch (update.kind) {
    case "Highlight":
      return {
        ...item,
        highlights: [...item.highlights.slice(-(HIGHLIGHTS_MAX - 1)), { line: update.line, at }],
      };
    case "Cell": {
      const cells = new Map(item.cells);
      cells.set(`${update.row}:${update.col}`, { mark: update.mark, at });
      return { ...item, cells };
    }
    case "Mark": {
      const serial = (item.marks[item.marks.length - 1]?.serial ?? -1) + 1;
      const mark = { line: update.line, columns: update.columns, mark: update.mark, at, serial };
      return { ...item, marks: [...item.marks.slice(-(CODE_MARKS_MAX - 1)), mark] };
    }
  }
}
