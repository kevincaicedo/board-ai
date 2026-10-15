// The orb renderer: five blurred gradient blobs in a masked body, a soft glow, and the listening
// dot field (Orb.dc.html). Imperative DOM on the frame loop; the DOM is built once at the
// design's 236 px and scaled, so per frame only transforms, opacities and filters change, and
// each is written only when its value moves.

import { clamp, round2 } from "../canvas/math.ts";
import type { OrbParams } from "./dynamics.ts";

const SVG = "http://www.w3.org/2000/svg";
/** The size the orb's DOM is built at; placement scales it. */
const BUILD_SIZE = 236;
const FIELD_RINGS = 15;

interface Blob {
  /** Center, size and rotation in % and degrees, from the design. */
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  readonly rotation: number;
  readonly radius: number;
  readonly omega: number;
  readonly phase: number;
}

const BLOBS: readonly Blob[] = [
  { x: 28, y: 30, w: 64, h: 54, rotation: -20, radius: 8, omega: -0.45, phase: 4.4 },
  { x: 80, y: 48, w: 62, h: 80, rotation: 10, radius: 6, omega: 0.5, phase: 3.1 },
  { x: 64, y: 72, w: 54, h: 44, rotation: 20, radius: 9, omega: -0.8, phase: 1.7 },
  { x: 40, y: 60, w: 98, h: 30, rotation: -38, radius: 7, omega: 0.6, phase: 0 },
  { x: 54, y: 20, w: 58, h: 34, rotation: -8, radius: 5, omega: 0.35, phase: 2.2 },
];

function div(className: string, parent: HTMLElement): HTMLDivElement {
  const element = document.createElement("div");
  element.className = className;
  parent.append(element);
  return element;
}

/** Writes a style property only when its text changes. */
class StyleSlot {
  readonly #style: CSSStyleDeclaration;
  readonly #name: string;
  #last = "";

  constructor(element: HTMLElement | SVGElement, name: string) {
    this.#style = element.style;
    this.#name = name;
  }

  set(value: string): void {
    if (value !== this.#last) {
      this.#last = value;
      this.#style.setProperty(this.#name, value);
    }
  }
}

export interface OrbFrame {
  readonly x: number;
  readonly y: number;
  readonly size: number;
  readonly opacity: number;
  /** 0 to 1: the dot field fades out as the orb docks. */
  readonly fieldScale: number;
}

export class OrbView {
  readonly root: HTMLDivElement;
  readonly #rootTransform: StyleSlot;
  readonly #rootOpacity: StyleSlot;
  readonly #rootDisplay: StyleSlot;
  readonly #body: StyleSlot;
  readonly #bodyFilter: StyleSlot;
  readonly #glowTransform: StyleSlot;
  readonly #glowOpacity: StyleSlot;
  readonly #glowFilter: StyleSlot;
  readonly #layer: StyleSlot;
  readonly #blobs: StyleSlot[];
  readonly #field: SVGSVGElement;
  readonly #fieldDisplay: StyleSlot;
  readonly #rings: SVGCircleElement[] = [];
  readonly #ringRadius: number[] = [];

