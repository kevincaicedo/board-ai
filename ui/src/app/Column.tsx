// The surface column in the right half of the stage (frames 4–6): at most two cards, stacked from
// the top, or one centered. Cards rise out of a blur when they arrive, glide on a spring when the
// column makes room, and blur out when they leave.

import { useMemo, useRef } from "preact/hooks";
import { round1, round2 } from "../canvas/math.ts";
import { CodeCard, CompareCard, DetourCard, RecapCard } from "../surfaces/Cards.tsx";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { columnTops, surfaceMotion } from "./layout.ts";
import type { ColumnItem } from "./session.ts";

function CardBody({ item }: { item: ColumnItem }) {
  if (item.kind === "detour") {
    return <DetourCard item={item} />;
  }
  const surface = item.surface;
  switch (surface.kind) {
    case "Code":
      return <CodeCard surface={surface} highlights={item.highlights} />;
    case "Compare":
      return <CompareCard surface={surface} cells={item.cells} />;
    case "Recap":
      return <RecapCard surface={surface} />;
  }
}

export function Column() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const items = runtime.scene.store.column.value;
  const cardRefs = useRef(new Map<string, HTMLElement>());
  const write = useMemo(styleWriter, []);

  useFrame((at) => {
    const tops = columnTops(items, at, tokens);
    for (const item of items) {
      const card = cardRefs.current.get(item.key) ?? null;
      if (card === null) {
        continue;
      }
      const motion = surfaceMotion(item.enterAt, item.leftAt, at, tokens.durationStandardMs);
      const visible = at >= item.enterAt && motion.exit < 1;
      write(card, "display", visible ? "block" : "none");
      if (!visible) {
        continue;
      }
      const top = (tops.get(item.key) ?? 0) + tokens.surfaceRise * (1 - motion.enter);
      write(card, "transform", `translateY(${round1(top)}px)`);
      write(card, "opacity", String(round2(motion.enter * (1 - motion.exit))));
      const blur = tokens.surfaceBlurIn * (1 - motion.enter) + tokens.surfaceBlurOut * motion.exit;
      write(card, "filter", blur < 0.05 ? "none" : `blur(${round1(blur)}px)`);
    }
  });

  return (
    <div class="column">
      {items.map((item) => (
        <section
          key={item.key}
          class={`surface surface-${item.kind === "detour" ? "detour" : item.surface.kind.toLowerCase()}`}
          style={{ height: `${item.height}px` }}
          ref={(element) => {
            if (element === null) {
              cardRefs.current.delete(item.key);
            } else {
              cardRefs.current.set(item.key, element);
            }
          }}
          aria-label={item.kind === "detour" ? `Detour: ${item.title}` : undefined}
        >
          <CardBody item={item} />
        </section>
      ))}
    </div>
  );
}
