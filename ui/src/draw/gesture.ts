// The pointer surface's gestures (13 · You draw), apart from the DOM so node can test them: one
// gesture at a time, sampled into one preallocated sampler, sent as a shape when it ends. A
// gesture ends with its pointerup, a lost capture, or drawing ending under it (the core heard
// "done" while the pen was down): the last two send what was drawn, since the core accepts a
// shape that lands after drawing ends (D-167). The surface is measured once, at pointerdown.

import { effect, untracked } from "@preact/signals";

import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import { drawnShape, PenPath, previewPath, StrokeSampler, type WorldTransform } from "./input.ts";
import type { DrawModel, Tool } from "./model.ts";

/** What the surface needs from the stage. */
export interface GestureHost {
  readonly draw: DrawModel;
  readonly send: (command: UiCommand) => void;
  /** The main layer's camera, or `null` before the board is mounted (nothing can be sent). */
  readonly transform: () => WorldTransform | null;
}

/** The fields of a pointer event the gestures read. */
export interface PointerInput {
  readonly pointerId: number;
  readonly button: number;
  /** Buttons held now; the primary one is bit 1. */
  readonly buttons: number;
  readonly clientX: number;
  readonly clientY: number;
}

/** Where the surface sits in the window, read once per gesture. */
export interface SurfaceOrigin {
  readonly left: number;
  readonly top: number;
}

/** A gesture that ended in a shape the core was sent: its preview path, in screen pixels. */
export interface SentShape {
  readonly d: string;
}

export class DrawingInput {
  readonly #host: GestureHost;
  readonly #sampler = new StrokeSampler();
  readonly #pen = new PenPath();
  #active: { pointer: number; tool: Tool } | null = null;
  #left = 0;
  #top = 0;

  constructor(host: GestureHost) {
    this.#host = host;
  }

  /** A gesture is in flight. */
  get active(): boolean {
    return this.#active !== null;
  }

  /**
   * Follows drawing mode for as long as the surface is mounted; returns the unsubscriber. The
   * surface stops rendering when drawing ends, so no pointer event would ever end its gesture.
   */
  watch(): () => void {
    return effect(() => {
      if (!this.#host.draw.drawing.value) {
        // Only drawing mode is followed: whatever sending reads must not re-run this.
        untracked(() => this.end());
      }
    });
  }

  /** Drawing ended: a gesture in flight is sent as drawn so far. */
  end(): SentShape | null {
    const gesture = this.#active;
    if (gesture === null) {
      return null;
    }
    this.#active = null;
    return this.#send(gesture.tool);
  }

  /** The live preview's path. */
  preview(): string {
    switch (this.#active?.tool) {
      case undefined:
        return "";
      case "pen":
        return this.#pen.path(this.#sampler);
      case "box":
      case "arrow":
        return previewPath(this.#active.tool, this.#sampler);
    }
  }

  /** The primary button went down: a gesture starts. Returns whether it did. */
  down(event: PointerInput, origin: SurfaceOrigin): boolean {
    if (event.button !== 0 || this.#active !== null) {
      return false;
    }
    this.#active = { pointer: event.pointerId, tool: this.#host.draw.tool.peek() };
    this.#left = origin.left;
    this.#top = origin.top;
    this.#sampler.begin(event.clientX - this.#left, event.clientY - this.#top);
    return true;
  }

  /**
   * A move; returns whether the preview changed. One without the primary button is a hover (its
   * pointerup was lost, outside the window): it never draws.
   */
  move(event: PointerInput): boolean {
    if (this.#active?.pointer !== event.pointerId || (event.buttons & 1) === 0) {
      return false;
    }
    return this.#sampler.add(event.clientX - this.#left, event.clientY - this.#top);
  }

  /** The pointer came up: the gesture's shape is sent. `null` when nothing was. */
  up(event: PointerInput): SentShape | null {
    const gesture = this.#active;
    if (gesture === null || gesture.pointer !== event.pointerId) {
      return null;
    }
    this.#active = null;
    this.#sampler.finish(event.clientX - this.#left, event.clientY - this.#top);
    return this.#send(gesture.tool);
  }

  /**
   * The surface lost the pointer's capture without a pointerup first (after one there is no
   * gesture left): what was drawn is sent, as at a pointerup where the pointer last was.
   */
  lost(event: Pick<PointerInput, "pointerId">): SentShape | null {
    return this.#active?.pointer === event.pointerId ? this.end() : null;
  }

  /** The browser took the pointer (pointercancel): nothing is sent. */
  cancel(event: Pick<PointerInput, "pointerId">): boolean {
    if (this.#active?.pointer !== event.pointerId) {
      return false;
    }
    this.#active = null;
    return true;
  }

  #send(tool: Tool): SentShape | null {
    const d = previewPath(tool, this.#sampler);
    const transform = this.#host.transform();
    const shape = transform === null ? null : drawnShape(tool, this.#sampler, transform);
    if (shape === null) {
      return null;
    }
    this.#host.send({ type: "Draw", action: { kind: "Shape", layer: 0, drawn: shape } });
    return { d };
  }
}
