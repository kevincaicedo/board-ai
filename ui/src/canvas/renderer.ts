// The canvas renderer: plain imperative SVG on the frame loop (LLD §4). One <path> per stroke and
// one per hachure fill, revealed through stroke-dashoffset; handwriting is two <tspan>s whose
// split moves letter by letter. Nothing is allocated for strokes that are already done, and an
// attribute is written only when its value changes.

import type { YoursView } from "../draw/inkView.ts";
import type { PointerTarget } from "../protocol/generated/PointerTarget.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { Tokens } from "../styles/tokens.ts";
import {
  type BoardModel,
  type Layer,
  LEAVING_PAGE,
  type Run,
  runActive,
  traceRunning,
} from "./board.ts";
import { cameraAt, depthOfField, type FitArea, type Viewport } from "./camera.ts";
import type { Box } from "./detail.ts";
import type { DetourCard } from "./detourCard.ts";
import type { Vec } from "./geometry.ts";
import { createStroke, createText, type ItemView, svg } from "./items.ts";
import { clamp, easeInOutCubic, easeOut, ramp, round1, round2 } from "./math.ts";
import { type Anchors, type NibFrame, nibAt, type Presence, type Transform } from "./presence.ts";
import { boardPush, writePush } from "./push.ts";
import { TARGET_PASSES, targetPass } from "./staged.ts";
import {
  lettersAt,
  pointAtLength,
  revealLength,
  revealLengthStepped,
  textOffset,
} from "./stroke.ts";
import { naturalTime } from "./timeline.ts";

const SVG = "http://www.w3.org/2000/svg";
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
  | "diagramCameraScaleMax"
  | "detailPushBlur"
  | "detailPushFade"
  | "detailPushShrink"
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

const HIDDEN = 0;
const PARTIAL = 1;
const DONE = 2;

interface RunView {
  readonly run: Run;
  readonly group: SVGGElement;
  /**
   * The run's strokes, under its pointer targets. An item's element is made the first frame it
   * shows, so a reveal of a whole section spreads its DOM work over the frames that draw it.
   */
  readonly strokes: SVGGElement;
  /** Handwriting sits above every stroke on the layer, as in the design, so fills never cross it. */
  readonly labels: SVGGElement;
  readonly items: (ItemView | null)[];
  readonly states: Int8Array;
  readonly progress: Float64Array;
  readonly trace: { readonly dot: SVGGElement; readonly at: Vec } | null;
  /** The elements inside the drawing (a reveal's parts), so a focus on one keeps it sharp. */
  readonly targets: ReadonlySet<string>;
  done: boolean;
  soft: boolean;
}

