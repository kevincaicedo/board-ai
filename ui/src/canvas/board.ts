// The board as the UI sees it (LLD §2.5): layers of elements, each drawn by one op run on its own
// clock. This module holds no DOM; the renderer reads it every frame. Everything is stamped with
// session time, so rebuilding from the event log at any time gives the same board (replay seek).

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { Vec } from "./geometry.ts";
import { easeOut, ramp } from "./math.ts";
import {
  labelOf,
  labelOfTarget,
  type OpPlan,
  type PlanTokens,
  planOp,
  type StruckLabel,
  traceStroke,
} from "./plan.ts";
import { itemEnd, itemStart, type Stroke, travelMs } from "./stroke.ts";
import {
  freezeClock,
  isDone,
  isFrozen,
  type OpClock,
  resumeClock,
  startClock,
} from "./timeline.ts";

/**
 * Elements drawn per layer: the core's `ELEMENTS_PER_LAYER_MAX`. Strikes are kept to the same
 * number apart (one per element); a re-sent element or a second strike replaces its drawing and
 * adds nothing. An op for a new element past this is dropped and counted (the core never sends
 * one). Replaced drawings are let go once the layer holds twice this many runs.
 */
export const LAYER_ELEMENTS_MAX = 500;
/** Layers that can be open at once: the main board plus the core's detour depth limit (3). */
export const LAYERS_MAX = 4;
/** Detour spans remembered for the board's push-back animation; older ones are dropped. */
export const DETOUR_SPANS_MAX = 8;
/** A trace dot never laps faster than this, whatever its budget. */
export const TRACE_PERIOD_MIN_MS = 300;

export type BoardEvent = Extract<
  UiEvent,
  { type: "OpStart" | "Freeze" | "Resume" | "LayerPush" | "LayerPop" | "Camera" }
>;

/** Where the pen comes from before an op's first stroke. */
export type Lead =
  | { readonly kind: "none" }
  | { readonly kind: "travel"; readonly from: Vec }
  /** The nib flies out of the orb (or back out of the dock) on an arc, after any wait. */
  | {
      readonly kind: "flight";
      readonly from: "center" | "dock";
      readonly waitMs: number;
      readonly flightMs: number;
    };

export interface Run {
  readonly layer: number;
  /** The element this run draws; a later op with the same key replaces it. */
  readonly key: string;
  readonly op: DrawOp;
  readonly plan: OpPlan;
  readonly clock: OpClock;
  readonly lead: Lead;
  readonly leadMs: number;
  /** For a trace: the path its dot follows and the time of one lap. */
  readonly trace: { readonly stroke: Stroke; readonly periodMs: number } | null;
  replacedAt: number | null;
  /**
   * A trace's dot laps until the topic moves on; a stop (`Freeze` on its layer) ends it, since
   * the voice it followed stopped and `Resume` only finishes strokes.
   */
  stoppedAt: number | null;
  /** Incremented when a newer run replaced this one, so the renderer can drop it. */
  readonly serial: number;
}

export interface Camera {
  readonly frame: Rect | null;
  readonly previous: Rect | null;
  readonly at: number;
  readonly focus: string | null;
  readonly previousFocus: string | null;
  readonly focusAt: number;
}

export interface Layer {
  readonly id: number;
  readonly title: string;
  readonly pushedAt: number;
  poppedAt: number | null;
  readonly runs: Run[];
  camera: Camera;
}

export interface DetourSpan {
  readonly openAt: number;
  closeAt: number | null;
}

export interface BoardCounters {
  droppedOps: number;
  orphanEvents: number;
}

const NO_CAMERA: Camera = {
  frame: null,
  previous: null,
  at: 0,
  focus: null,
  previousFocus: null,
  focusAt: 0,
};

