// The orb's living parameters, ported from the design's orb component (Orb.dc.html): each state
// has targets, and the orb eases toward them. The design eased per frame (k = 1 - e^(-9 dt)); here
// the same exponential is evaluated in closed form from the last state change, so a replay seek
// lands on exactly the frame a live session showed, whatever the frame rate.

import { bump, clamp, easeOut, ramp } from "../canvas/math.ts";
import type { OrbState } from "../protocol/generated/OrbState.ts";

/** Per second: how quickly parameters approach a new state's targets (the design's 9). */
const EASE_RATE = 9;
/** Per second: voice level attack and release, close to the design's 80 ms / 120 ms ramps. */
const LEVEL_RATE = 20;

const KEYS = [
  "swell",
  "breath",
  "swirl",
  "field",
  "ripple",
  "pulse",
  "cool",
  "mute",
  "motion",
  "lean",
  "bright",
] as const;
type Key = (typeof KEYS)[number];
type Targets = { readonly [K in Key]: number };

/**
 * State targets from the design's orb component. Drawing pulses with the voice here because in
 * BoardAI the orb draws while it speaks; the standalone component drew in silence.
 */
const TARGETS: { readonly [S in OrbState]: Targets } = {
  Idle: {
    swell: 1,
    breath: 1,
    swirl: 0,
    field: 0,
    ripple: 0,
    pulse: 0,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0,
  },
  Listening: {
    swell: 1.08,
    breath: 0.3,
    swirl: 0.1,
    field: 1,
    ripple: 1,
    pulse: 0,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0,
  },
  Thinking: {
    swell: 0.97,
    breath: 0,
    swirl: 1,
    field: 0,
    ripple: 0,
    pulse: 0,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0,
  },
  Speaking: {
    swell: 1,
    breath: 0.3,
    swirl: 0.12,
    field: 0,
    ripple: 0,
    pulse: 1,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0,
  },
  Drawing: {
    swell: 0.94,
    breath: 0.6,
    swirl: 0.25,
    field: 0,
    ripple: 0,
    pulse: 1,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0.2,
  },
  Muted: {
    swell: 0.95,
    breath: 0,
    swirl: 0,
    field: 0,
    ripple: 0,
    pulse: 0,
    cool: 0,
    mute: 1,
    motion: 0,
    lean: 0,
    bright: 0,
  },
};

export interface OrbParams {
  swell: number;
  breath: number;
  swirl: number;
  field: number;
  ripple: number;
  pulse: number;
  cool: number;
  mute: number;
  motion: number;
  lean: number;
  bright: number;
  /** Voice energy, 0 to 1, with the voice's texture on top of the level. */
  amp: number;
  /** Swirl rotation, integrated over time. */
  phase: number;
  /** The orb's own clock in seconds: it slows to a crawl when muted. */
  clock: number;
}

export function emptyParams(): OrbParams {
  return {
    swell: 1,
    breath: 1,
    swirl: 0,
    field: 0,
    ripple: 0,
    pulse: 0,
    cool: 0,
    mute: 0,
    motion: 1,
    lean: 0,
    bright: 0,
    amp: 0,
    phase: 0,
    clock: 0,
  };
}

/** The voice's texture from the design (Speech.voice): energy that flickers like syllables. */
export function voiceTexture(seconds: number): number {
  return (
    0.45 +
    0.35 * Math.abs(Math.sin(seconds * 7.3) * Math.sin(seconds * 2.9 + 0.7)) +
    0.2 * Math.abs(Math.sin(seconds * 17.1 + 1.3))
  );
}

export class OrbDynamics {
  readonly #from = new Float64Array(KEYS.length);
  readonly #to = new Float64Array(KEYS.length);
  #since = 0;
  #phaseAtSince = 0;
  #clockAtSince = 0;
  #levelFrom = 0;
  #levelTo = 0;
  #levelSince = 0;
  #state: OrbState = "Idle";

  constructor() {
    this.reset();
  }

  get state(): OrbState {
    return this.#state;
  }

  reset(): void {
    const idle = TARGETS.Idle;
    KEYS.forEach((key, index) => {
      this.#from[index] = idle[key];
      this.#to[index] = idle[key];
    });
    this.#since = 0;
    this.#phaseAtSince = 0;
    this.#clockAtSince = 0;
    this.#levelFrom = 0;
    this.#levelTo = 0;
    this.#levelSince = 0;
    this.#state = "Idle";
  }