/** The nib's written state: shown, x, y, scale, pulse, pen down. */
const NIB_STATE_SIZE = 6;

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
  /**
   * A detour layer's viewBox last written, as its card's width and height: one unit per CSS
   * pixel (D-151). Kept as numbers, so a frame whose card didn't change builds no string.
   */
  readonly lastViewBox: Float64Array;
  /** How far a leaving page had gone when last written (D-177); NaN forces the next write. */
  lastLeave: number;
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
  /**
   * The push-back and the end fade last written, so an unchanged board writes nothing; numbers,
   * so a still frame builds no string. NaN forces the first write, so a reset never leaves a
   * stale push-back on the board.
   */
  readonly #lastPush = Float64Array.of(Number.NaN, Number.NaN, Number.NaN, Number.NaN, Number.NaN);
  /** The push-back this frame, before it is compared with the last one written. */
  readonly #push = new Float64Array(5);
  /** Where a detail's anchor sits on the stage this frame, for the board's step back. */
  readonly #anchorBox: Box = { x: 0, y: 0, w: 0, h: 0 };
  /** The nib last written: shown (1 or 0), x, y, scale, pulse, pen down (1 or 0). */
  readonly #lastNib = new Float64Array(NIB_STATE_SIZE).fill(Number.NaN);
  #hovered: { layer: number; key: string } | null = null;
  /** The hand dash every dashed stroke without one of its own is drawn with. */
  readonly #handDash: string;

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
    this.#handDash = `${options.tokens.handDash} ${options.tokens.handDashGap}`;
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

  /** A layer's world → layer-view transform as last rendered, or null before its first frame. */
  layerTransform(layer: number): Transform | null {
    return this.#layers.get(layer)?.transform ?? null;
  }

  reset(): void {
    for (const view of this.#layers.values()) {
      view.svg.remove();
    }
    this.#layers.clear();
    this.#syncedVersion = -1;
    this.#lastPush.fill(Number.NaN);
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
      if (view.id === LEAVING_PAGE) {
        this.#renderLeaving(view, layer, at, stage);
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
    const onStage = id === 0 || id === LEAVING_PAGE;
    root.setAttribute("class", onStage ? "board-svg" : "board-svg board-svg-detour");
    root.setAttribute("aria-hidden", "true");
    if (!onStage) {
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
    const host = id === LEAVING_PAGE ? this.#mainHost : (this.#hosts.get(id) ?? null);
    if (host !== null) {
      if (onStage) {
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
      lastViewBox: Float64Array.of(Number.NaN, Number.NaN),
      lastLeave: Number.NaN,
    };
  }

  #createRun(layer: LayerView, run: Run): RunView {
    const group = svg("g", layer.strokes, "element");
    const strokes = svg("g", group);
    const labels = svg("g", layer.labels);
    const items: (ItemView | null)[] = new Array(run.plan.items.length).fill(null);
    let trace: RunView["trace"] = null;
    if (run.trace !== null) {
      const dot = svg("g", group, "trace-dot");
      svg("circle", dot, "trace-halo").setAttribute("r", "9");
      svg("circle", dot, "trace-core").setAttribute("r", "5");
      dot.style.display = "none";
      trace = { dot, at: [0, 0] };
    } else if (run.op.kind === "Reveal") {
      // A reveal names no element: each part is a target of its own. Frames go under edges and
      // edges under everything else, so pointing inside a frame names what is there.
      for (const pass of TARGET_PASSES) {
        for (const target of run.plan.targets) {
          if (targetPass(run.op.parts, target.id) === pass) {
            this.#hit(group, layer, target.rect, target.id, 0);
          }
        }
      }
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
      strokes,
      labels,
      items,
      states: new Int8Array(items.length),
      progress: new Float64Array(items.length).fill(-1),
      trace,
      targets: new Set(run.plan.targets.map((target) => target.id)),
      done: false,
      soft: false,
    };
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
    const last = view.lastViewBox;
    if (card.width !== last[0] || card.height !== last[1]) {
      last[0] = card.width;
      last[1] = card.height;
      view.svg.setAttribute("viewBox", `0 0 ${card.width} ${card.height}`);
    }
  }

  #renderLayer(view: LayerView, layer: Layer, at: number, area: FitArea): void {
    const tokens = this.#options.tokens;
    cameraAt(
      layer.camera,
      at,
      area,
      tokens.durationSceneMs,
      view.transform,
      tokens.diagramCameraScaleMax,
    );
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
        dof > 0.001 &&
        focus !== null &&
        runView.run.key !== focus &&
        !runView.targets.has(focus) &&
        runView.run.trace === null;
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

  /**
   * The page a page turn sent away (D-177): it keeps its camera and lifts off to the left,
   * blurring and fading as a dismissed card does, while the fresh page starts under it. The
   * board's own push-back and end fade still apply around both.
   */
  #renderLeaving(view: LayerView, layer: Layer, at: number, stage: FitArea): void {
    const tokens = this.#options.tokens;
    const turnedAt = layer.poppedAt ?? at;
    const gone = easeOut(ramp(at, turnedAt, turnedAt + tokens.durationSceneMs));
    const style = view.svg.style;
    if (gone >= 1) {
      if (style.display !== "none") {
        style.display = "none";
        view.lastLeave = Number.NaN;
      }
      return;
    }
    this.#renderLayer(view, layer, at, stage);
    // Only a changed amount is written, as the push-back does, so a still frame writes nothing.
    const amount = Math.round(gone * 1000) / 1000;
    if (amount === view.lastLeave) {
      return;
    }
    view.lastLeave = amount;
    const shift = round1(-tokens.detourPushShift * 2 * amount);
    style.display = "";
    style.opacity = String(round2(1 - amount));
    style.filter = `blur(${round1(amount * tokens.surfaceBlurOut)}px)`;
    style.transform = `translate(${shift}px, ${round1(shift / 4)}px)`;
  }

  #renderRun(view: RunView, at: number): void {
    const run = view.run;
    const natural = naturalTime(run.clock, at);
    const local = natural - run.leadMs;
    const { starts, ends } = run.plan;
    let finished = natural >= run.clock.naturalMs;
    for (let index = 0; index < view.items.length; index += 1) {
      const start = starts[index] ?? 0;
      const end = ends[index] ?? 0;
      if (natural < 0 || local < start) {
        finished = false;
        const hidden = view.items[index];
        if (hidden !== null && hidden !== undefined) {
          this.#setState(view, index, hidden, HIDDEN);
        }
        continue;
      }
      const itemView = view.items[index] ?? this.#createItem(view, index);
      if (itemView === null) {
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

  /** The element of item `index`, made the first frame it shows (it starts hidden). */
  #createItem(view: RunView, index: number): ItemView | null {
    const item = view.run.plan.items[index];
    if (item === undefined) {
      return null;
    }
    const created =
      item.type === "stroke"
        ? createStroke(view.strokes, item, this.#handDash)
        : createText(view.labels, item);
    view.items[index] = created;
    return created;
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
    if ("path" in item && item.item.kind === "fill") {
      const opacity = this.#options.reducedMotion ? 1 : round2(f);
      if (view.progress[index] !== opacity) {
        view.progress[index] = opacity;
        item.path.style.opacity = String(opacity);
      }
      return;
    }
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
   * The board steps back while a detour or a detail layer is open (frame 4, 15b), and fades out
   * for good when the session ends (D-145). Only a changed amount is written.
   */
  #renderPush(at: number): void {
    const now = boardPush(
      this.#model,
      at,
      this.#options.tokens,
      this.#options.yours?.lifted === true,
      this.mainTransform,
      this.#anchorBox,
      this.#push,
    );
    const last = this.#lastPush;
    const same =
      now[0] === last[0] &&
      now[1] === last[1] &&
      now[2] === last[2] &&
      (now[2] === 0 || (now[3] === last[3] && now[4] === last[4]));
    if (!same) {
      last.set(now);
      writePush(this.#mainHost.style, now, this.#options.tokens);
    }
  }

  #renderNib(at: number, anchors: Anchors): void {
    const endedAt = this.#model.endedAt;
    if (endedAt !== null && at >= endedAt) {
      // The session ended: the pen rests with the orb.
      if (this.#lastNib[0] !== 0) {
        this.#lastNib.fill(Number.NaN);
        this.#lastNib[0] = 0;
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
    const last = this.#lastNib;
    if (!frame.visible) {
      if (last[0] !== 0) {
        last.fill(Number.NaN);
        last[0] = 0;
        this.#nib.style.display = "none";
      }
      return;
    }
    const x = round1(frame.x);
    const y = round1(frame.y);
    const scale = round2(frame.scale);
    const pulse = round2(
      frame.frozen ? 0.72 + 0.28 * Math.cos(((at - frame.frozenAt) / 1000) * 4) : 1,
    );
    const down = frame.down ? 1 : 0;
    if (
      last[0] === 1 &&
      last[1] === x &&
      last[2] === y &&
      last[3] === scale &&
      last[4] === pulse &&
      last[5] === down
    ) {
      return;
    }
    last[0] = 1;
    last[1] = x;
    last[2] = y;
    last[3] = scale;
    last[4] = pulse;
    last[5] = down;
    this.#nib.style.display = "block";
    this.#nib.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
    this.#nib.style.opacity = String(pulse);
    this.#nibGlow.style.opacity = frame.down ? "0.95" : "0.55";
  }
}
