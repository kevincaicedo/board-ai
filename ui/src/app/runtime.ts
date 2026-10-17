// The UI runtime: one requestAnimationFrame loop that pulls session time from the transport and
// renders the canvas, the orb and every time-driven surface style from it. It pauses while the
// window is hidden (PRODUCT §8.6: the orb rests when nobody can see it).

import { createContext } from "preact";
import { emptyArea, type FitArea, stageArea, type Viewport } from "../canvas/camera.ts";
import {
  cardBounds,
  columnWidth,
  type DetourCard,
  detourCardAt,
  emptyCard,
  type Size,
} from "../canvas/detourCard.ts";
import type { Vec } from "../canvas/geometry.ts";
import { type OrbPlacement, orbPlacement } from "../canvas/presence.ts";
import { BoardView } from "../canvas/renderer.ts";
import { emptyParams } from "../orb/dynamics.ts";
import { OrbView } from "../orb/orb.ts";
import type { PointerTarget } from "../protocol/generated/PointerTarget.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { ReplayTransport } from "../transport/replay.ts";
import type { Transport } from "../transport/transport.ts";
import { type CaptionPlace, captionLetters, orbSpot, placeForView } from "../views/layout.ts";
import { Replays } from "./replays.ts";
import { Scene } from "./scene.ts";
import { type ViewDurations, viewMotion, viewVisible } from "./views.ts";

export type FrameCallback = (at: number) => void;

/** Frames measured for the script-time budget (LLD §4: at most 4 ms per frame). */
const FRAME_STATS_WINDOW = 120;

export interface FrameStats {
  frames: number;
  /** Worst frame among the last FRAME_STATS_WINDOW. */
  worstMs: number;
  meanMs: number;
  /** Worst frame since the page loaded, the first frame (which builds a seeked board) included. */
  worstEverMs: number;
}

export class Runtime {
  readonly transport: Transport;
  readonly replay: ReplayTransport | null;
  readonly scene: Scene;
  readonly tokens: Tokens;
  readonly reducedMotion: boolean;
  /** Replays opened from a card, and how far each was watched (UI navigation, not core state). */
  readonly replays = new Replays();
  readonly viewDurations: ViewDurations;
  #callbacks = new Set<FrameCallback>();
  #stageElement: HTMLElement | null = null;
  readonly #captionPlace: CaptionPlace = { shift: 0, width: 0 };
  #lastCaptionPlace = "";
  #board: BoardView | null = null;
  #orb: OrbView | null = null;
  #raf = 0;
  #running = false;
  #at = 0;
  #generation = -1;
  #stage: Viewport = { width: 0, height: 0 };
  #card: Viewport = { width: 0, height: 0 };
  /** Where the main layer fits its camera frame; recomputed on resize, never per frame. */
  readonly #stageArea: FitArea = emptyArea();
  /** Frame 4's detour card for this stage, and the largest a detour card may grow (D-151). */
  readonly #cardBase: Size = { width: 0, height: 0 };
  readonly #cardMax: Size = { width: 0, height: 0 };
  /** Each open detour layer's card this frame: its size and where its drawing fits. */
  readonly detourCards = new Map<number, DetourCard>();
  #anchors: { center: Vec; dock: Vec } = { center: [0, 0], dock: [0, 0] };
  #placement: OrbPlacement = { x: 0, y: 0, size: 0, opacity: 0, docked: 0, condense: 0 };
  #params = emptyParams();
  #lastPointer = "";
  readonly #frameCosts = new Float64Array(FRAME_STATS_WINDOW);
  #frames = 0;
  #worstEverMs = 0;
  #resize: ResizeObserver | null = null;

