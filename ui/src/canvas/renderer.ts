// The canvas renderer: plain imperative SVG on the frame loop (LLD §4). One <path> per stroke and
// one per hachure fill, revealed through stroke-dashoffset; handwriting is two <tspan>s whose
// split moves letter by letter. Nothing is allocated for strokes that are already done, and an
// attribute is written only when its value changes.

import type { YoursView } from "../draw/inkView.ts";
import type { PointerTarget } from "../protocol/generated/PointerTarget.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import { type BoardModel, type Layer, type Run, runActive, traceRunning } from "./board.ts";
import { cameraAt, depthOfField, type FitArea, type Viewport } from "./camera.ts";
import type { DetourCard } from "./detourCard.ts";
import type { Vec } from "./geometry.ts";
import { clamp, easeInOutCubic, easeOut, ramp, round1, round2 } from "./math.ts";
import { type Anchors, type NibFrame, nibAt, type Presence, type Transform } from "./presence.ts";
import {
  lettersAt,
  pointAtLength,
  revealLength,
  revealLengthStepped,
  type Stroke,
  type TextMark,
  textOffset,
} from "./stroke.ts";
import { naturalTime } from "./timeline.ts";

const SVG = "http://www.w3.org/2000/svg";
/** Gap after a stroke's dash so the hidden remainder never shows (the design's +40). */
const DASH_TAIL = 40;
const HOVER_PAD = 16;

type RenderTokens = Pick<
  Tokens,
  | "durationSceneMs"
  | "durationStandardMs"
  | "detourPushBlur"
  | "detourPushShift"
  | "detourPushShrink"
  | "detourPushFade"
  | "surfaceBlurOut"
  | "dofBlur"
  | "dofFade"
  | "handDash"
  | "handDashGap"
>;

export interface BoardViewOptions {
  readonly tokens: RenderTokens;
  /** The coordinate space of a detour card's layer (the card's design size). */
  readonly card: Viewport;
  readonly reducedMotion: boolean;
  readonly onPointer: (target: PointerTarget | null) => void;
  /** Your shapes (13 · You draw), drawn into each layer above BoardAI's handwriting. */
  readonly yours?: YoursView;
}

