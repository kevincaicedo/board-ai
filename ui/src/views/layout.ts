// Where things sit while a view is open (the canvas's 8 · Library, 9 · Brain and memory frames):
// the panels' widths, the free space left of them, and where the orb and the captions go. The
// library and the boards dock the orb at bottom center; the panels move the conversing orb (and
// its captions) to the middle of the space beside them (D-150: the canvas's 1440 px offsets,
// kept as ratios, drifted right on a wider window). Pure functions, blended by each view's
// motion.

import type { ViewKindName } from "../app/views.ts";
import type { Viewport } from "../canvas/camera.ts";
import { lerp } from "../canvas/math.ts";
import type { OrbPlacement } from "../canvas/presence.ts";
import type { Tokens } from "../styles/tokens.ts";

export type ViewLayoutTokens = Pick<
  Tokens,
  | "viewFreeMin"
  | "panelRight"
  | "panelWidth"
  | "panelMemoryRight"
  | "panelMemoryWidth"
  | "panelMemoryGap"
  | "viewOrbTasks"
  | "viewOrbMemory"
  | "orbDockSize"
  | "orbDockSwell"
  | "captionWidth"
  | "captionMaxLetters"
  | "typeCaption"
>;

/** Where the orb goes for a view. */
export type OrbSpot =
  | { readonly kind: "dock"; readonly size: number }
  | {
      readonly kind: "aside";
      readonly x: number;
      readonly size: number;
      /** The free width left of the panels; captions keep inside it. */
      readonly free: number;
    };

/** The Now panel's width: the canvas's, narrowed so the orb keeps its room on small windows. */
export function tasksPanelWidth(stage: Viewport, tokens: ViewLayoutTokens): number {
  return Math.max(
    0,
    Math.min(tokens.panelWidth, stage.width - tokens.panelRight - tokens.viewFreeMin),
  );
}

/** Each of the two memory panels' width, by the same rule. */
export function memoryPanelWidth(stage: Viewport, tokens: ViewLayoutTokens): number {
  const room = stage.width - tokens.panelMemoryRight - tokens.panelMemoryGap - tokens.viewFreeMin;
  return Math.max(0, Math.min(tokens.panelMemoryWidth, room / 2));
}

export function orbSpot(kind: ViewKindName, stage: Viewport, tokens: ViewLayoutTokens): OrbSpot {
  switch (kind) {
    case "Library":
    case "PastSession":
    case "Replay":
      return { kind: "dock", size: tokens.orbDockSize + tokens.orbDockSwell };
    case "Tasks": {
      const free = stage.width - tokens.panelRight - tasksPanelWidth(stage, tokens);
      return { kind: "aside", x: free / 2, size: tokens.viewOrbTasks, free };
    }
    case "Memory": {
      const used =
        tokens.panelMemoryRight + 2 * memoryPanelWidth(stage, tokens) + tokens.panelMemoryGap;
      const free = stage.width - used;
      return { kind: "aside", x: free / 2, size: tokens.viewOrbMemory, free };
    }
  }
}

export interface CaptionPlace {
  /** Horizontal offset of the captions' center from the stage's center. */
  shift: number;
  /** Width the captions may use. */
  width: number;
}

/**
 * Moves the orb (and the captions with it) toward `spot` by `amount` (0 to 1). A docked orb (the
 * board is active) stays docked for a panel; only the library and the boards dock it themselves.
 */
export function placeForView(
  spot: OrbSpot,
  amount: number,
  stage: Viewport,
  dock: readonly [number, number],
  tokens: ViewLayoutTokens,
  orb: OrbPlacement,
  captions: CaptionPlace,
): void {
  if (amount <= 0) {
    return;
  }
  if (spot.kind === "dock") {
    orb.x = lerp(orb.x, dock[0], amount);
    orb.y = lerp(orb.y, dock[1], amount);
    orb.size = lerp(orb.size, spot.size, amount);
    orb.opacity = lerp(orb.opacity, 1, amount);
    orb.docked = lerp(orb.docked, 1, amount);
    return;
  }
  const weight = amount * (1 - orb.docked);
  orb.x = lerp(orb.x, spot.x, weight);
  orb.size = lerp(orb.size, spot.size, weight);
  captions.shift = lerp(captions.shift, spot.x - stage.width / 2, amount);
  captions.width = lerp(captions.width, Math.min(tokens.captionWidth, spot.free), amount);
}

/** The UI face averages about half its size per letter. */
const LETTER_WIDTH_EM = 0.52;
/** A caption line keeps at least this many letters, however narrow its room. */
const CAPTION_LETTERS_MIN = 12;

/** Letters a caption line may hold in `width`: the design's 54, fewer in a narrow space. */
export function captionLetters(width: number, tokens: ViewLayoutTokens): number {
  const room = width > 0 ? width : tokens.captionWidth;
  const fitting = Math.floor(room / (tokens.typeCaption * LETTER_WIDTH_EM));
  return Math.max(CAPTION_LETTERS_MIN, Math.min(tokens.captionMaxLetters, fitting));
}
