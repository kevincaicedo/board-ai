// What the stage shows about staged diagrams and drill-down (canvas 15 and 15b, D-187, D-188):
// each layer's diagram header from `Diagram` events, and the detail layers from `LayerPush` with
// an anchor, as signals for the header and the detail cards. The board model draws the layers;
// this store only holds what the stage's chrome reads.

import { signal } from "@preact/signals";
import type { DetailAnchor } from "../protocol/generated/DetailAnchor.ts";
import type { DiagramProvenance } from "../protocol/generated/DiagramProvenance.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";

/** Levels a drill-down can reach: the main board plus the core's detour depth (3). */
export const DETAIL_LEVELS_MAX = 4;

/** Detail layers kept after they closed, for their exit; older ones are let go. */
const CLOSED_DETAILS_KEPT = DETAIL_LEVELS_MAX;

export interface DiagramHeader {
  readonly layer: number;
  readonly title: string;
  readonly part: number;
  readonly parts: number;
  /** Where it came from when a research brief drew it (D-242). */
  readonly provenance: DiagramProvenance | null;
  readonly at: number;
}

export interface DetailView {
  readonly layer: number;
  readonly title: string;
  readonly anchor: DetailAnchor;
  readonly openAt: number;
  readonly closedAt: number | null;
}

/** The header line the stage shows (15: "title · part 3 of 4"; 15b: the breadcrumbs). */
export type HeaderView =
  | {
      readonly kind: "diagram";
      readonly title: string;
      /** "part 3 of 4", or null before any part is drawn. */
      readonly part: string | null;
      /** The chip above a brief's diagram ("from Claude Code's brief · 23 files read"). */
      readonly credit: string | null;
    }
  | {
      readonly kind: "detail";
      /** The path, outermost first; the last is the layer in front. */
      readonly crumbs: readonly string[];
      /** "depth 2 of 4". */
      readonly depth: string;
      /** The chip for the diagram on the layer in front, when a brief drew it. */
      readonly credit: string | null;
    };

/** The root of a drill-down's path when the main board has no staged diagram. */
export const MAIN_BOARD_CRUMB = "Main board";

/**
 * The chip's words: who read the code, and how much (D-242). A brief from the cache was read in
 * an earlier session; a brief that read no file says only whose it is.
 */
export function creditLine(provenance: DiagramProvenance | null): string | null {
  if (provenance === null) {
    return null;
  }
  const whose = `from ${provenance.reader}'s brief`;
  const read = provenance.files_read;
  if (read === null) {
    return `${whose} · read earlier`;
  }
  if (read === 0) {
    return whose;
  }
  return `${whose} · ${read === 1 ? "1 file" : `${read} files`} read`;
}

/** The header for these diagrams and open details, or null when there is nothing to say. */
export function headerView(
  headers: readonly DiagramHeader[],
  details: readonly DetailView[],
): HeaderView | null {
  const open = details.filter((detail) => detail.closedAt === null);
  const main = headers.find((header) => header.layer === 0) ?? null;
  if (open.length > 0) {
    const crumbs = [main?.title ?? MAIN_BOARD_CRUMB, ...open.map((detail) => detail.title)];
    const front = open.at(-1)?.layer;
    const inFront = headers.find((header) => header.layer === front) ?? null;
    return {
      kind: "detail",
      crumbs,
      depth: `depth ${Math.min(open.length + 1, DETAIL_LEVELS_MAX)} of ${DETAIL_LEVELS_MAX}`,
      credit: creditLine(inFront?.provenance ?? null),
    };
  }
  if (main === null) {
    return null;
  }
  return {
    kind: "diagram",
    title: main.title,
    part: main.part === 0 ? null : `part ${main.part} of ${main.parts}`,
    credit: creditLine(main.provenance),
  };
}

export class DiagramStore {
  readonly headers = signal<readonly DiagramHeader[]>([]);
  readonly details = signal<readonly DetailView[]>([]);

  reset(): void {
    this.headers.value = [];
    this.details.value = [];
  }

  apply(event: UiEvent, at: number): void {
    switch (event.type) {
      case "Diagram": {
        const header: DiagramHeader = {
          layer: event.layer,
          title: event.title,
          part: event.part,
          parts: event.parts,
          provenance: event.provenance,
          at,
        };
        this.headers.value = [
          ...this.headers.value.filter((existing) => existing.layer !== event.layer),
          header,
        ];
        break;
      }
      case "LayerPush":
        if (event.anchor !== null) {
          const kept = this.details.value.filter(
            (detail) => detail.layer !== event.layer && detail.closedAt === null,
          );
          const closed = this.details.value
            .filter((detail) => detail.layer !== event.layer && detail.closedAt !== null)
            .slice(-CLOSED_DETAILS_KEPT);
          this.details.value = [
            ...closed,
            ...kept,
            {
              layer: event.layer,
              title: event.title,
              anchor: event.anchor,
              openAt: at,
              closedAt: null,
            },
          ];
        }
        // A new layer starts without the header a closed one with its id had.
        this.#dropHeader(event.layer);
        break;
      case "LayerPop":
        this.details.value = this.details.value.map((detail) =>
          detail.layer === event.layer && detail.closedAt === null
            ? { ...detail, closedAt: at }
            : detail,
        );
        this.#dropHeader(event.layer);
        break;
      // A fresh page has no diagram yet (D-177); the detours it closed came first.
      case "PageTurn":
        this.#dropHeader(0);
        break;
      case "SessionEnd":
        this.headers.value = [];
        this.details.value = this.details.value.map((detail) =>
          detail.closedAt === null ? { ...detail, closedAt: at } : detail,
        );
        break;
      case "Orb":
      case "Acknowledge":
      case "Caption":
      case "Thread":
      case "OpStart":
      case "Freeze":
      case "Resume":
      case "Camera":
      case "SurfaceShow":
      case "SurfaceUpdate":
      case "SurfaceDismiss":
      case "Chip":
      case "Problem":
      case "Suggestion":
      case "SessionStart":
      case "ViewShow":
      case "ViewDismiss":
      case "OpenSettings":
      case "DrawMode":
      case "UserShape":
      case "UserShapeRemove":
      case "LabelHint":
        break;
    }
  }

  #dropHeader(layer: number): void {
    if (this.headers.value.some((header) => header.layer === layer)) {
      this.headers.value = this.headers.value.filter((header) => header.layer !== layer);
    }
  }
}