/** The element a run draws: its own id, or for a strike and a written value its target's. */
function keyOf(op: DrawOp): string {
  switch (op.kind) {
    case "Strike":
      return `${op.target}#strike`;
    case "Write":
      return `${op.target}#write`;
    case "Node":
    case "Edge":
    case "Annotate":
    case "Trace":
    case "Participant":
    case "Message":
    case "Cells":
      return op.id;
  }
}

/** What the presence (orb and nib) tells the board when a main-layer op starts. */
export interface LeadDecision {
  /** Where the nib is at `at`: out on the canvas, waiting where the orb condensed, or docked. */
  nibAtRest(at: number): "out" | "center" | "dock";
  /** A flight's wait (for the orb to finish condensing) and its duration. */
  flightLead(from: "center" | "dock", at: number): { waitMs: number; flightMs: number };
}

export class BoardModel {
  readonly layers = new Map<number, Layer>();
  readonly detours: DetourSpan[] = [];
  readonly counters: BoardCounters = { droppedOps: 0, orphanEvents: 0 };
  /**
   * When the session ended (`SessionEnd`): the board fades out and the nib rests, until the
   * next session's start resets the board (D-182).
   */
  endedAt: number | null = null;
  /** Bumped on every structural change (a run added or replaced, a layer pushed or popped). */
  version = 0;
  readonly #tokens: PlanTokens;
  #serial = 0;

  constructor(tokens: PlanTokens) {
    this.#tokens = tokens;
    this.reset();
  }

  reset(): void {
    this.endedAt = null;
    this.layers.clear();
    this.detours.length = 0;
    this.counters.droppedOps = 0;
    this.counters.orphanEvents = 0;
    this.layers.set(0, {
      id: 0,
      title: "",
      pushedAt: 0,
      poppedAt: null,
      runs: [],
      camera: NO_CAMERA,
    });
    this.version += 1;
  }

  get main(): Layer {
    const main = this.layers.get(0);
    if (main === undefined) {
      throw new Error("the main layer always exists");
    }
    return main;
  }

  /** The session ended at `at`: its board fades out (the first end counts). */
  end(at: number): void {
    this.endedAt ??= at;
  }

  /** How far the ended board has faded out at `at`, from 0 to 1. */
  endedAmount(at: number, durationMs: number): number {
    const endedAt = this.endedAt;
    return endedAt === null ? 0 : easeOut(ramp(at, endedAt, endedAt + durationMs));
  }

  /** Open detour layers, outermost first. */
  openDetours(): Layer[] {
    const open: Layer[] = [];
    for (const layer of this.layers.values()) {
      if (layer.id !== 0 && layer.poppedAt === null) {
        open.push(layer);
      }
    }
    return open.sort((a, b) => a.pushedAt - b.pushedAt);
  }

  apply(event: BoardEvent, at: number, lead: LeadDecision): void {
    switch (event.type) {
      case "OpStart":
        this.#start(event.layer, event.op, event.budget_ms, at, lead);
        break;
      case "Freeze":
        this.#freeze(event.layer, at);
        break;
      case "Resume":
        this.#resume(event.layer, at, event.budget_ms);
        break;
      case "LayerPush":
        this.#push(event.layer, event.title, at);
        break;
      case "LayerPop":
        this.#pop(event.layer, at);
        break;
      case "Camera":
        this.#camera(event.layer, event.frame, event.focus, at);
        break;
    }
  }

  /** The newest run on the main layer that the nib follows (traces don't move the nib). */
  latestPenRun(layer: Layer, at: number): Run | null {
    for (let index = layer.runs.length - 1; index >= 0; index -= 1) {
      const run = layer.runs[index];
      if (run !== undefined && run.trace === null && run.clock.startAt <= at) {
        return run;
      }
    }
    return null;
  }

