// A small line drawing of a board, for the digest card: each node outlined in ink over its tone.

import { useMemo } from "preact/hooks";
import type { CoverShape } from "../protocol/generated/CoverShape.ts";
import { sketchLayout } from "./cover.ts";

/** Room around the drawing inside its box. */
const SKETCH_MARGIN = 7;

export function Sketch({
  shapes,
  width,
  height,
}: {
  shapes: readonly CoverShape[];
  width: number;
  height: number;
}) {
  const boxes = useMemo(
    () => sketchLayout(shapes, width, height, SKETCH_MARGIN),
    [shapes, width, height],
  );
  return (
    <svg
      class="sketch"
      aria-hidden="true"
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
    >
      {boxes.map((box) => (
        <rect
          key={`${box.x},${box.y},${box.w},${box.h}`}
          class={`sketch-box cover-${box.tone}`}
          x={box.x}
          y={box.y}
          width={box.w}
          height={box.h}
          rx={box.round ? Math.min(box.w, box.h) / 2 : 1.5}
        />
      ))}
    </svg>
  );
}