  constructor(parent: HTMLElement, label: string) {
    this.root = div("orb", parent);
    this.root.setAttribute("role", "img");
    this.root.setAttribute("aria-label", label);
    this.root.style.width = `${BUILD_SIZE}px`;
    this.root.style.height = `${BUILD_SIZE}px`;
    this.#rootTransform = new StyleSlot(this.root, "transform");
    this.#rootOpacity = new StyleSlot(this.root, "opacity");
    this.#rootDisplay = new StyleSlot(this.root, "display");

    const span = Math.ceil(BUILD_SIZE * 3.9);
    this.#field = document.createElementNS(SVG, "svg");
    this.#field.setAttribute("class", "orb-field");
    this.#field.setAttribute("width", String(span));
    this.#field.setAttribute("height", String(span));
    this.#field.setAttribute("viewBox", `0 0 ${span} ${span}`);
    this.#field.setAttribute("aria-hidden", "true");
    const offset = BUILD_SIZE / 2 - span / 2;
    this.#field.style.left = `${offset}px`;
    this.#field.style.top = `${offset}px`;
    this.root.append(this.#field);
    this.#fieldDisplay = new StyleSlot(this.#field, "display");
    const center = span / 2;
    const inner = BUILD_SIZE * 0.64;
    const outer = BUILD_SIZE * 1.9;
    const gap = Math.max(4, BUILD_SIZE * 0.058);
    for (let ring = 0; ring < FIELD_RINGS; ring += 1) {
      const radius = inner + ((outer - inner) * ring) / (FIELD_RINGS - 1);
      const circumference = 2 * Math.PI * radius;
      const dots = Math.max(12, Math.round(circumference / gap));
      const circle = document.createElementNS(SVG, "circle");
      circle.setAttribute("cx", String(center));
      circle.setAttribute("cy", String(center));
      circle.setAttribute("r", String(round2(radius)));
      circle.setAttribute("class", ring < 3 ? "orb-ring orb-ring-inner" : "orb-ring");
      circle.setAttribute("stroke-dasharray", `0 ${round2(circumference / dots)}`);
      this.#field.append(circle);
      this.#rings.push(circle);
      this.#ringRadius.push(center);
    }

    const glow = div("orb-glow", this.root);
    this.#glowTransform = new StyleSlot(glow, "transform");
    this.#glowOpacity = new StyleSlot(glow, "opacity");
    this.#glowFilter = new StyleSlot(glow, "filter");
    const body = div("orb-body", this.root);
    this.#body = new StyleSlot(body, "transform");
    this.#bodyFilter = new StyleSlot(body, "filter");
    div("orb-base", body);
    const layer = div("orb-layer", body);
    this.#layer = new StyleSlot(layer, "transform");
    const blur = Math.max(0.6, BUILD_SIZE * 0.075);
    this.#blobs = BLOBS.map((blob, index) => {
      const element = div(`orb-blob orb-blob-${index + 1}`, layer);
      element.style.left = `${blob.x}%`;
      element.style.top = `${blob.y}%`;
      element.style.width = `${blob.w}%`;
      element.style.height = `${blob.h}%`;
      element.style.filter = `blur(${round2(blur)}px)`;
      return new StyleSlot(element, "transform");
    });
  }

  render(frame: OrbFrame, params: OrbParams, still: boolean): void {
    if (frame.size < 0.5 || frame.opacity <= 0.001) {
      this.#rootDisplay.set("none");
      return;
    }
    this.#rootDisplay.set("block");
    const scale = frame.size / BUILD_SIZE;
    this.#rootTransform.set(
      `translate(${round2(frame.x - BUILD_SIZE / 2)}px, ${round2(frame.y - BUILD_SIZE / 2)}px) scale(${round2(scale * 1000) / 1000})`,
    );
    this.#rootOpacity.set(String(round2(frame.opacity)));

    const t = still ? 0 : params.clock;
    const phase = still ? 0 : params.phase;
    const voice = clamp(params.amp);
    const breathing = 1 + 0.028 * params.breath * Math.sin((t * Math.PI * 2) / 6);
    const pulse = 1 + 0.055 * params.pulse * voice;
    const swell = params.swell * breathing * pulse;
    const wobble = 0.035 * params.ripple * voice * Math.sin(t * 10.5);
    const filter = `hue-rotate(${round2(-16 * params.cool)}deg) saturate(${round2(Math.max(0.05, 1 - 0.25 * params.cool - 0.92 * params.mute))}) brightness(${round2(1 + 0.04 * params.cool + 0.14 * params.bright + 0.02 * params.mute)})`;
    this.#body.set(
      `translateX(${round2(params.lean * BUILD_SIZE * 0.03)}px) scale(${round2(swell * (1 + wobble) * 1000) / 1000}, ${round2(swell * (1 - wobble) * 1000) / 1000})`,
    );
    this.#bodyFilter.set(`${filter} blur(${round2(BUILD_SIZE * 0.006)}px)`);
    this.#glowOpacity.set(
      String(
        round2(
          clamp(
            0.85 * (1 - 0.75 * params.mute) + 0.6 * params.bright + 0.25 * params.pulse * voice,
          ),
        ),
      ),
    );
    this.#glowTransform.set(`scale(${round2(swell * (1 + 0.04 * params.ripple * voice))})`);
    this.#glowFilter.set(filter);
    this.#layer.set(`rotate(${round2(phase * 24)}deg)`);
    for (let index = 0; index < BLOBS.length; index += 1) {
      const blob = BLOBS[index];
      const slot = this.#blobs[index];
      if (blob === undefined || slot === undefined) {
        continue;
      }
      const angle = phase * blob.omega + blob.phase;
      const radius =
        blob.radius * (1 - 0.5 * params.swirl) +
        3 * params.ripple * voice * Math.sin(t * 8 + blob.phase);
      let x = blob.x + radius * Math.cos(angle);
      let y = blob.y + radius * Math.sin(angle);
      x += (50 - x) * 0.22 * params.swirl;
      y += (50 - y) * 0.22 * params.swirl;
      const dx = ((x - blob.x) / 100) * BUILD_SIZE;
      const dy = ((y - blob.y) / 100) * BUILD_SIZE;
      slot.set(
        `translate(-50%, -50%) translate(${round2(dx)}px, ${round2(dy)}px) rotate(${round2(blob.rotation + phase * blob.omega * 18)}deg)`,
      );
    }
    this.#renderField(clamp(params.field * frame.fieldScale), voice, t);
  }

  #renderField(field: number, voice: number, t: number): void {
    if (field <= 0.01) {
      this.#fieldDisplay.set("none");
      return;
    }
    this.#fieldDisplay.set("block");
    const energy = 0.35 + 0.65 * voice;
    for (let ring = 0; ring < this.#rings.length; ring += 1) {
      const circle = this.#rings[ring];
      const center = this.#ringRadius[ring] ?? 0;
      if (circle === undefined) {
        continue;
      }
      const wave = 0.5 + 0.5 * Math.sin(ring * 0.9 - t * 6.5);
      const width =
        BUILD_SIZE * 0.021 * (0.35 + 0.95 * wave * energy) * (1 - (0.35 * ring) / FIELD_RINGS);
      const opacity = field * (1 - ring / FIELD_RINGS) ** 1.15 * (0.5 + 0.5 * wave);
      circle.setAttribute("stroke-width", String(round2(width)));
      circle.setAttribute("opacity", String(round2(opacity)));
      circle.setAttribute(
        "transform",
        `rotate(${round2((ring % 2 ? 1 : -1) * t * 3 + ring * 7)} ${center} ${center})`,
      );
    }
  }
}