  #layer(id: number, at: number): Layer | null {
    const existing = this.layers.get(id);
    if (existing !== undefined) {
      return existing;
    }
    if (this.layers.size >= LAYERS_MAX) {
      return null;
    }
    // An op for a layer that was never pushed: show it rather than lose it, and count it.
    this.counters.orphanEvents += 1;
    this.#push(id, "", at);
    return this.layers.get(id) ?? null;
  }

  #start(layerId: number, op: DrawOp, budgetMs: number, at: number, decision: LeadDecision): void {
    const layer = this.#layer(layerId, at);
    if (layer === null) {
      this.counters.droppedOps += 1;
      return;
    }
    const key = keyOf(op);
    // An element re-sent (a group grown for a new child) carries its current label, so a strike
    // written over its old one goes with the old drawing.
    const struck = `${key}#strike`;
    let replaces = false;
    let sameKind = 0;
    for (const run of layer.runs) {
      if (run.replacedAt !== null) {
        continue;
      }
      replaces ||= run.key === key;
      if ((run.op.kind === "Strike") === (op.kind === "Strike")) {
        sameKind += 1;
      }
    }
    const target = op.kind === "Strike" ? struckLabel(layer.runs, op.target) : null;
    const round =
      op.kind === "Annotate" &&
      op.mark === "Highlight" &&
      layer.runs.some(
        (run) =>
          run.replacedAt === null &&
          run.key === op.target &&
          run.op.kind === "Node" &&
          run.op.shape === "Circle",
      );
    if (!replaces && sameKind >= LAYER_ELEMENTS_MAX) {
      this.counters.droppedOps += 1;
      return;
    }
    for (const run of layer.runs) {
      if (run.replacedAt === null && (run.key === key || run.key === struck)) {
        run.replacedAt = at;
      }
    }
    if (layer.runs.length >= 2 * LAYER_ELEMENTS_MAX) {
      dropReplaced(layer.runs);
    }
    const plan = planOp(op, this.#tokens, target, round);
    let lead: Lead = { kind: "none" };
    let leadMs = 0;
    const first = plan.items[0];
    if (op.kind === "Trace" || first === undefined) {
      lead = { kind: "none" };
    } else if (layerId === 0) {
      const rest = decision.nibAtRest(at);
      if (rest === "out") {
        const previous = this.latestPenRun(layer, at);
        const last = previous?.plan.items[previous.plan.items.length - 1];
        if (last !== undefined) {
          const from = itemEnd(last, [0, 0]);
          lead = { kind: "travel", from };
          leadMs = travelMs(from, itemStart(first, [0, 0]));
        }
      } else {
        const { waitMs, flightMs } = decision.flightLead(rest, at);
        lead = { kind: "flight", from: rest, waitMs, flightMs };
        leadMs = waitMs + flightMs;
      }
    } else {
      const previous = this.latestPenRun(layer, at);
      const last = previous?.plan.items[previous.plan.items.length - 1];
      if (last !== undefined) {
        const from = itemEnd(last, [0, 0]);
        lead = { kind: "travel", from };
        leadMs = travelMs(from, itemStart(first, [0, 0]));
      }
    }
    const trace =
      op.kind === "Trace"
        ? { stroke: traceStroke(op.points), periodMs: Math.max(TRACE_PERIOD_MIN_MS, budgetMs) }
        : null;
    this.#serial += 1;
    layer.runs.push({
      layer: layerId,
      key,
      op,
      plan,
      clock: startClock(at, leadMs + plan.naturalMs, budgetMs),
      lead,
      leadMs,
      trace,
      replacedAt: null,
      stoppedAt: null,
      serial: this.#serial,
    });
    this.version += 1;
  }

  #freeze(layerId: number, at: number): void {
    const layer = this.layers.get(layerId);
    if (layer === undefined) {
      this.counters.orphanEvents += 1;
      return;
    }
    for (const run of layer.runs) {
      if (run.replacedAt !== null) {
        continue;
      }
      if (run.trace === null) {
        freezeClock(run.clock, at);
      } else if (run.stoppedAt === null) {
        run.stoppedAt = at;
      }
    }
  }

  #resume(layerId: number, at: number, budgetMs: number): void {
    const layer = this.layers.get(layerId);
    if (layer === undefined) {
      this.counters.orphanEvents += 1;
      return;
    }
    for (const run of layer.runs) {
      if (isFrozen(run.clock, at)) {
        resumeClock(run.clock, at, budgetMs);
      }
    }
  }

  #push(layerId: number, title: string, at: number): void {
    if (layerId === 0) {
      this.counters.orphanEvents += 1;
      return;
    }
    const wasOpen = this.openDetours().length > 0;
    this.layers.set(layerId, {
      id: layerId,
      title,
      pushedAt: at,
      poppedAt: null,
      runs: [],
      camera: NO_CAMERA,
    });
    if (!wasOpen) {
      this.detours.push({ openAt: at, closeAt: null });
      if (this.detours.length > DETOUR_SPANS_MAX) {
        this.detours.shift();
      }
    }
    this.version += 1;
  }

  #pop(layerId: number, at: number): void {
    const layer = this.layers.get(layerId);
    if (layer === undefined || layerId === 0 || layer.poppedAt !== null) {
      this.counters.orphanEvents += 1;
      return;
    }
    layer.poppedAt = at;
    const span = this.detours[this.detours.length - 1];
    if (span !== undefined && span.closeAt === null && this.openDetours().length === 0) {
      span.closeAt = at;
    }
    this.version += 1;
  }

  #camera(layerId: number, frame: Rect, focus: string | null, at: number): void {
    const layer = this.layers.get(layerId);
    if (layer === undefined) {
      this.counters.orphanEvents += 1;
      return;
    }
    const camera = layer.camera;
    layer.camera = {
      frame,
      previous: camera.frame,
      at,
      focus,
      previousFocus: camera.focus,
      focusAt: focus === camera.focus ? camera.focusAt : at,
    };
  }

  /** Drops layers that finished leaving long ago, so a long session doesn't accumulate them. */
  prune(at: number, keepMs: number): void {
    for (const [id, layer] of this.layers) {
      if (id !== 0 && layer.poppedAt !== null && at - layer.poppedAt > keepMs) {
        this.layers.delete(id);
        this.version += 1;
      }
    }
  }
}

