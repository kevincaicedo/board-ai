// BoardAI's presence on screen (PRODUCT §8.6): where the orb sits and how big it is, and where the
// nib is. The signature moment: when drawing starts the orb condenses into a nib that flies onto
// the canvas and draws; when drawing is done the nib flies back and the docked orb swells.
// Ported from the design's orbAt and nibAt. Everything here is a pure function of session time.

import type { OrbState } from "../protocol/generated/OrbState.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { BoardModel, LeadDecision, Run } from "./board.ts";
import type { Vec } from "./geometry.ts";
import { clamp, easeInOutCubic, easeOut, lerp, ramp, spring } from "./math.ts";
import { itemEnd, itemStart, pointAtLength, revealLength } from "./stroke.ts";
import { endTime, isFrozen, naturalTime } from "./timeline.ts";

/** Orb state changes remembered; the oldest half is dropped past this (hours of conversation). */
export const ORB_TRANSITIONS_MAX = 4096;

export type PresenceTokens = Pick<
  Tokens,
  | "durationOrbCondenseMs"
  | "durationNibFlightMs"
  | "durationNibHomeMs"
  | "durationDockGrowMs"
  | "durationDockSwellMs"
  | "orbSize"
  | "orbDockSize"
  | "orbDockSwell"
  | "nibSize"
>;

export interface OrbTransition {
  readonly at: number;
  readonly state: OrbState;
  readonly level: number;
}

/** A world → screen mapping: uniform scale, then translation. */
export interface Transform {
  scale: number;
  x: number;
  y: number;
}

export interface Anchors {
  /** Screen position of the orb while conversing. */
  readonly center: Vec;
  /** Screen position of the docked orb while the canvas is active. */
  readonly dock: Vec;
}

export interface NibFrame {
  visible: boolean;
  x: number;
  y: number;
  scale: number;
  /** The pen is on the paper (drawing), not travelling. */
  down: boolean;
  frozen: boolean;
  frozenAt: number;
}

export interface OrbPlacement {
  x: number;
  y: number;
  size: number;
  opacity: number;
  /** 0 centered, 1 docked; the dot field only shows while centered. */
  docked: number;
  /** How far the condense has gone (0 to 1); the orb brightens as it becomes the nib. */
  condense: number;
}

const NIB_APPEARS_BEFORE_MS = 80;
const FLIGHT_ARC = 130;
const HOME_ARC = 40;
const TRAVEL_LIFT_MAX = 18;

export class Presence {
  readonly transitions: OrbTransition[] = [];
  condenseAt: number | null = null;
  readonly freezes: number[] = [];
  lastAcknowledgeAt: number | null = null;
  readonly #tokens: PresenceTokens;

  constructor(tokens: PresenceTokens) {
    this.#tokens = tokens;
  }

  get tokens(): PresenceTokens {
    return this.#tokens;
  }

  reset(): void {
    this.transitions.length = 0;
    this.condenseAt = null;
    this.freezes.length = 0;
    this.lastAcknowledgeAt = null;
  }

  orb(state: OrbState, level: number, at: number): void {
    // Levels can arrive every frame; only state changes matter to where the orb and nib are.
    if (this.transitions[this.transitions.length - 1]?.state === state) {
      return;
    }
    if (this.transitions.length >= ORB_TRANSITIONS_MAX) {
      this.transitions.splice(0, ORB_TRANSITIONS_MAX / 2);
    }
    this.transitions.push({ at, state, level });
    if (state === "Drawing" && this.condenseAt === null) {
      this.condenseAt = at;
    }
  }

  /** The board started drawing; if the orb never condensed, it does so now. */
  drawingStarted(at: number): void {
    if (this.condenseAt === null) {
      this.condenseAt = at;
    }
  }

  freeze(at: number): void {
    this.freezes.push(at);
    if (this.freezes.length > ORB_TRANSITIONS_MAX) {
      this.freezes.shift();
    }
  }

  acknowledge(at: number): void {
    this.lastAcknowledgeAt = at;
  }

  /** The transition in force at `at`, or null before the first one. */
  stateAt(at: number): OrbTransition | null {
    for (let index = this.transitions.length - 1; index >= 0; index -= 1) {
      const transition = this.transitions[index];
      if (transition !== undefined && transition.at <= at) {
        return transition;
      }
    }
    return null;
  }

  /** When the orb last stopped drawing, if it is not drawing at `at`. */
  notDrawingSince(at: number): number | null {
    let since: number | null = null;
    for (let index = this.transitions.length - 1; index >= 0; index -= 1) {
      const transition = this.transitions[index];
      if (transition === undefined || transition.at > at) {
        continue;
      }
      if (transition.state === "Drawing") {
        return since;
      }
      since = transition.at;
    }
    return since;
  }

  lastFreezeAt(at: number): number | null {
    for (let index = this.freezes.length - 1; index >= 0; index -= 1) {
      const freeze = this.freezes[index];
      if (freeze !== undefined && freeze <= at) {
        return freeze;
      }
    }
    return null;
  }

