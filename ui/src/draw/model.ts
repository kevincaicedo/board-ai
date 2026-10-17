// What the UI shows of drawing together (13 · You draw), built only from the core's events: whether
// you are drawing, the label hint while you speak, and your shapes per layer as the core keeps
// them (the committed copy of what the pointer previewed). The tool you hold is the UI's own
// choice, like a scroll position. Nothing here is per-frame: the renderer syncs when `version`
// moves.

import { signal } from "@preact/signals";
import { LEAVING_PAGE } from "../canvas/board.ts";
import type { LabelHint } from "../protocol/generated/LabelHint.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import { USER_SHAPES_PER_LAYER_MAX } from "./limits.ts";

/** What the pointer draws while you draw. */
export type Tool = "pen" | "box" | "arrow";

export const TOOLS: readonly Tool[] = ["pen", "box", "arrow"];

export type DrawEvent = Extract<
  UiEvent,
  { type: "DrawMode" | "UserShape" | "UserShapeRemove" | "LabelHint" }
>;

/** One of your shapes on a layer, with when it last changed. */
export interface Yours {
  readonly layer: number;
  readonly shape: UserShape;
  readonly at: number;
}

export interface DrawCounters {
  /** Shapes past `USER_SHAPES_PER_LAYER_MAX` on a layer, dropped (the core never sends one). */
  dropped: number;
}

export class DrawModel {
  /** You are drawing: the tools show and BoardAI watches. */
  readonly drawing = signal(false);
  /** The words heard so far and the shape they will label, while you speak. */
  readonly hint = signal<LabelHint | null>(null);
  /** The tool the pointer draws with (UI-local). */
  readonly tool = signal<Tool>("pen");
  /**
   * Shapes the core took from the pointer: bumped when a shape is new on its layer, or a pen
   * shape gained a stroke (one joined it, D-164). Each shape sent is at most one of these, so a
   * preview waits for its own; a label, a note, an arrow reconnecting or an undo re-sends a shape
   * without taking one, and must not take a preview away early. Never reset: previews compare
   * against it across sessions.
   */
  readonly accepted = signal(0);
  /** Your shapes by layer, then by id, in the order they were first drawn. */
  readonly layers = new Map<number, Map<string, Yours>>();
  readonly counters: DrawCounters = { dropped: 0 };
  /** Bumped on every change to the shapes, so the renderer rebuilds only then. */
  version = 0;

  reset(): void {
    this.drawing.value = false;
    this.hint.value = null;
    this.layers.clear();
    this.counters.dropped = 0;
    this.version += 1;
  }

  /** The session ended: drawing is over, and the shapes stay until the next session's reset. */
  end(): void {
    this.drawing.value = false;
    this.hint.value = null;
  }

  apply(event: DrawEvent, at: number): void {
    switch (event.type) {
      case "DrawMode":
        this.drawing.value = event.drawing;
        if (!event.drawing) {
          this.hint.value = null;
        }
        break;
      case "LabelHint":
        this.hint.value = event.hint;
        break;
      case "UserShape":
        this.#put(event.layer, event.shape, at);
        break;
      case "UserShapeRemove":
        if (this.layers.get(event.layer)?.delete(event.id) === true) {
          this.version += 1;
        }
        break;
    }
  }

  /**
   * The page turned (D-177): your shapes on the main board leave with it (the core keeps them
   * with the earlier page), drawn on the leaving page's layer until it is let go.
   */
  turnPage(): void {
    const main = this.layers.get(0);
    this.layers.delete(LEAVING_PAGE);
    if (main !== undefined) {
      this.layers.set(LEAVING_PAGE, main);
      this.layers.delete(0);
    }
    this.version += 1;
  }

  /** Your shape `id` on `layer`, if it is on the board. */
  shape(layer: number, id: string): Yours | null {
    return this.layers.get(layer)?.get(id) ?? null;
  }

  #put(layer: number, shape: UserShape, at: number): void {
    let shapes = this.layers.get(layer);
    if (shapes === undefined) {
      shapes = new Map();
      this.layers.set(layer, shapes);
    }
    const before = shapes.get(shape.id);
    if (before === undefined && shapes.size >= USER_SHAPES_PER_LAYER_MAX) {
      this.counters.dropped += 1;
      return;
    }
    shapes.set(shape.id, { layer, shape, at });
    this.version += 1;
    if (before === undefined || strokeCount(shape) > strokeCount(before.shape)) {
      this.accepted.value += 1;
    }
  }
}

/** A pen shape's strokes; other shapes take none after the first. */
function strokeCount(shape: UserShape): number {
  return shape.kind.kind === "Ink" ? shape.kind.strokes.length : 0;
}
