// Hand-written compare marks (Session.dc.html markPts): a check, a cross, and the "depends"
// squiggle, jittered by a seed so each cell is drawn by hand but replays identically.

import { quadPoints, type Vec } from "../canvas/geometry.ts";
import { randomFor } from "../canvas/rng.ts";
import { makeStroke, type Stroke } from "../canvas/stroke.ts";

export type MarkKind = "Yes" | "No" | "Depends";

export function markStrokes(kind: MarkKind, seed: string): Stroke[] {
  const random = randomFor(seed);
  const jitter = (): number => (random() - 0.5) * 1.6;
  switch (kind) {
    case "Yes": {
      const down = quadPoints(
        3 + jitter(),
        13 + jitter(),
        6.5 + jitter(),
        16 + jitter(),
        10 + jitter(),
        21 + jitter(),
        6,
      );
      const up = quadPoints(
        10,
        21,
        14 + jitter(),
        11 + jitter(),
        25 + jitter(),
        3 + jitter(),
        8,
      ).slice(1);
      return [makeStroke([...down, ...up], "ink", 2.2)];
    }
    case "No":
      return [
        makeStroke(
          quadPoints(
            5 + jitter(),
            4 + jitter(),
            13 + jitter(),
            12 + jitter(),
            22 + jitter(),
            21 + jitter(),
            8,
          ),
          "ink3",
          2.2,
        ),
        makeStroke(
          quadPoints(
            22 + jitter(),
            4 + jitter(),
            13 + jitter(),
            12 + jitter(),
            5 + jitter(),
            21 + jitter(),
            8,
          ),
          "ink3",
          2.2,
        ),
      ];
    case "Depends": {
      const wave: Vec[] = [
        [3, 14],
        [7, 10],
        [11, 9],
        [15, 13],
        [19, 15],
        [23, 12],
        [26, 9],
      ];
      return [makeStroke(wave, "ink", 2.2)];
    }
  }
}
