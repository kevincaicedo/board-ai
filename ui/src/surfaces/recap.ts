// The recap card's bullets (StageStates: 3–5 bullets). A bullet may repeat another word for word,
// and the list never reorders once shown, so each bullet is keyed by its place.

import type { Surface } from "../protocol/generated/Surface.ts";

export interface RecapRow {
  readonly text: string;
  readonly key: string;
}

export function recapRows(surface: Extract<Surface, { kind: "Recap" }>): RecapRow[] {
  return surface.bullets.map((text, index) => ({ text, key: `bullet-${index}` }));
}
