// The SVG elements of a plan's items (LLD §4): a path per stroke, revealed through its dash
// offset (or, for a dashed stroke, through a mask's), a filled path per fill, and handwriting as
// two <tspan>s whose split moves letter by letter. Made once per item, never per frame.

import { round1 } from "./math.ts";
import { maskRegion, type Stroke, type TextMark } from "./stroke.ts";

const SVG = "http://www.w3.org/2000/svg";

/** Gap after a stroke's dash so the hidden remainder never shows (the design's +40). */
export const DASH_TAIL = 40;

/** Room a dashed stroke's mask keeps past its points: its reveal path is 4 wider, and the caps. */
const MASK_PAD = 4;

/** Mask ids are unique in the document, whichever view made them. */
let maskSerial = 0;

export function svg<K extends keyof SVGElementTagNameMap>(
  name: K,
  parent: Element,
  className?: string,
): SVGElementTagNameMap[K] {
  const element = document.createElementNS(SVG, name);
  if (className !== undefined) {
    element.setAttribute("class", className);
  }
  parent.append(element);
  return element;
}

export interface StrokeView {
  readonly item: Stroke;
  readonly path: SVGPathElement;
  /** The reveal path for a dashed stroke lives in a mask; the visible path carries the dashes. */
  readonly reveal: SVGPathElement;
}

export interface TextView {
  readonly item: TextMark;
  readonly text: SVGTextElement;
  readonly written: SVGTSpanElement;
  readonly unwritten: SVGTSpanElement;
}

export type ItemView = StrokeView | TextView;

export function createStroke(group: SVGGElement, item: Stroke, handDash: string): StrokeView {
  if (item.kind === "fill") {
    // A staged box's soft fill: painted, not stroked, under the ink, fading in (canvas 15).
    const path = svg("path", group, `fill tone-${item.tone}`);
    group.prepend(path);
    path.setAttribute("d", item.path);
    path.style.display = "none";
    path.style.opacity = "0";
    return { item, path, reveal: path };
  }
  const path = svg("path", group, `stroke tone-${item.tone}`);
  if (item.kind === "hatch") {
    // Fills are drawn last but sit under the ink, so dividers and outlines stay unbroken.
    group.prepend(path);
  }
  path.setAttribute("d", item.path);
  path.setAttribute("stroke-width", String(item.width));
  path.style.display = "none";
  if (!item.dashed) {
    path.setAttribute(
      "stroke-dasharray",
      `${round1(item.length)} ${round1(item.length + DASH_TAIL)}`,
    );
    path.setAttribute("stroke-dashoffset", String(round1(item.length)));
    return { item, path, reveal: path };
  }
  // A dashed stroke can't also use its dash array to reveal itself, so a mask path does.
  maskSerial += 1;
  const maskId = `reveal-${maskSerial}`;
  const mask = svg("mask", group);
  mask.setAttribute("id", maskId);
  mask.setAttribute("maskUnits", "userSpaceOnUse");
  // Without a region a mask covers -10%…120% of the viewport in user units, which a scaled
  // camera can leave short of a long stroke: a frame far down lost its bottom side.
  const region = maskRegion(item, item.width + MASK_PAD);
  mask.setAttribute("x", String(region.x));
  mask.setAttribute("y", String(region.y));
  mask.setAttribute("width", String(region.w));
  mask.setAttribute("height", String(region.h));
  const reveal = svg("path", mask, "stroke-reveal");
  reveal.setAttribute("d", item.path);
  reveal.setAttribute("stroke-width", String(item.width + 4));
  reveal.setAttribute(
    "stroke-dasharray",
    `${round1(item.length)} ${round1(item.length + DASH_TAIL)}`,
  );
  reveal.setAttribute("stroke-dashoffset", String(round1(item.length)));
  path.setAttribute("stroke-dasharray", item.dashPattern ?? handDash);
  path.setAttribute("mask", `url(#${maskId})`);
  return { item, path, reveal };
}

export function createText(group: SVGGElement, item: TextMark): TextView {
  const text = svg("text", group, `${item.font === "code" ? "code" : "hand"} tone-${item.tone}`);
  text.setAttribute("x", String(round1(item.x)));
  text.setAttribute("y", String(round1(item.y)));
  text.setAttribute("font-size", String(item.size));
  text.setAttribute(
    "text-anchor",
    item.align === "left" ? "start" : item.align === "right" ? "end" : "middle",
  );
  // The design centers a line box 55% above the baseline point; central sits 0.05em lower.
  // Set as attributes: WebKit honors dominant-baseline on SVG text more reliably than in CSS.
  text.setAttribute("dominant-baseline", "central");
  text.setAttribute("dy", "-0.1em");
  text.style.display = "none";
  const written = svg("tspan", text);
  const unwritten = svg("tspan", text, "unwritten");
  unwritten.textContent = item.text;
  return { item, text, written, unwritten };
}
