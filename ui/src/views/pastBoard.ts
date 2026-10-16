// A past session's board for the recall view (PRODUCT §7.4: "brings back the canvas"). It is
// drawn at once, with no pacing (every op gets a zero budget), into its own board model, so the
// live board stays exactly what the core drew (law 1). The camera frames it to fit the space left
// of the cards without ever enlarging the handwriting.

import { BoardModel, type LeadDecision } from "../canvas/board.ts";
import type { FitArea, Viewport } from "../canvas/camera.ts";
import type { PlanTokens } from "../canvas/plan.ts";
import type { PastLayer } from "../protocol/generated/PastLayer.ts";
import type { Rect } from "../protocol/generated/Rect.ts";

/** Room kept around a past board inside its frame, in world units. */
export const PAST_BOARD_MARGIN = 48;

/** A past board is already drawn: no nib flies, no pen travels. */
const NO_LEAD: LeadDecision = {
  nibAtRest: () => "out",
  flightLead: () => ({ waitMs: 0, flightMs: 0 }),
};

/**
 * The main layer of a past board, fully drawn at `at`. Detour layers still open when that session
 * ended are left out: they belonged to a card that isn't there any more.
 */
export function pastBoardModel(
  layers: readonly PastLayer[],
  tokens: PlanTokens,
  at: number,
): BoardModel {
  const board = new BoardModel(tokens);
  const main = layers.find((layer) => layer.title === null);
  for (const op of main?.ops ?? []) {
    board.apply({ type: "OpStart", layer: 0, op, budget_ms: 0 }, at, NO_LEAD);
  }
  return board;
}

/** The world-space box holding every element drawn on the main layer, or null when empty. */
export function boardBounds(board: BoardModel): Rect | null {
  let left = Number.POSITIVE_INFINITY;
  let top = Number.POSITIVE_INFINITY;
  let right = Number.NEGATIVE_INFINITY;
  let bottom = Number.NEGATIVE_INFINITY;
  for (const run of board.main.runs) {
    if (run.replacedAt !== null) {
      continue;
    }
    const { x, y, w, h } = run.plan.bounds;
    left = Math.min(left, x);
    top = Math.min(top, y);
    right = Math.max(right, x + w);
    bottom = Math.max(bottom, y + h);
  }
  return Number.isFinite(left) ? { x: left, y: top, w: right - left, h: bottom - top } : null;
}

/**
 * The camera frame for `bounds` in `viewport`: centered, with a margin, the viewport's aspect,
 * and never smaller than the viewport, so the board is shrunk to fit but never enlarged.
 */
export function pastFrame(bounds: Rect, viewport: Viewport): Rect {
  const ratio = viewport.width > 0 && viewport.height > 0 ? viewport.width / viewport.height : 1;
  let width = Math.max(bounds.w + 2 * PAST_BOARD_MARGIN, viewport.width);
  let height = Math.max(bounds.h + 2 * PAST_BOARD_MARGIN, viewport.height);
  if (width / height < ratio) {
    width = height * ratio;
  } else {
    height = width / ratio;
  }
  const centerX = bounds.x + bounds.w / 2;
  const centerY = bounds.y + bounds.h / 2;
  return {
    x: Math.round(centerX - width / 2),
    y: Math.round(centerY - height / 2),
    w: Math.round(width),
    h: Math.round(height),
  };
}

/** Where a past board is fitted: its whole host, at most at its drawn size (never enlarged). */
export function pastArea(viewport: Viewport): FitArea {
  return { x: 0, y: 0, w: viewport.width, h: viewport.height, maxScale: 1 };
}

/**
 * Points the past board's camera at its contents for `viewport`, at once: the board is a
 * picture, so a new window size reframes it without a camera move. A viewport not laid out yet
 * (zero size) changes nothing; the next resize frames it.
 */
export function framePastBoard(board: BoardModel, viewport: Viewport, at: number): void {
  const bounds = boardBounds(board);
  if (bounds === null || viewport.width <= 0 || viewport.height <= 0) {
    return;
  }
  board.main.camera = {
    frame: pastFrame(bounds, viewport),
    previous: null,
    at,
    focus: null,
    previousFocus: null,
    focusAt: at,
  };
}
