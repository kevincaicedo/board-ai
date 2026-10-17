// The stage's header at top left (15 and 15b): a staged diagram's title and the part the voice is
// on ("inf-alloc · memory allocation  part 3 of 4"), or while a detail layer is open the path it
// drilled through ("inf-alloc › Arena"), how to back out, and how deep it is. A brief's diagram
// adds the chip naming who read the code (D-242). It keeps clear of the Sources tray and its
// button, which hold the same corner (14): it starts right of them (`headerLeft`, LLD §4).

import { useMemo, useRef } from "preact/hooks";
import { headerLeft } from "../sources/model.ts";
import { trayCornerAt } from "../sources/Sources.tsx";
import { headerView } from "./diagrams.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";

function Credit({ credit }: { credit: string | null }) {
  return credit === null ? null : <span class="brief-chip">{credit}</span>;
}

export function StageHeader() {
  const runtime = useRuntime();
  const diagrams = runtime.scene.diagrams;
  const view = headerView(diagrams.headers.value, diagrams.details.value);
  const headerRef = useRef<HTMLElement>(null);
  const write = useMemo(styleWriter, []);
  const leftAt = (at: number): string =>
    `${Math.round(headerLeft(trayCornerAt(runtime, at), runtime.tokens))}px`;
  useFrame((at) => write(headerRef.current, "left", leftAt(at)));
  if (view === null) {
    return null;
  }
  // Placed from its first paint, then kept beside the tray every frame.
  const style = { left: leftAt(runtime.at) };
  if (view.kind === "diagram") {
    return (
      <header class="stage-header stage-header-diagram" ref={headerRef} style={style}>
        <span class="stage-header-current">{view.title}</span>
        {view.part === null ? null : <span>{view.part}</span>}
        <Credit credit={view.credit} />
      </header>
    );
  }
  const last = view.crumbs.length - 1;
  return (
    <nav
      class="stage-header stage-header-detail"
      aria-label="Detail path"
      ref={headerRef}
      style={style}
    >
      {view.crumbs.map((crumb, index) =>
        index === last ? (
          <span key={`crumb-${index}`} class="stage-header-current" aria-current="page">
            {crumb}
          </span>
        ) : (
          <span key={`crumb-${index}`} class="stage-header-crumb">
            <span>{crumb}</span>
            <span aria-hidden="true">›</span>
          </span>
        ),
      )}
      <span class="back-out">Say “back out” · Esc</span>
      <span class="depth-chip">{view.depth}</span>
      <Credit credit={view.credit} />
    </nav>
  );
}
