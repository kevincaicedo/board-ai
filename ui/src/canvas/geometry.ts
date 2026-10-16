// Rough, hand-drawn geometry ported from the design prototype (Main.dc.html: quad, rline, rcurve,
// rellipse, head, hatchPts, rectPoly, cylPoly). Generators run once per op, never per frame, so
// they may allocate. Every random draw comes from the caller's seeded generator.

import type { Random } from "./rng.ts";

export type Vec = [number, number];

/** Points along a quadratic Bézier, `segments` + 1 of them. */
export function quadPoints(
  x0: number,
  y0: number,
  cx: number,
  cy: number,
  x1: number,
  y1: number,
  segments: number,
): Vec[] {
  const points: Vec[] = [];
  for (let index = 0; index <= segments; index += 1) {
    const u = index / segments;
    const a = (1 - u) * (1 - u);
    const b = 2 * (1 - u) * u;
    const c = u * u;
    points.push([a * x0 + b * cx + c * x1, a * y0 + b * cy + c * y1]);
  }
  return points;
}

export interface LineOptions {
  /** How far the pen runs past each end; defaults to 7% of the length, at most 7. */
  readonly overshoot?: number;
  /** Extra sideways bow on top of the random one. */
  readonly bow?: number;
}

/** A straight stroke that starts a little early, overshoots, wobbles and bows like a hand. */
export function roughLine(
  random: Random,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  options: LineOptions = {},
): Vec[] {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const length = Math.hypot(dx, dy) || 1;
  const ux = dx / length;
  const uy = dy / length;
  const nx = -uy;
  const ny = ux;
  const overshoot = options.overshoot ?? Math.min(7, length * 0.07);
  const lead = (random() * 0.7 - 0.25) * overshoot;
  const tail = (random() * 0.8 + 0.15) * overshoot;
  const jitter = (): number => (random() - 0.5) * 1.4;
  const sx = x1 - ux * lead + nx * jitter();
  const sy = y1 - uy * lead + ny * jitter();
  const ex = x2 + ux * tail + nx * jitter();
  const ey = y2 + uy * tail + ny * jitter();
  const bow = (random() - 0.5) * Math.min(length * 0.04, 5) + (options.bow ?? 0);
  return quadPoints(
    sx,
    sy,
    (sx + ex) / 2 + nx * bow,
    (sy + ey) / 2 + ny * bow,
    ex,
    ey,
    Math.max(4, Math.round(length / 5)),
  );
}

/** A quadratic connector with jittered ends. */
export function roughCurve(
  random: Random,
  x1: number,
  y1: number,
  cx: number,
  cy: number,
  x2: number,
  y2: number,
): Vec[] {
  const jitter = (): number => (random() - 0.5) * 1.2;
  return quadPoints(
    x1 + jitter(),
    y1 + jitter(),
    cx,
    cy,
    x2 + jitter(),
    y2 + jitter(),
    Math.max(10, Math.round(Math.hypot(x2 - x1, y2 - y1) / 5)),
  );
}

export interface EllipseOptions {
  /** Start angle in radians; random near the upper left by default. */
  readonly start?: number;
  /** Swept angle; a little over a full turn by default, so the ends overlap like a hand's. */
  readonly span?: number;
  /** Radial growth across the sweep, so the closing overlap doesn't retrace the start. */
  readonly grow?: number;
}

export function roughEllipse(
  random: Random,
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  options: EllipseOptions = {},
): Vec[] {
  const start = options.start ?? -Math.PI * 0.72 + (random() - 0.5) * 0.5;
  const span = options.span ?? Math.PI * 2 + 0.3 + random() * 0.2;
  const phase1 = random() * 6;
  const phase2 = random() * 6;
  const grow = options.grow ?? 0.035;
  const count = Math.max(28, Math.round(((rx + ry) * 0.7 * Math.abs(span)) / 6.28));
  const points: Vec[] = [];
  for (let index = 0; index <= count; index += 1) {
    const f = index / count;
    const angle = start + span * f;
    const k =
      1 +
      0.02 * Math.sin(3 * angle + phase1) +
      0.012 * Math.sin(5 * angle + phase2) +
      grow * (f - 0.5);
    points.push([cx + Math.cos(angle) * rx * k, cy + Math.sin(angle) * ry * k]);
  }
  return points;
}