function svg<K extends keyof SVGElementTagNameMap>(
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

const HIDDEN = 0;
const PARTIAL = 1;
const DONE = 2;

interface StrokeView {
  readonly item: Stroke;
  readonly path: SVGPathElement;
  /** The reveal path for a dashed stroke lives in a mask; the visible path carries the dashes. */
  readonly reveal: SVGPathElement;
}

interface TextView {
  readonly item: TextMark;
  readonly text: SVGTextElement;
  readonly written: SVGTSpanElement;
  readonly unwritten: SVGTSpanElement;
}

type ItemView = StrokeView | TextView;

interface RunView {
  readonly run: Run;
  readonly group: SVGGElement;
  /** Handwriting sits above every stroke on the layer, as in the design, so fills never cross it. */
  readonly labels: SVGGElement;
  readonly items: ItemView[];
  readonly states: Int8Array;
  readonly progress: Float64Array;
  readonly trace: { readonly dot: SVGGElement; readonly at: Vec } | null;
  done: boolean;
  soft: boolean;
}

interface LayerView {
  readonly id: number;
  readonly svg: SVGSVGElement;
  readonly camera: SVGGElement;
  readonly strokes: SVGGElement;
  readonly labels: SVGGElement;
  readonly hover: SVGRectElement;
  readonly blur: SVGFEGaussianBlurElement;
  readonly filterId: string;
  readonly runs: Map<number, RunView>;
  readonly transform: Transform;
  host: HTMLElement | null;
  /** Scale, x and y last written, so an unchanged camera writes nothing. */
  readonly lastTransform: Float64Array;
  lastBlur: number;
  /** A detour layer's viewBox last written: its card's size, one unit per CSS pixel (D-151). */
  lastViewBox: string;
}

export class BoardView {
  readonly #model: BoardModel;
  readonly #presence: Presence;
  readonly #options: BoardViewOptions;
  readonly #mainHost: HTMLElement;
  readonly #layers = new Map<number, LayerView>();
  readonly #hosts = new Map<number, HTMLElement>();
  readonly #nib: HTMLDivElement;
  readonly #nibGlow: HTMLDivElement;
  readonly #nibFrame: NibFrame = {
    visible: false,
    x: 0,
    y: 0,
    scale: 1,
    down: false,
    frozen: false,
    frozenAt: 0,
  };
  #syncedVersion = -1;
  /** NaN forces the first write, so a reset never leaves a stale push-back on the board. */
  /** The push-back and end fade last written (`push|end`), so an unchanged board writes nothing. */
  #lastPush = "";
  #lastNib = "";
  #hovered: { layer: number; key: string } | null = null;
  #maskSerial = 0;

  constructor(
    model: BoardModel,
    presence: Presence,
    mainHost: HTMLElement,
    options: BoardViewOptions,
  ) {
    this.#model = model;
    this.#presence = presence;
    this.#mainHost = mainHost;
    this.#options = options;
    this.#hosts.set(0, mainHost);
    this.#nib = document.createElement("div");
    this.#nib.className = "nib";
    this.#nib.setAttribute("aria-hidden", "true");
    this.#nibGlow = document.createElement("div");
    this.#nibGlow.className = "nib-glow";
    const core = document.createElement("div");
    core.className = "nib-core";
    this.#nib.append(this.#nibGlow, core);
    mainHost.append(this.#nib);
  }

  /** A detour card mounted (or unmounted) the element its layer draws into. */
  attachLayerHost(layer: number, host: HTMLElement | null): void {
    if (host === null) {
      this.#hosts.delete(layer);
    } else {
      this.#hosts.set(layer, host);
    }
    const view = this.#layers.get(layer);
    if (view !== undefined) {
      view.host = host;
      if (host !== null) {
        host.append(view.svg);
      } else {
        view.svg.remove();
      }
    }
  }

  /** The main layer's current world → screen transform, for the nib and pointer math. */
  get mainTransform(): Transform {
    return this.#layers.get(0)?.transform ?? { scale: 1, x: 0, y: 0 };
  }

  reset(): void {
    for (const view of this.#layers.values()) {
      view.svg.remove();
    }
    this.#layers.clear();
    this.#syncedVersion = -1;
    this.#lastPush = "";
    this.#hovered = null;
    this.#options.yours?.reset();
  }

  /**
   * `stage` is where the main layer fits its camera frame; each detour layer fits its own in its
   * card (`cards`, sized from its drawing, D-151).
   */
  render(
    at: number,
    stage: FitArea,
    cards: ReadonlyMap<number, DetourCard>,
    anchors: Anchors,
  ): void {
    if (this.#syncedVersion !== this.#model.version) {
      this.#sync();
      this.#syncedVersion = this.#model.version;
    }
    this.#options.yours?.sync();
    for (const view of this.#layers.values()) {
      const layer = this.#model.layers.get(view.id);
      if (layer === undefined) {
        continue;
      }
      if (view.id === 0) {
        this.#renderLayer(view, layer, at, stage);
        continue;
      }
      const card = cards.get(view.id);
      if (card !== undefined) {
        this.#sizeDetour(view, card);
        this.#renderLayer(view, layer, at, card.area);
      }
    }
    this.#renderPush(at);
    this.#renderNib(at, anchors);
  }

  #sync(): void {
    for (const [id, view] of this.#layers) {
      if (!this.#model.layers.has(id)) {
        view.svg.remove();
        this.#layers.delete(id);
        this.#options.yours?.detach(id);
      }
    }
    for (const layer of this.#model.layers.values()) {
      let view = this.#layers.get(layer.id);
      if (view === undefined) {
        view = this.#createLayer(layer.id);
        this.#layers.set(layer.id, view);
      }
      const live = new Set<number>();
      for (const run of layer.runs) {
        if (run.replacedAt !== null) {
          continue;
        }
        live.add(run.serial);
        if (!view.runs.has(run.serial)) {
          view.runs.set(run.serial, this.#createRun(view, run));
        }
      }
      for (const [serial, runView] of view.runs) {
        if (!live.has(serial)) {
          runView.group.remove();
          runView.labels.remove();
          view.runs.delete(serial);
        }
      }
    }
  }

  #createLayer(id: number): LayerView {
    const root = document.createElementNS(SVG, "svg");
    root.setAttribute("class", id === 0 ? "board-svg" : "board-svg board-svg-detour");
    root.setAttribute("aria-hidden", "true");
    if (id !== 0) {
      root.setAttribute("viewBox", `0 0 ${this.#options.card.width} ${this.#options.card.height}`);
    }
    const defs = svg("defs", root);
    const filterId = `dof-${id}`;
    const filter = svg("filter", defs);
    filter.setAttribute("id", filterId);
    filter.setAttribute("x", "-20%");
    filter.setAttribute("y", "-20%");
    filter.setAttribute("width", "140%");
    filter.setAttribute("height", "140%");
    const blur = svg("feGaussianBlur", filter);
    blur.setAttribute("stdDeviation", "0");
    const camera = svg("g", root);
    const hover = svg("rect", camera, "hover-highlight");
    hover.setAttribute("rx", "20");
    const strokes = svg("g", camera);
    const labels = svg("g", camera);
    this.#options.yours?.attach(id, svg("g", camera));
    const host = this.#hosts.get(id) ?? null;
    if (host !== null) {
      if (id === 0) {
        host.insertBefore(root, this.#nib);
      } else {
        host.append(root);
      }
    }
    return {
      id,
      svg: root,
      camera,
      strokes,
      labels,
      hover,
      blur,
      filterId,
      runs: new Map(),
      transform: { scale: 1, x: 0, y: 0 },
      host,
      lastTransform: Float64Array.of(Number.NaN, Number.NaN, Number.NaN),
      lastBlur: -1,
      lastViewBox: "",
    };
  }

  #createRun(layer: LayerView, run: Run): RunView {
    const group = svg("g", layer.strokes, "element");
    const labels = svg("g", layer.labels);
    const items: ItemView[] = [];
    for (const item of run.plan.items) {
      items.push(
        item.type === "stroke" ? this.#createStroke(group, item) : this.#createText(labels, item),
      );
    }
    let trace: RunView["trace"] = null;
    if (run.trace !== null) {
      const dot = svg("g", group, "trace-dot");
      svg("circle", dot, "trace-halo").setAttribute("r", "9");
      svg("circle", dot, "trace-core").setAttribute("r", "5");
      dot.style.display = "none";
      trace = { dot, at: [0, 0] };
    } else if (run.op.kind !== "Strike" && run.op.kind !== "Write") {
      this.#hit(group, layer, run.plan.bounds, run.op.id, 6);
      // A row's cells sit over the row, so pointing at a cell names the cell (`arr.3`).
      for (const target of run.plan.targets) {
        this.#hit(group, layer, target.rect, target.id, 0);
      }
    }
    return {
      run,
      group,
      labels,
      items,
      states: new Int8Array(items.length),
      progress: new Float64Array(items.length).fill(-1),
      trace,
      done: false,
      soft: false,
    };
  }

  #createStroke(group: SVGGElement, item: Stroke): StrokeView {
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
    this.#maskSerial += 1;
    const maskId = `reveal-${this.#maskSerial}`;
    const mask = svg("mask", group);
    mask.setAttribute("id", maskId);
    mask.setAttribute("maskUnits", "userSpaceOnUse");
    const reveal = svg("path", mask, "stroke-reveal");
    reveal.setAttribute("d", item.path);
    reveal.setAttribute("stroke-width", String(item.width + 4));
    reveal.setAttribute(
      "stroke-dasharray",
      `${round1(item.length)} ${round1(item.length + DASH_TAIL)}`,
    );
    reveal.setAttribute("stroke-dashoffset", String(round1(item.length)));
    path.setAttribute(
      "stroke-dasharray",
      `${this.#options.tokens.handDash} ${this.#options.tokens.handDashGap}`,
    );
    path.setAttribute("mask", `url(#${maskId})`);
    return { item, path, reveal };
  }

  #createText(group: SVGGElement, item: TextMark): TextView {
    const text = svg("text", group, `hand tone-${item.tone}`);
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

  /** A pointer target for element `id`; the hover is keyed by the element it names. */
  #hit(group: SVGGElement, layer: LayerView, bounds: Rect, id: string, pad: number): void {
    const hit = svg("rect", group, "hit");
    hit.setAttribute("x", String(round1(bounds.x - pad)));
    hit.setAttribute("y", String(round1(bounds.y - pad)));
    hit.setAttribute("width", String(round1(bounds.w + pad * 2)));
    hit.setAttribute("height", String(round1(bounds.h + pad * 2)));
    hit.addEventListener("pointerenter", () => this.#hover(layer, id, bounds));
    hit.addEventListener("pointerleave", () => this.#unhover(layer, id));
  }

  #hover(layer: LayerView, id: string, bounds: Rect): void {
    this.#hovered = { layer: layer.id, key: id };
    layer.hover.setAttribute("x", String(round1(bounds.x - HOVER_PAD)));
    layer.hover.setAttribute("y", String(round1(bounds.y - HOVER_PAD)));
    layer.hover.setAttribute("width", String(round1(bounds.w + HOVER_PAD * 2)));
    layer.hover.setAttribute("height", String(round1(bounds.h + HOVER_PAD * 2)));
    layer.hover.classList.add("on");
    this.#options.onPointer({ kind: "Element", layer: layer.id, id });
  }

  #unhover(layer: LayerView, key: string): void {
    if (this.#hovered?.layer === layer.id && this.#hovered.key === key) {
      this.#hovered = null;
      layer.hover.classList.remove("on");
      this.#options.onPointer(null);
    }
  }

  /** A detour layer draws one unit per pixel of its card, whatever size the card has grown to. */
  #sizeDetour(view: LayerView, card: DetourCard): void {
    const box = `0 0 ${card.width} ${card.height}`;
    if (box !== view.lastViewBox) {
      view.lastViewBox = box;
      view.svg.setAttribute("viewBox", box);
    }
  }

  #renderLayer(view: LayerView, layer: Layer, at: number, area: FitArea): void {
    const tokens = this.#options.tokens;
    cameraAt(layer.camera, at, area, tokens.durationSceneMs, view.transform);
    const scale = Math.round(view.transform.scale * 1000) / 1000;
    const x = round1(view.transform.x);
    const y = round1(view.transform.y);
    const last = view.lastTransform;
    if (scale !== last[0] || x !== last[1] || y !== last[2]) {
      last[0] = scale;
      last[1] = x;
      last[2] = y;
      view.camera.setAttribute("transform", `matrix(${scale} 0 0 ${scale} ${x} ${y})`);
    }
    const dof = depthOfField(layer.camera, at, tokens.durationStandardMs);
    const blur = round2(dof * tokens.dofBlur);
    if (blur !== view.lastBlur) {
      view.lastBlur = blur;
      view.blur.setAttribute("stdDeviation", String(blur));
    }
    const focus = layer.camera.focus ?? layer.camera.previousFocus;
    for (const runView of view.runs.values()) {
      const soft =
        dof > 0.001 && focus !== null && runView.run.key !== focus && runView.run.trace === null;
      if (soft !== runView.soft || soft) {
        runView.soft = soft;
        for (const group of [runView.group, runView.labels]) {
          if (soft) {
            group.setAttribute("filter", `url(#${view.filterId})`);
            group.setAttribute("opacity", String(round2(1 - tokens.dofFade * dof)));
          } else {
            group.removeAttribute("filter");
            group.removeAttribute("opacity");
          }
        }
      }
      if (runView.trace !== null) {
        this.#renderTrace(runView, layer, at);
      } else if (!runView.done) {
        this.#renderRun(runView, at);
      }
    }
  }

  #renderRun(view: RunView, at: number): void {
    const run = view.run;
    const natural = naturalTime(run.clock, at);
    const local = natural - run.leadMs;
    const { starts, ends } = run.plan;
    let finished = natural >= run.clock.naturalMs;
    for (let index = 0; index < view.items.length; index += 1) {
      const itemView = view.items[index];
      if (itemView === undefined) {
        continue;
      }
      const start = starts[index] ?? 0;
      const end = ends[index] ?? 0;
      if (natural < 0 || local < start) {
        finished = false;
        this.#setState(view, index, itemView, HIDDEN);
        continue;
      }
      const f = clamp((local - start) / (end - start || 1));
      if (f >= 1) {
        this.#write(view, index, itemView, 1);
        this.#setState(view, index, itemView, DONE);
      } else {
        finished = false;
        this.#setState(view, index, itemView, PARTIAL);
        this.#write(view, index, itemView, f);
      }
    }
    view.done = finished && !runActive(run, at);
  }

  #setState(view: RunView, index: number, item: ItemView, state: number): void {
    if (view.states[index] === state) {
      return;
    }
    view.states[index] = state;
    const element = "path" in item ? item.path : item.text;
    element.style.display = state === HIDDEN ? "none" : "";
    if (state === HIDDEN) {
      view.progress[index] = -1;
    }
  }

  #write(view: RunView, index: number, item: ItemView, f: number): void {
    if ("path" in item) {
      const stroke = item.item;
      const length = this.#options.reducedMotion
        ? revealLengthStepped(stroke, f)
        : revealLength(stroke, f);
      const offset = round1(stroke.length - length);
      if (view.progress[index] !== offset) {
        view.progress[index] = offset;
        item.reveal.setAttribute("stroke-dashoffset", String(offset));
      }
      return;
    }
    const letters = lettersAt(item.item, f);
    if (view.progress[index] !== letters) {
      view.progress[index] = letters;
      const split = textOffset(item.item, letters);
      item.written.textContent = item.item.text.slice(0, split);
      item.unwritten.textContent = item.item.text.slice(split);
    }
  }

  #renderTrace(view: RunView, layer: Layer, at: number): void {
    const run = view.run;
    const trace = view.trace;
    if (trace === null || run.trace === null) {
      return;
    }
    const running = traceRunning(run, layer, at);
    trace.dot.style.display = running ? "" : "none";
    if (!running) {
      return;
    }
    const period = run.trace.periodMs;
    const phase = ((at - run.clock.startAt) % period) / period;
    const stroke = run.trace.stroke;
    pointAtLength(stroke, stroke.length * easeInOutCubic(phase), trace.at);
    trace.dot.setAttribute("transform", `translate(${round1(trace.at[0])} ${round1(trace.at[1])})`);
    trace.dot.setAttribute(
      "opacity",
      String(round2(clamp(Math.sin(Math.PI * clamp(phase * 1.12)) * 1.5))),
    );
  }

  /**
   * The board steps back while a detour is open: blurred, faded, nudged left (frame 4). When the
   * session ends it fades out for good, blurring as a card leaves (D-145).
   */
  #renderPush(at: number): void {
    const tokens = this.#options.tokens;
    let amount = 0;
    for (let index = this.#model.detours.length - 1; index >= 0; index -= 1) {
      const span = this.#model.detours[index];
      if (span === undefined || span.openAt > at) {
        continue;
      }
      const rise = easeOut(ramp(at, span.openAt, span.openAt + tokens.durationSceneMs));
      const fall =
        span.closeAt === null || at < span.closeAt
          ? 0
          : easeOut(ramp(at, span.closeAt, span.closeAt + tokens.durationSceneMs));
      amount = rise * (1 - fall);
      break;
    }
    // While you draw, the main board you draw on stays sharp (13 · You draw).
    if (this.#options.yours?.lifted === true) {
      amount = 0;
    }
    const rounded = amount <= 0.001 ? 0 : Math.round(amount * 1000) / 1000;
    const ended = this.#model.endedAmount(at, tokens.durationSceneMs);
    const gone = ended <= 0.001 ? 0 : Math.round(ended * 1000) / 1000;
    const key = `${rounded}|${gone}`;
    if (key === this.#lastPush) {
      return;
    }
    this.#lastPush = key;
    const host = this.#mainHost.style;
    if (rounded === 0 && gone === 0) {
      host.removeProperty("filter");
      host.removeProperty("opacity");
      host.removeProperty("transform");
      return;
    }
    const blur = rounded * tokens.detourPushBlur + gone * tokens.surfaceBlurOut;
    host.filter = `blur(${round1(blur)}px)`;
    host.opacity = String(round2((1 - tokens.detourPushFade * rounded) * (1 - gone)));
    host.transform = `translateX(${round1(-tokens.detourPushShift * rounded)}px) scale(${Math.round((1 - tokens.detourPushShrink * rounded) * 1000) / 1000})`;
  }

  #renderNib(at: number, anchors: Anchors): void {
    const endedAt = this.#model.endedAt;
    if (endedAt !== null && at >= endedAt) {
      // The session ended: the pen rests with the orb.
      if (this.#lastNib !== "none") {
        this.#lastNib = "none";
        this.#nib.style.display = "none";
      }
      return;
    }
    const frame = nibAt(
      this.#model,
      this.#presence,
      at,
      this.mainTransform,
      anchors,
      this.#nibFrame,
    );
    let state: string;
    if (!frame.visible) {
      state = "none";
    } else {
      const pulse = frame.frozen ? 0.72 + 0.28 * Math.cos(((at - frame.frozenAt) / 1000) * 4) : 1;
      state = `${round1(frame.x)}|${round1(frame.y)}|${round2(frame.scale)}|${round2(pulse)}|${frame.down ? 1 : 0}`;
    }
    if (state === this.#lastNib) {
      return;
    }
    this.#lastNib = state;
    if (!frame.visible) {
      this.#nib.style.display = "none";
      return;
    }
    const pulse = frame.frozen ? 0.72 + 0.28 * Math.cos(((at - frame.frozenAt) / 1000) * 4) : 1;
    this.#nib.style.display = "block";
    this.#nib.style.transform = `translate(${round1(frame.x)}px, ${round1(frame.y)}px) scale(${round2(frame.scale)})`;
    this.#nib.style.opacity = String(round2(pulse));
    this.#nibGlow.style.opacity = frame.down ? "0.95" : "0.55";
  }
}