/**
 * The label a strike on `target` crosses: a value written into it, else the element's own label,
 * else its label inside another drawing (a cell of a row).
 */
function struckLabel(runs: readonly Run[], target: string): StruckLabel | null {
  let own: Run | null = null;
  let inside: StruckLabel | null = null;
  for (const run of runs) {
    if (run.replacedAt !== null) {
      continue;
    }
    if (run.key === `${target}#write`) {
      return labelOf(run.plan);
    }
    if (run.key === target) {
      own = run;
    } else if (inside === null && run.plan.targets.length > 0) {
      inside = labelOfTarget(run.plan, target);
    }
  }
  return own === null ? inside : labelOf(own.plan);
}

/** Lets go of drawings a newer run replaced; the renderer has already removed them. */
function dropReplaced(runs: Run[]): void {
  let kept = 0;
  for (const run of runs) {
    if (run.replacedAt === null) {
      runs[kept] = run;
      kept += 1;
    }
  }
  runs.length = kept;
}

/** Whether a trace's dot is lapping at `at`. */
export function traceRunning(run: Run, layer: Layer, at: number): boolean {
  return (
    run.trace !== null &&
    at >= run.clock.startAt &&
    run.replacedAt === null &&
    (run.stoppedAt === null || at < run.stoppedAt) &&
    (layer.poppedAt === null || at < layer.poppedAt)
  );
}

/** Whether a run is still drawing (or frozen) at `at`. */
export function runActive(run: Run, at: number): boolean {
  return (
    run.replacedAt === null &&
    run.clock.startAt <= at &&
    (isFrozen(run.clock, at) || !isDone(run.clock, at))
  );
}
