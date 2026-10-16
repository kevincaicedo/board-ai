// The views summoned by voice, over the canvas and under the orb and captions (LLD §4). The core
// opens one at a time; each arrives and leaves on session time like a surface (the panels slide in
// from the brain edge, the rest rise out of a blur). Esc is routed in app/keys.ts: it leaves a
// card's replay first, then asks the core to close its view, and otherwise stops BoardAI.

import { useEffect, useLayoutEffect, useMemo, useRef } from "preact/hooks";
import { styleWriter, useFrame, useRuntime } from "../app/hooks.ts";
import { entranceOf, type ViewEntry, viewMotion } from "../app/views.ts";
import { easeOut, ramp, round1, round2 } from "../canvas/math.ts";
import { LibraryView } from "./Library.tsx";
import { MemoryView } from "./Memory.tsx";
import { RecallView } from "./Recall.tsx";
import { ReplayOverlay } from "./Replay.tsx";
import { TasksView } from "./Tasks.tsx";

/** Sections of a panel rise in this far, as the canvas's pearlRise does. */
const STAGGER_RISE = 12;
const STAGGER_BLUR = 8;

function ViewBody({ entry }: { entry: ViewEntry }) {
  const runtime = useRuntime();
  const view = entry.view;
  switch (view.kind) {
    case "Library":
      return <LibraryView view={view} today={entry.today} />;
    case "Tasks":
      return <TasksView view={view} today={entry.today} />;
    case "Memory":
      return <MemoryView view={view} today={entry.today} />;
    case "PastSession":
      return <RecallView entry={entry} view={view} />;
    case "Replay":
      return (
        <ReplayOverlay card={view.session} onClose={() => runtime.send({ type: "CloseView" })} />
      );
  }
}

function ViewFrame({ entry }: { entry: ViewEntry }) {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const rootRef = useRef<HTMLDivElement>(null);
  const sectionsRef = useRef<HTMLElement[]>([]);
  const write = useMemo(styleWriter, []);
  const entrance = entranceOf(entry.view.kind);

  useLayoutEffect(() => {
    sectionsRef.current = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>("[data-stagger]") ?? [],
    );
  });

  useFrame((at) => {
    const root = rootRef.current;
    const motion = viewMotion(entry, at, runtime.viewDurations);
    const visible = at >= entry.shownAt && motion.exit < 1;
    write(root, "display", visible ? "block" : "none");
    if (!visible) {
      return;
    }
    write(root, "opacity", String(round2(motion.enter * (1 - motion.exit))));
    const blurIn = entrance === "panel" ? tokens.panelBlurIn : tokens.surfaceBlurIn;
    const blur = blurIn * (1 - motion.enter) + tokens.surfaceBlurOut * motion.exit;
    write(root, "filter", blur < 0.05 ? "none" : `blur(${round1(blur)}px)`);
    write(
      root,
      "transform",
      entrance === "panel"
        ? `translateX(${round1(tokens.panelShiftIn * (1 - motion.enter))}px)`
        : `translateY(${round1(tokens.surfaceRise * (1 - motion.enter))}px)`,
    );
    for (const section of sectionsRef.current) {
      const index = Number(section.dataset.stagger ?? "0");
      const begin = entry.shownAt + tokens.durationMicroMs + index * tokens.durationViewStaggerMs;
      const rise = easeOut(ramp(at, begin, begin + tokens.durationStandardMs));
      write(section, "opacity", String(round2(rise)));
      write(section, "transform", `translateY(${round1(STAGGER_RISE * (1 - rise))}px)`);
      write(
        section,
        "filter",
        rise > 0.995 ? "none" : `blur(${round1(STAGGER_BLUR * (1 - rise))}px)`,
      );
    }
  });

  return (
    <div
      class={`view-frame view-frame-${entry.view.kind.toLowerCase()}`}
      ref={rootRef}
      data-view={entry.view.kind}
    >
      <ViewBody entry={entry} />
    </div>
  );
}

export function Views() {
  const runtime = useRuntime();
  const entries = runtime.scene.views.entries.value;
  const local = runtime.replays.local.value;
  const last = entries[entries.length - 1];
  const open = last !== undefined && last.leftAt === null ? last : null;
  const openKind = open?.view.kind ?? null;

  // A card's replay lives inside the view it was started from: when that view goes, so does it.
  useEffect(() => {
    const from = runtime.replays.local.value?.from;
    if (from === undefined) {
      return;
    }
    if (openKind !== (from === "library" ? "Library" : "PastSession")) {
      runtime.replays.close();
    }
  }, [runtime, openKind, open?.serial]);

  return (
    <div class="views">
      {entries.map((entry) => (
        <ViewFrame key={entry.serial} entry={entry} />
      ))}
      {local === null ? null : (
        <div class="view-frame view-frame-local-replay">
          <ReplayOverlay
            key={local.card.id}
            card={local.card}
            onClose={() => runtime.replays.close()}
          />
        </div>
      )}
    </div>
  );
}