  condenseEnd(): number | null {
    return this.condenseAt === null ? null : this.condenseAt + this.#tokens.durationOrbCondenseMs;
  }
}

/** When the nib started flying home, if it is home (or on its way) at `at`. */
export function homeStart(board: BoardModel, presence: Presence, at: number): number | null {
  const condenseEnd = presence.condenseEnd();
  if (condenseEnd === null || at < condenseEnd) {
    return null;
  }
  const since = presence.notDrawingSince(at);
  if (since === null) {
    return null;
  }
  const run = board.latestPenRun(board.main, at);
  let begin = Math.max(since, condenseEnd);
  if (run !== null) {
    if (isFrozen(run.clock, at)) {
      return null;
    }
    const end = endTime(run.clock);
    if (end === null) {
      return null;
    }
    begin = Math.max(begin, end);
  }
  return at >= begin ? begin : null;
}

export function leadDecision(board: BoardModel, presence: Presence): LeadDecision {
  const tokens = presence.tokens;
  return {
    nibAtRest(at) {
      const condenseEnd = presence.condenseEnd();
      if (condenseEnd === null) {
        return "center";
      }
      if (homeStart(board, presence, at) !== null) {
        return "dock";
      }
      return board.latestPenRun(board.main, at) === null ? "center" : "out";
    },
    flightLead(from, at) {
      const condenseEnd = presence.condenseEnd() ?? at;
      const waitMs = from === "center" ? Math.max(0, condenseEnd - at) : 0;
      return { waitMs, flightMs: tokens.durationNibFlightMs };
    },
  };
}

const scratchA: Vec = [0, 0];
const scratchB: Vec = [0, 0];

function project(transform: Transform, world: Vec, out: Vec): Vec {
  out[0] = world[0] * transform.scale + transform.x;
  out[1] = world[1] * transform.scale + transform.y;
  return out;
}

function quadraticInto(out: NibFrame, from: Vec, cx: number, cy: number, to: Vec, f: number): void {
  const a = (1 - f) * (1 - f);
  const b = 2 * (1 - f) * f;
  const c = f * f;
  out.x = a * from[0] + b * cx + c * to[0];
  out.y = a * from[1] + b * cy + c * to[1];
}

/** The pen's resting point after a run: the end of its last item. */
function runEnd(run: Run, transform: Transform, out: Vec): Vec {
  const last = run.plan.items[run.plan.items.length - 1];
  if (last === undefined) {
    return out;
  }
  return project(transform, itemEnd(last, scratchA), out);
}

function penOnRun(
  run: Run,
  natural: number,
  transform: Transform,
  anchors: Anchors,
  out: NibFrame,
): void {
  const items = run.plan.items;
  const first = items[0];
  if (first === undefined) {
    out.visible = false;
    return;
  }
  if (natural < run.leadMs) {
    const to = project(transform, itemStart(first, scratchA), scratchB);
    const lead = run.lead;
    if (lead.kind === "flight") {
      const from = lead.from === "center" ? anchors.center : anchors.dock;
      const g = easeOut(clamp((natural - lead.waitMs) / (lead.flightMs || 1)));
      quadraticInto(out, from, (from[0] + to[0]) / 2, Math.min(from[1], to[1]) - FLIGHT_ARC, to, g);
      out.scale = 0.8 + 0.2 * g;
    } else if (lead.kind === "travel") {
      const fromX = lead.from[0] * transform.scale + transform.x;
      const fromY = lead.from[1] * transform.scale + transform.y;
      const g = easeOut(clamp(natural / (run.leadMs || 1)));
      const lift =
        Math.sin(g * Math.PI) *
        Math.min(TRAVEL_LIFT_MAX, Math.hypot(to[0] - fromX, to[1] - fromY) * 0.12);
      out.x = lerp(fromX, to[0], g);
      out.y = lerp(fromY, to[1], g) - lift;
      out.scale = 0.86;
    } else {
      out.x = to[0];
      out.y = to[1];
      out.scale = 0.86;
    }
    out.down = false;
    return;
  }
  const local = natural - run.leadMs;
  const { starts, ends } = run.plan;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    const start = starts[index] ?? 0;
    const end = ends[index] ?? 0;
    if (item === undefined || local > end) {
      continue;
    }
    if (local >= start) {
      const f = clamp((local - start) / (end - start || 1));
      if (item.type === "text") {
        const letters = Math.min(item.count, f * item.count);
        scratchA[0] = item.start[0] + letters * item.letterWidth;
        scratchA[1] = item.y + 3 + 2.5 * Math.sin(letters * 2.3);
      } else {
        pointAtLength(item, revealLength(item, f), scratchA);
      }
      project(transform, scratchA, scratchB);
      out.x = scratchB[0];
      out.y = scratchB[1];
      out.scale = 1;
      out.down = true;
      return;
    }
    const previous = items[index - 1];
    if (previous === undefined) {
      break;
    }
    const gapStart = ends[index - 1] ?? 0;
    const g = easeOut(clamp((local - gapStart) / (start - gapStart || 1)));
    const from = project(transform, itemEnd(previous, scratchA), scratchA);
    const to = project(transform, itemStart(item, scratchB), scratchB);
    const lift =
      Math.sin(g * Math.PI) *
      Math.min(TRAVEL_LIFT_MAX, Math.hypot(to[0] - from[0], to[1] - from[1]) * 0.12);
    out.x = lerp(from[0], to[0], g);
    out.y = lerp(from[1], to[1], g) - lift;
    out.scale = 0.86;
    out.down = false;
    return;
  }
  runEnd(run, transform, scratchB);
  out.x = scratchB[0];
  out.y = scratchB[1];
  out.scale = 0.86;
  out.down = false;
}