/** An open arrowhead at (tx, ty) pointing along `angle`: barb, tip, barb, drawn in one stroke. */
export function arrowHead(random: Random, tx: number, ty: number, angle: number, size = 13): Vec[] {
  const a1 = angle + Math.PI - 0.46 + (random() - 0.5) * 0.12;
  const a2 = angle + Math.PI + 0.46 + (random() - 0.5) * 0.12;
  const b1: Vec = [tx + Math.cos(a1) * size, ty + Math.sin(a1) * size];
  const b2: Vec = [tx + Math.cos(a2) * size * 0.95, ty + Math.sin(a2) * size * 0.95];
  const tip: Vec = [tx + (random() - 0.5), ty + (random() - 0.5)];
  const middle1: Vec = [
    b1[0] + (tip[0] - b1[0]) * 0.5 + (random() - 0.5),
    b1[1] + (tip[1] - b1[1]) * 0.5 + (random() - 0.5),
  ];
  const middle2: Vec = [tip[0] + (b2[0] - tip[0]) * 0.5, tip[1] + (b2[1] - tip[1]) * 0.5];
  return [b1, middle1, tip, middle2, b2];
}

/**
 * Hachure: parallel lines at `degrees` across the polygon, `gap` apart, joined into one zigzag so
 * the whole fill is a single stroke (one SVG path per fill, LLD §4).
 */
export function hachure(
  random: Random,
  polygon: readonly Vec[],
  gap: number,
  degrees: number,
): Vec[] {
  const angle = (degrees * Math.PI) / 180;
  const dx = Math.cos(angle);
  const dy = Math.sin(angle);
  const nx = -dy;
  const ny = dx;
  let low = Number.POSITIVE_INFINITY;
  let high = Number.NEGATIVE_INFINITY;
  for (const [x, y] of polygon) {
    const distance = x * nx + y * ny;
    low = Math.min(low, distance);
    high = Math.max(high, distance);
  }
  const points: Vec[] = [];
  let flip = false;
  for (let offset = low + gap * 0.55; offset < high - gap * 0.3; offset += gap) {
    const hits: Vec[] = [];
    for (let index = 0; index < polygon.length; index += 1) {
      const p = polygon[index];
      const q = polygon[(index + 1) % polygon.length];
      if (p === undefined || q === undefined) {
        continue;
      }
      const dp = p[0] * nx + p[1] * ny - offset;
      const dq = q[0] * nx + q[1] * ny - offset;
      if ((dp <= 0 && dq > 0) || (dp > 0 && dq <= 0)) {
        const f = dp / (dp - dq);
        hits.push([p[0] + (q[0] - p[0]) * f, p[1] + (q[1] - p[1]) * f]);
      }
    }
    if (hits.length < 2) {
      continue;
    }
    hits.sort((u, v) => u[0] * dx + u[1] * dy - (v[0] * dx + v[1] * dy));
    const first = hits[0];
    const last = hits[hits.length - 1];
    if (first === undefined || last === undefined) {
      continue;
    }
    const [a, b] = flip ? [last, first] : [first, last];
    flip = !flip;
    points.push([a[0] + (random() - 0.5) * 1.6, a[1] + (random() - 0.5) * 1.6]);
    points.push([b[0] + (random() - 0.5) * 1.6, b[1] + (random() - 0.5) * 1.6]);
  }
  return points;
}

export function rectPolygon(x: number, y: number, w: number, h: number, inset: number): Vec[] {
  return [
    [x + inset, y + inset],
    [x + w - inset, y + inset],
    [x + w - inset, y + h - inset],
    [x + inset, y + h - inset],
  ];
}

/** The outline of a cylinder's body: the back half of the top ellipse and the front of the base. */
export function cylinderPolygon(
  cx: number,
  top: number,
  bottom: number,
  rx: number,
  ry: number,
  inset: number,
): Vec[] {
  const points: Vec[] = [];
  for (let index = 0; index <= 12; index += 1) {
    const angle = Math.PI + (Math.PI * index) / 12;
    points.push([cx + Math.cos(angle) * (rx - inset), top + Math.sin(angle) * (ry - inset * 0.4)]);
  }
  for (let index = 0; index <= 12; index += 1) {
    const angle = (Math.PI * index) / 12;
    points.push([
      cx + Math.cos(angle) * (rx - inset),
      bottom + Math.sin(angle) * (ry - inset * 0.4),
    ]);
  }
  return points;
}

/** An ellipse's outline as a polygon, `inset` inside it, for a circle node's hachure. */
export function ellipsePolygon(
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  inset: number,
): Vec[] {
  const points: Vec[] = [];
  for (let index = 0; index < 24; index += 1) {
    const angle = (Math.PI * 2 * index) / 24;
    points.push([cx + Math.cos(angle) * (rx - inset), cy + Math.sin(angle) * (ry - inset)]);
  }
  return points;
}
