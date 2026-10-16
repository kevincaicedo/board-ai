// Soft cover art for a session (see cover.ts): a porcelain ground, the session's nodes as pastel
// shapes far out of focus, and a gentle light. Colors are the canvas's sketch tones, set in CSS.

import { useMemo } from "preact/hooks";
import type { CoverShape } from "../protocol/generated/CoverShape.ts";
import { coverLayout } from "./cover.ts";

let coverSerial = 0;

export function Cover({
  shapes,
  seed,
  width,
  height,
}: {
  shapes: readonly CoverShape[];
  seed: string;
  width: number;
  height: number;
}) {
  const layout = useMemo(
    () => coverLayout(shapes, seed, width, height),
    [shapes, seed, width, height],
  );
  // SVG ids are document-wide; each cover gets its own.
  const id = useMemo(() => {
    coverSerial += 1;
    return `cover-${coverSerial}`;
  }, []);
  return (
    <svg
      class="cover"
      aria-hidden="true"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="xMidYMid slice"
    >
      <defs>
        <linearGradient id={`${id}-ground`} x1="0" y1="0" x2="0" y2="1">
          <stop class="cover-stop-top" offset="0" />
          <stop class="cover-stop-bottom" offset="1" />
        </linearGradient>
        <radialGradient
          id={`${id}-light`}
          cx={layout.light.x}
          cy={layout.light.y}
          r={Math.max(width, height) * 0.9}
          gradientUnits="userSpaceOnUse"
        >
          <stop class="cover-light-in" offset="0" />
          <stop class="cover-light-out" offset="1" />
        </radialGradient>
        <filter id={`${id}-blur`} x="-30%" y="-30%" width="160%" height="160%">
          <feGaussianBlur stdDeviation={Math.max(4, Math.min(width, height) * 0.04)} />
        </filter>
      </defs>
      <rect width={width} height={height} fill={`url(#${id}-ground)`} />
      <g filter={`url(#${id}-blur)`}>
        {/* A porcelain shade under each shape gives it depth before the blur softens it. */}
        {layout.blobs.map((blob, index) => (
          <rect
            key={`shade-${index}`}
            class="cover-shade"
            x={blob.x + blob.h * 0.12}
            y={blob.y + blob.h * 0.32}
            width={blob.w}
            height={blob.h}
            rx={blob.r}
          />
        ))}
        {layout.blobs.map((blob, index) => (
          <rect
            // Blobs never reorder for a given cover; the index is their identity.
            key={index}
            class={`cover-blob cover-${blob.tone}`}
            x={blob.x}
            y={blob.y}
            width={blob.w}
            height={blob.h}
            rx={blob.r}
          />
        ))}
      </g>
      <rect width={width} height={height} fill={`url(#${id}-light)`} />
    </svg>
  );
}
