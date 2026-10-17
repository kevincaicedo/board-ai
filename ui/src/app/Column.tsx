// The surface column in the right half of the stage (frames 4–6): the open detour's card and at
// most two surfaces, stacked from the top, or one centered. Cards rise out of a blur when they
// arrive, glide on a spring when the column makes room, and blur out when they leave. A detour
// card is sized from its drawing each frame (D-151): it grows taller, and wider to the left of
// the column, when the drawing would no longer read at frame 4's size.

import { useMemo, useRef } from "preact/hooks";
import { round1, round2 } from "../canvas/math.ts";
import { CodeCard, CompareCard, DetourCard, RecapCard } from "../surfaces/Cards.tsx";
import { DocCard } from "../surfaces/DocCard.tsx";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { ColumnTopsCache, surfaceMotion } from "./layout.ts";
import type { ColumnItem } from "./session.ts";

function CardBody({ item }: { item: ColumnItem }) {
  if (item.kind === "detour") {
    return <DetourCard item={item} />;
  }
  const surface = item.surface;
  switch (surface.kind) {
    case "Code":
      return <CodeCard surface={surface} highlights={item.highlights} marks={item.marks} />;
    case "Compare":
      return <CompareCard surface={surface} cells={item.cells} />;
    case "Recap":
      return <RecapCard surface={surface} />;
    case "Doc":
      return <DocCard surface={surface} highlights={item.highlights} />;
  }
}

export function Column() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const items = runtime.scene.store.column.value;
  const cardRefs = useRef(new Map<string, HTMLElement>());
  const write = useMemo(styleWriter, []);
  // A detour card's height comes from its drawing (runtime.detourCards, sized once per frame);
  // looked up rather than copied into the items, and laid out only while something moves.
  const layout = useMemo(() => {
    const cache = new ColumnTopsCache<ColumnItem>();
    const height = (item: ColumnItem): number =>
      item.kind === "detour"
        ? (runtime.detourCards.get(item.layer)?.height ?? item.height)
        : item.height;
    return { cache, height };
  }, [runtime]);

  useFrame((at) => {
    const cards = runtime.detourCards;
    const tops = layout.cache.tops(items, at, tokens, layout.height);
    const column = runtime.columnWidth;
    for (const item of items) {
      const card = cardRefs.current.get(item.key) ?? null;
      if (card === null) {
        continue;
      }
      const size = item.kind === "detour" ? cards.get(item.layer) : undefined;
      if (size !== undefined) {
        write(card, "height", `${size.height}px`);
        write(card, "width", `${size.width}px`);
        write(card, "left", `${column - size.width}px`);
      }
      const motion = surfaceMotion(item, at, tokens.durationStandardMs);
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
          // A detour card's size is written each frame from its drawing.
          style={item.kind === "detour" ? undefined : { height: `${item.height}px` }}
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