  constructor(
    transport: Transport,
    replay: ReplayTransport | null,
    tokens: Tokens,
    reducedMotion: boolean,
  ) {
    this.transport = transport;
    this.replay = replay;
    this.tokens = tokens;
    this.reducedMotion = reducedMotion;
    this.scene = new Scene(tokens);
    this.#card = { width: tokens.surfaceWidth, height: tokens.detourCardHeight };
    this.viewDurations = { sceneMs: tokens.durationSceneMs, standardMs: tokens.durationStandardMs };
  }

  /** The stage element, once mounted (views write their layout variables on it). */
  get stageElement(): HTMLElement | null {
    return this.#stageElement;
  }

  /** The session time of the last rendered frame. */
  get at(): number {
    return this.#at;
  }

  get board(): BoardView | null {
    return this.#board;
  }

  mount(stage: HTMLElement, boardHost: HTMLElement, orbHost: HTMLElement): () => void {
    this.#stageElement = stage;
    this.#board = new BoardView(this.scene.board, this.scene.presence, boardHost, {
      tokens: this.tokens,
      card: this.#card,
      reducedMotion: this.reducedMotion,
      onPointer: (target) => this.pointer(target),
    });
    this.#orb = new OrbView(orbHost, "BoardAI");
    this.#measure(stage);
    this.#resize = new ResizeObserver(() => this.#measure(stage));
    this.#resize.observe(stage);
    const visibility = (): void => {
      if (document.hidden) {
        this.#stop();
      } else {
        this.#start();
      }
    };
    document.addEventListener("visibilitychange", visibility);
    void this.transport.connect(this.scene);
    this.#start();
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      this.#resize?.disconnect();
      this.#stop();
      this.transport.close();
    };
  }

  /** Registers a per-frame callback; it also runs at once so a new element never flashes. */
  onFrame(callback: FrameCallback): () => void {
    this.#callbacks.add(callback);
    callback(this.#at);
    return () => this.#callbacks.delete(callback);
  }

  send(command: UiCommand): void {
    this.transport.send(command);
  }

  /** Pointer targets go out only when they change (LLD §2.2 d: a recent target is a cue). */
  pointer(target: PointerTarget | null): void {
    const key = JSON.stringify(target);
    if (key === this.#lastPointer) {
      return;
    }
    this.#lastPointer = key;
    this.send({ type: "Pointer", target });
  }

  attachLayerHost(layer: number, host: HTMLElement | null): void {
    this.#board?.attachLayerHost(layer, host);
  }

  get stage(): Viewport {
    return this.#stage;
  }

  #measure(stage: HTMLElement): void {
    this.#stage = { width: stage.clientWidth, height: stage.clientHeight };
    const tokens = this.tokens;
    stageArea(this.#stage, tokens, this.#stageArea);
    cardBounds(this.#stage, tokens, this.#cardBase, this.#cardMax);
    this.#anchors = {
      center: [this.#stage.width / 2, this.#stage.height / 2 - tokens.orbCenterLift],
      dock: [this.#stage.width / 2, this.#stage.height - tokens.orbDockBottom],
    };
  }

  #start(): void {
    if (this.#running) {
      return;
    }
    this.#running = true;
    const tick = (pageNow: number): void => {
      if (!this.#running) {
        return;
      }
      this.#frame(pageNow);
      this.#raf = requestAnimationFrame(tick);
    };
    this.#raf = requestAnimationFrame(tick);
  }

  #stop(): void {
    this.#running = false;
    cancelAnimationFrame(this.#raf);
  }

  #frame(pageNow: number): void {
    const begin = performance.now();
    const at = this.transport.sessionTime(pageNow);
    this.#at = at;
    const scene = this.scene;
    if (scene.generation !== this.#generation) {
      this.#generation = scene.generation;
      this.#board?.reset();
    }
    this.#sizeDetourCards(at);
    this.#board?.render(at, this.#stageArea, this.detourCards, this.#anchors);
    orbPlacement(scene.board, scene.presence, at, this.#anchors, this.#placement);
    this.#placeForViews(at);
    const freezeAt = scene.presence.lastFreezeAt(at);
    const acknowledgeAt = scene.presence.lastAcknowledgeAt;
    scene.orb.sample(
      at,
      freezeAt,
      acknowledgeAt !== null && acknowledgeAt <= at ? acknowledgeAt : null,
      this.#params,
    );
    this.#params.bright += this.#placement.condense;
    this.#orb?.render(
      {
        x: this.#placement.x,
        y: this.#placement.y,
        size: this.#placement.size,
        opacity: this.#placement.opacity,
        fieldScale: 1 - this.#placement.docked,
      },
      this.#params,
      this.reducedMotion,
    );
    for (const callback of this.#callbacks) {
      callback(at);
    }
    this.#record(performance.now() - begin);
  }

  /** The column's width on this stage: a detour card grows leftward from its right edge. */
  get columnWidth(): number {
    return columnWidth(this.#stage, this.tokens);
  }

  /** Each detour layer's card for this frame, from its camera (reusing the objects). */
  #sizeDetourCards(at: number): void {
    const layers = this.scene.board.layers;
    for (const id of this.detourCards.keys()) {
      if (!layers.has(id)) {
        this.detourCards.delete(id);
      }
    }
    for (const layer of layers.values()) {
      if (layer.id === 0) {
        continue;
      }
      let card = this.detourCards.get(layer.id);
      if (card === undefined) {
        card = emptyCard();
        this.detourCards.set(layer.id, card);
      }
      detourCardAt(layer.camera, at, this.#cardBase, this.#cardMax, this.tokens, card);
    }
  }

  /** Open views move the orb (docked, or beside the panels) and the captions with it. */
  #placeForViews(at: number): void {
    const place = this.#captionPlace;
    place.shift = 0;
    place.width = this.tokens.captionWidth;
    for (const entry of this.scene.views.entries.value) {
      if (!viewVisible(entry, at, this.viewDurations)) {
        continue;
      }
      const motion = viewMotion(entry, at, this.viewDurations);
      placeForView(
        orbSpot(entry.view.kind, this.#stage, this.tokens),
        motion.enter * (1 - motion.exit),
        this.#stage,
        this.#anchors.dock,
        this.tokens,
        this.#placement,
        place,
      );
    }
    const key = `${Math.round(place.shift * 10) / 10}|${Math.round(place.width)}`;
    if (key !== this.#lastCaptionPlace && this.#stageElement !== null) {
      this.#lastCaptionPlace = key;
      this.#stageElement.style.setProperty(
        "--view-caption-shift",
        `${Math.round(place.shift * 10) / 10}px`,
      );
      this.#stageElement.style.setProperty("--view-caption-width", `${Math.round(place.width)}px`);
    }
  }

  /** How many letters a caption line may hold now (an open panel leaves the captions less room). */
  get captionLetters(): number {
    return captionLetters(this.#captionPlace.width, this.tokens);
  }

  /** The orb's live parameters for this frame (the mic ring follows the voice level). */
  get voiceLevel(): number {
    return this.#params.amp;
  }

  #record(costMs: number): void {
    this.#frameCosts[this.#frames % FRAME_STATS_WINDOW] = costMs;
    this.#frames += 1;
    this.#worstEverMs = Math.max(this.#worstEverMs, costMs);
  }

  /** Script time of the last frames, for the budget check (read on demand, not per frame). */
  get stats(): FrameStats {
    const count = Math.min(this.#frames, FRAME_STATS_WINDOW);
    let worst = 0;
    let sum = 0;
    for (let index = 0; index < count; index += 1) {
      const cost = this.#frameCosts[index] ?? 0;
      worst = Math.max(worst, cost);
      sum += cost;
    }
    return {
      frames: this.#frames,
      worstMs: worst,
      meanMs: count === 0 ? 0 : sum / count,
      worstEverMs: this.#worstEverMs,
    };
  }
}

export const RuntimeContext = createContext<Runtime | null>(null);