  /** A new state (and level) at session time `at`. Calls must come in time order. */
  set(state: OrbState, levelPercent: number, at: number): void {
    const level = clamp(levelPercent / 100);
    this.#levelFrom = this.#level(at);
    this.#levelTo = level;
    this.#levelSince = at;
    if (state === this.#state) {
      return;
    }
    const seconds = Math.max(0, (at - this.#since) / 1000);
    this.#phaseAtSince += this.#phaseIntegral(seconds);
    this.#clockAtSince += this.#clockIntegral(seconds);
    const decay = Math.exp(-EASE_RATE * seconds);
    const targets = TARGETS[state];
    KEYS.forEach((key, index) => {
      const from = this.#from[index] ?? 0;
      const to = this.#to[index] ?? 0;
      this.#from[index] = to + (from - to) * decay;
      this.#to[index] = targets[key];
    });
    this.#since = at;
    this.#state = state;
  }

  #value(index: number, decay: number): number {
    const to = this.#to[index] ?? 0;
    return to + ((this.#from[index] ?? 0) - to) * decay;
  }

  #level(at: number): number {
    const seconds = Math.max(0, (at - this.#levelSince) / 1000);
    return this.#levelTo + (this.#levelFrom - this.#levelTo) * Math.exp(-LEVEL_RATE * seconds);
  }

  /** ∫ (0.35 + 2.4·swirl)·motion dt over `seconds` since the last change, in closed form. */
  #phaseIntegral(seconds: number): number {
    const swirlIndex = KEYS.indexOf("swirl");
    const motionIndex = KEYS.indexOf("motion");
    const s1 = this.#to[swirlIndex] ?? 0;
    const ds = (this.#from[swirlIndex] ?? 0) - s1;
    const m1 = this.#to[motionIndex] ?? 0;
    const dm = (this.#from[motionIndex] ?? 0) - m1;
    const k = EASE_RATE;
    const a = 0.35 * m1 + 2.4 * s1 * m1;
    const b = 0.35 * dm + 2.4 * (s1 * dm + m1 * ds);
    const c = 2.4 * ds * dm;
    return (
      a * seconds +
      (b * (1 - Math.exp(-k * seconds))) / k +
      (c * (1 - Math.exp(-2 * k * seconds))) / (2 * k)
    );
  }

  /** ∫ (0.15 + 0.85·motion) dt: the orb's clock nearly stops when muted. */
  #clockIntegral(seconds: number): number {
    const motionIndex = KEYS.indexOf("motion");
    const m1 = this.#to[motionIndex] ?? 0;
    const dm = (this.#from[motionIndex] ?? 0) - m1;
    return (
      0.15 * seconds +
      0.85 * (m1 * seconds + (dm * (1 - Math.exp(-EASE_RATE * seconds))) / EASE_RATE)
    );
  }

  /**
   * Parameters at session time `at`, written into `out`. `freezeAt` adds the interruption
   * (a 120 ms contraction that cools and leans), `acknowledgeAt` a small backchannel pulse.
   */
  sample(
    at: number,
    freezeAt: number | null,
    acknowledgeAt: number | null,
    out: OrbParams,
  ): OrbParams {
    const seconds = Math.max(0, (at - this.#since) / 1000);
    const decay = Math.exp(-EASE_RATE * seconds);
    out.swell = this.#value(0, decay);
    out.breath = this.#value(1, decay);
    out.swirl = this.#value(2, decay);
    out.field = this.#value(3, decay);
    out.ripple = this.#value(4, decay);
    out.pulse = this.#value(5, decay);
    out.cool = this.#value(6, decay);
    out.mute = this.#value(7, decay);
    out.motion = this.#value(8, decay);
    out.lean = this.#value(9, decay);
    out.bright = this.#value(10, decay);
    out.phase = this.#phaseAtSince + this.#phaseIntegral(seconds);
    out.clock = this.#clockAtSince + this.#clockIntegral(seconds);
    out.amp = clamp(this.#level(at) * voiceTexture(at / 1000));
    if (freezeAt !== null) {
      const contract =
        ramp(at, freezeAt, freezeAt + 60) * (1 - easeOut(ramp(at, freezeAt + 120, freezeAt + 420)));
      const cool =
        ramp(at, freezeAt, freezeAt + 120) * (1 - ramp(at, freezeAt + 1300, freezeAt + 2100));
      out.swell *= 1 - 0.3 * contract;
      out.cool = Math.max(out.cool, cool);
      out.lean += 3 * cool;
    }
    if (acknowledgeAt !== null) {
      const pulse = bump(
        at,
        acknowledgeAt,
        acknowledgeAt + 90,
        acknowledgeAt + 200,
        acknowledgeAt + 520,
      );
      out.swell *= 1 + 0.06 * pulse;
      out.bright += 0.15 * pulse;
    }
    return out;
  }
}
