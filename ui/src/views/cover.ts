// Cover art for a session card (PRODUCT §7.6: "a blurred pastel render of the session's
// diagram"). The session's nodes become soft rounded shapes in the canvas's sketch tones, scaled
// to fill the card and blurred as if far out of focus, over a porcelain ground. Deterministic per
// session id, like every stroke (LLD §4), so a card looks the same every time.

import { randomFor } from "../canvas/rng.ts";
import type { CoverShape } from "../protocol/generated/CoverShape.ts";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";

/** Shapes drawn per cover; the rest add nothing at this blur. */
export const COVER_BLOBS_MAX = 24;

export type CoverTone = "sky" | "lilac" | "mint" | "blush" | "porcelain";

export interface CoverBlob {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  readonly r: number;
  readonly tone: CoverTone;
}

export interface CoverLayout {
  readonly blobs: readonly CoverBlob[];
  /** Where the soft light falls from, in card units. */
  readonly light: { readonly x: number; readonly y: number };
}

/** Service tones in the order a board takes them (D-031). */
const SERVICE_TONES: readonly CoverTone[] = ["sky", "lilac", "mint", "blush"];

function toneOf(shape: CoverShape["shape"], serviceIndex: number): CoverTone {
  switch (shape) {
    case "Db":
      return "sky";
    case "Queue":
      return "mint";
    case "Group":
      return "porcelain";
    case "Actor":
      return "blush";
    case "Note":
    case "Circle":
      return "lilac";
    case "Service":
      return SERVICE_TONES[serviceIndex % SERVICE_TONES.length] ?? "sky";
  }
}

/** Groups first, so their large pale shapes sit under the nodes they hold. */
function order(shape: CoverShape["shape"]): number {
  return shape === "Group" ? 0 : 1;
}

export function coverLayout(
  shapes: readonly CoverShape[],
  seed: string,
  width: number,
  height: number,
): CoverLayout {
  const random = randomFor(`cover:${seed}`);
  const light = { x: width * (0.15 + 0.3 * random()), y: height * (0.05 + 0.2 * random()) };
  const kept = shapes
    .filter((shape) => shape.rect.w > 0 && shape.rect.h > 0)
    .slice(0, COVER_BLOBS_MAX);
  if (kept.length === 0 || width <= 0 || height <= 0) {
    return { blobs: [], light };
  }
  let left = Number.POSITIVE_INFINITY;
  let top = Number.POSITIVE_INFINITY;
  let right = Number.NEGATIVE_INFINITY;
  let bottom = Number.NEGATIVE_INFINITY;
  for (const { rect } of kept) {
    left = Math.min(left, rect.x);
    top = Math.min(top, rect.y);
    right = Math.max(right, rect.x + rect.w);
    bottom = Math.max(bottom, rect.y + rect.h);
  }
  // The whole diagram, a little larger than fits, as if photographed close: nudged off center
  // so no two covers line up the same.
  const zoom = 1.1 + 0.25 * random();
  const scale = Math.min(width / (right - left), height / (bottom - top)) * zoom;
  const offsetX = (width - (right - left) * scale) / 2 + (random() - 0.5) * width * 0.12;
  const offsetY = (height - (bottom - top) * scale) / 2 + (random() - 0.5) * height * 0.12;
  let services = 0;
  const blobs = kept
    .map((shape, index) => ({ shape, index }))
    .sort((a, b) => order(a.shape.shape) - order(b.shape.shape) || a.index - b.index)
    .map(({ shape }) => {
      const tone = toneOf(shape.shape, services);
      if (shape.shape === "Service") {
        services += 1;
      }
      const w = shape.rect.w * scale;
      const h = shape.rect.h * scale;
      return {
        x: Math.round((offsetX + (shape.rect.x - left) * scale) * 10) / 10,
        y: Math.round((offsetY + (shape.rect.y - top) * scale) * 10) / 10,
        w: Math.round(w * 10) / 10,
        h: Math.round(h * 10) / 10,
        r: Math.round(Math.min(w, h) * 0.42 * 10) / 10,
        tone,
      };
    });
  return { blobs, light };
}

/** Cover shapes from a past board's ops, for a session sent without its own cover. */
export function coverFromOps(ops: readonly DrawOp[]): CoverShape[] {
  const shapes: CoverShape[] = [];
  for (const op of ops) {
    if (op.kind === "Node" && shapes.length < COVER_BLOBS_MAX) {
      shapes.push({ rect: op.rect, shape: op.shape });
    }
  }
  return shapes;
}

export interface SketchBox {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  readonly tone: CoverTone;
  readonly round: boolean;
}

/**
 * A small line thumbnail of a board (the digest card's): every node fitted inside the box with a
 * margin, keeping its proportions, outlined in ink with its tone behind.
 */
export function sketchLayout(
  shapes: readonly CoverShape[],
  width: number,
  height: number,
  margin: number,
): SketchBox[] {
  const kept = shapes.filter((shape) => shape.rect.w > 0 && shape.rect.h > 0);
  if (kept.length === 0) {
    return [];
  }
  const left = Math.min(...kept.map((shape) => shape.rect.x));
  const top = Math.min(...kept.map((shape) => shape.rect.y));
  const right = Math.max(...kept.map((shape) => shape.rect.x + shape.rect.w));
  const bottom = Math.max(...kept.map((shape) => shape.rect.y + shape.rect.h));
  const scale = Math.min(
    (width - 2 * margin) / Math.max(1, right - left),
    (height - 2 * margin) / Math.max(1, bottom - top),
  );
  const offsetX = (width - (right - left) * scale) / 2;
  const offsetY = (height - (bottom - top) * scale) / 2;
  let services = 0;
  return kept.slice(0, COVER_BLOBS_MAX).map((shape) => {
    const tone = toneOf(shape.shape, services);
    if (shape.shape === "Service") {
      services += 1;
    }
    return {
      x: Math.round((offsetX + (shape.rect.x - left) * scale) * 10) / 10,
      y: Math.round((offsetY + (shape.rect.y - top) * scale) * 10) / 10,
      w: Math.round(shape.rect.w * scale * 10) / 10,
      h: Math.round(shape.rect.h * scale * 10) / 10,
      tone,
      round: shape.shape === "Db" || shape.shape === "Actor",
    };
  });
}
