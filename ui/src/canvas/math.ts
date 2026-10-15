// Scalar helpers and easing curves, ported from the design canvas (Main.dc.html). Every animated
// value in the UI is a pure function of time built from these, so a replay seek is exact.

export function clamp(value: number, low = 0, high = 1): number {
  if (value < low) {
    return low;
  }
  return value > high ? high : value;
}

export function lerp(from: number, to: number, fraction: number): number {
  return from + (to - from) * fraction;
}

/** 0 before `begin`, 1 after `end`, linear between; a zero-length ramp is a step at `begin`. */
export function ramp(time: number, begin: number, end: number): number {
  if (end === begin) {
    return time >= begin ? 1 : 0;
  }
  return clamp((time - begin) / (end - begin));
}

/** Rises over [a, b], holds, then falls over [c, d]. */
export function bump(time: number, a: number, b: number, c: number, d: number): number {
  return ramp(time, a, b) * (1 - ramp(time, c, d));
}

/** A CSS-style cubic Bézier timing function, solved with Newton's method (8 steps at most). */
export function cubicBezier(x1: number, y1: number, x2: number, y2: number): (x: number) => number {
  const cx = 3 * x1;
  const bx = 3 * (x2 - x1) - cx;
  const ax = 1 - cx - bx;
  const cy = 3 * y1;
  const by = 3 * (y2 - y1) - cy;
  const ay = 1 - cy - by;
  return (x: number): number => {
    if (x <= 0) {
      return 0;
    }
    if (x >= 1) {
      return 1;
    }
    let u = x;
    for (let step = 0; step < 8; step += 1) {
      const error = ((ax * u + bx) * u + cx) * u - x;
      const slope = (3 * ax * u + 2 * bx) * u + cx;
      if (Math.abs(error) < 1e-5 || Math.abs(slope) < 1e-6) {
        break;
      }
      u -= error / slope;
    }
    const solved = clamp(u);
    return ((ay * solved + by) * solved + cy) * solved;
  };
}

/** The house ease-out, cubic-bezier(.22, 1, .36, 1) (PRODUCT §8.5). */
export const easeOut = cubicBezier(0.22, 1, 0.36, 1);

export function easeInOutCubic(fraction: number): number {
  const f = clamp(fraction);
  return f < 0.5 ? 4 * f * f * f : 1 - (-2 * f + 2) ** 3 / 2;
}

/** A damped spring from 0 to 1 that overshoots once; surfaces and the orb move on it. */
export function spring(fraction: number): number {
  const f = clamp(fraction);
  if (f >= 1) {
    return 1;
  }
  return 1 - Math.exp(-6.5 * f) * Math.cos(10 * f);
}

/** One decimal, the precision of every SVG coordinate the canvas writes. */
export function round1(value: number): number {
  return Math.round(value * 10) / 10;
}

export function round2(value: number): number {
  return Math.round(value * 100) / 100;
}
