// Your shapes on the canvas (13 · You draw): one SVG group per board layer, inside the layer's
// camera group above BoardAI's handwriting, rebuilt only when the drawing model changes (never
// per frame). A committed shape appears whole at once: you just drew it. Each shape is a pointer
// target like BoardAI's elements, so pointing at `u2` works.

import type { PointerTarget } from "../protocol/generated/PointerTarget.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import { type InkPiece, type InkTokens, planYours } from "./ink.ts";
import type { DrawModel, Yours } from "./model.ts";

const SVG = "http://www.w3.org/2000/svg";

interface LayerGroups {
  readonly strokes: SVGGElement;
  readonly labels: SVGGElement;
  readonly shapes: Map<string, { readonly yours: Yours; readonly parts: readonly SVGElement[] }>;
}

function element<K extends keyof SVGElementTagNameMap>(
  name: K,
  parent: Element,
  className: string,
): SVGElementTagNameMap[K] {
  const created = document.createElementNS(SVG, name);
  created.setAttribute("class", className);
  parent.append(created);
  return created;
}

function round1(value: number): string {
  return String(Math.round(value * 10) / 10);
}

export class YoursView {
  readonly #model: DrawModel;
  readonly #tokens: InkTokens;
  readonly #onPointer: (target: PointerTarget | null) => void;
  readonly #layers = new Map<number, LayerGroups>();
  #synced = -1;
  #hovered: string | null = null;

  constructor(
    model: DrawModel,
    tokens: InkTokens,
    onPointer: (target: PointerTarget | null) => void,
  ) {
    this.#model = model;
    this.#tokens = tokens;
    this.#onPointer = onPointer;
  }

  /** While you draw the main board comes forward: a detour's push-back lifts. */
  get lifted(): boolean {
    return this.#model.drawing.peek();
  }

  /** A board layer's view was created: your shapes on it draw into `group`. */
  attach(layer: number, group: SVGGElement): void {
    this.#layers.set(layer, {
      strokes: element("g", group, "yours-strokes"),
      labels: element("g", group, "yours-labels"),
      shapes: new Map(),
    });
    this.#synced = -1;
  }

  detach(layer: number): void {
    this.#layers.delete(layer);
  }

  /** The board view was reset; its SVG (and so ours) is gone. */
  reset(): void {
    this.#layers.clear();
    this.#synced = -1;
    this.#hovered = null;
  }

  /** Brings the groups up to the model; cheap when nothing changed. */
  sync(): void {
    if (this.#synced === this.#model.version) {
      return;
    }
    this.#synced = this.#model.version;
    for (const [layer, groups] of this.#layers) {
      const shapes = this.#model.layers.get(layer);
      for (const [id, drawn] of groups.shapes) {
        if (shapes?.get(id) !== drawn.yours) {
          for (const part of drawn.parts) {
            part.remove();
          }
          groups.shapes.delete(id);
        }
      }
      for (const [id, yours] of shapes ?? []) {
        if (!groups.shapes.has(id)) {
          groups.shapes.set(id, { yours, parts: this.#draw(layer, groups, yours) });
        }
      }
    }
  }

  #draw(layer: number, groups: LayerGroups, yours: Yours): SVGElement[] {
    const plan = planYours(yours.shape, this.#tokens);
    const strokes = element("g", groups.strokes, "element yours");
    const labels = element("g", groups.labels, "yours");
    for (const piece of plan.pieces) {
      this.#piece(piece, strokes, labels);
    }
    this.#hit(strokes, layer, yours.shape.id, plan.bounds);
    return [strokes, labels];
  }

  #piece(piece: InkPiece, strokes: SVGGElement, labels: SVGGElement): void {
    if (piece.type === "path") {
      const path = element("path", strokes, "stroke tone-you");
      path.setAttribute("d", piece.d);
      path.setAttribute("stroke-width", String(piece.width));
      return;
    }
    const text = element("text", labels, piece.note ? "hand tone-you you-note" : "hand tone-you");
    text.setAttribute("x", round1(piece.x));
    text.setAttribute("y", round1(piece.y));
    text.setAttribute("font-size", String(piece.size));
    text.setAttribute("text-anchor", piece.align === "left" ? "start" : "middle");
    // As the renderer writes handwriting: centred on y, set as attributes for WebKit.
    text.setAttribute("dominant-baseline", "central");
    text.setAttribute("dy", "-0.1em");
    text.textContent = piece.text;
  }

  #hit(group: SVGGElement, layer: number, id: string, bounds: Rect): void {
    const hit = element("rect", group, "hit");
    hit.setAttribute("x", round1(bounds.x));
    hit.setAttribute("y", round1(bounds.y));
    hit.setAttribute("width", round1(bounds.w));
    hit.setAttribute("height", round1(bounds.h));
    hit.addEventListener("pointerenter", () => {
      this.#hovered = id;
      this.#onPointer({ kind: "Element", layer, id });
    });
    hit.addEventListener("pointerleave", () => {
      if (this.#hovered === id) {
        this.#hovered = null;
        this.#onPointer(null);
      }
    });
  }
}