/** Where the nib is at `at`, written into `out` (no allocation per frame). */
export function nibAt(
  board: BoardModel,
  presence: Presence,
  at: number,
  transform: Transform,
  anchors: Anchors,
  out: NibFrame,
): NibFrame {
  out.visible = false;
  out.frozen = false;
  out.down = false;
  out.scale = 1;
  const condenseEnd = presence.condenseEnd();
  if (condenseEnd === null || at < condenseEnd - NIB_APPEARS_BEFORE_MS) {
    return out;
  }
  const tokens = presence.tokens;
  const run = board.latestPenRun(board.main, at);
  const home = homeStart(board, presence, at);
  if (home !== null) {
    const arrive = home + tokens.durationNibHomeMs;
    if (at >= arrive) {
      return out;
    }
    const rest = run === null ? anchors.center : runEnd(run, transform, scratchB);
    const restX = rest[0];
    const restY = rest[1];
    scratchA[0] = restX;
    scratchA[1] = restY;
    const h = easeOut(ramp(at, home, arrive));
    quadraticInto(
      out,
      scratchA,
      (restX + anchors.dock[0]) / 2,
      Math.max(restY, anchors.dock[1]) - HOME_ARC,
      anchors.dock,
      h,
    );
    out.scale = 1 - 0.45 * ramp(at, arrive - 250, arrive);
    out.visible = true;
    return out;
  }
  out.visible = true;
  if (run === null) {
    out.x = anchors.center[0];
    out.y = anchors.center[1];
    out.scale = 0.8;
    return out;
  }
  penOnRun(run, naturalTime(run.clock, at), transform, anchors, out);
  if (isFrozen(run.clock, at)) {
    out.frozen = true;
    out.frozenAt = presence.lastFreezeAt(at) ?? at;
  }
  return out;
}

/** How swollen the docked orb is: 0 while the nib is out, 1 once it is home. */
export function dockSwell(board: BoardModel, presence: Presence, at: number): number {
  const tokens = presence.tokens;
  const home = homeStart(board, presence, at);
  if (home !== null) {
    const begin = home + tokens.durationNibHomeMs - 50;
    return spring(ramp(at, begin, begin + tokens.durationDockSwellMs));
  }
  const run = board.latestPenRun(board.main, at);
  if (run !== null && run.lead.kind === "flight" && run.lead.from === "dock") {
    return 1 - spring(ramp(at, run.clock.startAt, run.clock.startAt + tokens.durationDockSwellMs));
  }
  return 0;
}

export function orbPlacement(
  board: BoardModel,
  presence: Presence,
  at: number,
  anchors: Anchors,
  out: OrbPlacement,
): OrbPlacement {
  const tokens = presence.tokens;
  const condenseAt = presence.condenseAt;
  out.docked = 0;
  out.condense = 0;
  if (condenseAt === null || at < condenseAt) {
    out.x = anchors.center[0];
    out.y = anchors.center[1];
    out.size = tokens.orbSize;
    out.opacity = 1;
    return out;
  }
  const condenseEnd = condenseAt + tokens.durationOrbCondenseMs;
  if (at < condenseEnd) {
    const c = easeInOutCubic(ramp(at, condenseAt, condenseEnd));
    out.x = anchors.center[0];
    out.y = anchors.center[1];
    out.size = lerp(tokens.orbSize, tokens.nibSize, c);
    out.opacity = 1 - ramp(at, condenseEnd - 100, condenseEnd);
    out.condense = c;
    return out;
  }
  const grow = spring(ramp(at, condenseEnd, condenseEnd + tokens.durationDockGrowMs));
  const swell = dockSwell(board, presence, at);
  out.x = anchors.dock[0];
  out.y = anchors.dock[1];
  out.size = Math.max(0.1, tokens.orbDockSize * grow + tokens.orbDockSwell * swell);
  out.opacity = Math.min(1, grow * 1.4);
  out.docked = 1;
  out.condense = 0.25 * (1 - Math.min(1, grow));
  return out;
}
