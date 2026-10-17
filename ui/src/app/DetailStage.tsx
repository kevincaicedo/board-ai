// Drill-down (15b, D-188): each detail layer is a full-stage card over the board, grown out of
// the part it drills into. Its box eases from the anchor's box on screen to the board's area over
// a scene duration and back when it closes; a detail opened inside it covers it, and the one
// under steps back as the board does. The layer draws into the card's host one unit per pixel
// (the runtime sizes it with the detour cards).

import { useLayoutEffect, useMemo, useRef } from "preact/hooks";
import { type Box, boxAt, detailBox, detailOpen, screenBox } from "../canvas/detail.ts";
import { clamp, lerp, round1, round2 } from "../canvas/math.ts";
import type { DetailView } from "./diagrams.ts";
import {
  type NumberFormat,
  numberWriter,
  plain,
  px,
  styleWriter,
  useFrame,
  useRuntime,
} from "./hooks.ts";

/** The card's contents fade in over the first part of its growth. */
const CONTENT_IN_SHARE = 0.35;

/** A step back's blur in pixels, `none` at 0. */
const blurFilter: NumberFormat = (blur) => (blur === 0 ? "none" : `blur(${blur}px)`);

/** A step back's scale, `none` at 1. */
const scaleTransform: NumberFormat = (scale) => (scale === 1 ? "none" : `scale(${scale})`);

function emptyBox(): Box {
  return { x: 0, y: 0, w: 0, h: 0 };
}

function DetailCard({
  detail,
  cardRef,
}: {
  detail: DetailView;
  cardRef: (layer: number, element: HTMLElement | null) => void;
}) {
  const runtime = useRuntime();
  const hostRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    runtime.attachLayerHost(detail.layer, hostRef.current);
    return () => runtime.attachLayerHost(detail.layer, null);
  }, [runtime, detail.layer]);
  const edge = detail.anchor.category?.toLowerCase() ?? "plain";
  return (
    <section
      class={`detail-card detail-edge-${edge}`}
      aria-label={`Detail layer: ${detail.title}`}
      ref={(element) => cardRef(detail.layer, element)}
    >
      <div class="detail-host" ref={hostRef} />
    </section>
  );
}

export function DetailStage() {
  const runtime = useRuntime();
  const details = runtime.scene.diagrams.details.value;
  const cards = useRef(new Map<number, HTMLElement>());
  const write = useMemo(styleWriter, []);
  const writeNumber = useMemo(numberWriter, []);
  const boxes = useMemo(() => ({ from: emptyBox(), to: emptyBox(), now: emptyBox() }), []);
  const cardRef = useMemo(
    () => (layer: number, element: HTMLElement | null) => {
      if (element === null) {
        cards.current.delete(layer);
      } else {
        cards.current.set(layer, element);
      }
    },
    [],
  );

  useFrame((at) => {
    const tokens = runtime.tokens;
    const duration = tokens.durationSceneMs;
    detailBox(runtime.stage, tokens, boxes.to);
    for (let index = 0; index < details.length; index += 1) {
      const detail = details[index];
      const card = detail === undefined ? undefined : cards.current.get(detail.layer);
      if (detail === undefined || card === undefined) {
        continue;
      }
      const open = detailOpen(detail.openAt, detail.closedAt, at, duration);
      const shown = at >= detail.openAt && open > 0.001;
      write(card, "display", shown ? "block" : "none");
      if (!shown) {
        continue;
      }
      anchorBox(detail, boxes.to, boxes.from);
      boxAt(boxes.from, boxes.to, open, boxes.now);
      // Numbers, compared before any text is built: a settled card writes nothing.
      writeNumber(card, "left", round1(boxes.now.x), px);
      writeNumber(card, "top", round1(boxes.now.y), px);
      writeNumber(card, "width", round1(boxes.now.w), px);
      writeNumber(card, "height", round1(boxes.now.h), px);
      const radius = lerp(tokens.diagramFrameRadius, tokens.detailCardRadius, open);
      writeNumber(card, "border-radius", round1(radius), px);
      // A detail opened inside this one covers it: this one steps back as the board does.
      let covered = 0;
      for (let later = index + 1; later < details.length; later += 1) {
        const inner = details[later];
        if (inner !== undefined && inner.anchor.layer === detail.layer) {
          covered = Math.max(covered, detailOpen(inner.openAt, inner.closedAt, at, duration));
        }
      }
      const fade = clamp(open / CONTENT_IN_SHARE) * (1 - tokens.detailPushFade * covered);
      writeNumber(card, "opacity", round2(fade), plain);
      const stepped = covered >= 0.001;
      writeNumber(
        card,
        "filter",
        stepped ? round1(tokens.detailPushBlur * covered) : 0,
        blurFilter,
      );
      const scale = stepped ? round2(1 - tokens.detailPushShrink * covered) : 1;
      writeNumber(card, "transform", scale, scaleTransform);
    }
  });

  /**
   * Where the detail's anchor is on the stage now: on the main board, or in an outer detail's
   * card (whose layer draws one unit per pixel of the full card). Inside a detour's card in the
   * column, the card grows from the middle of the board's area instead.
   */
  function anchorBox(detail: DetailView, card: Box, out: Box): void {
    const parent = detail.anchor.layer;
    const transform = runtime.board?.layerTransform(parent) ?? null;
    const inDetail = runtime.scene.board.layers.get(parent)?.anchor != null;
    if (transform === null || (parent !== 0 && !inDetail)) {
      out.x = card.x + card.w / 2;
      out.y = card.y + card.h / 2;
      out.w = 0;
      out.h = 0;
      return;
    }
    screenBox(detail.anchor.rect, transform, out);
    if (inDetail) {
      out.x += card.x;
      out.y += card.y;
    }
  }

  return (
    <div class="details">
      {details.map((detail) => (
        <DetailCard key={`detail-${detail.layer}`} detail={detail} cardRef={cardRef} />
      ))}
    </div>
  );
}
