// The stage's header at top left (15 and 15b): a staged diagram's title and the part the voice is
// on ("inf-alloc · memory allocation  part 3 of 4"), or while a detail layer is open the path it
// drilled through ("inf-alloc › Arena"), how to back out, and how deep it is.

import { headerView } from "./diagrams.ts";
import { useRuntime } from "./hooks.ts";

export function StageHeader() {
  const runtime = useRuntime();
  const diagrams = runtime.scene.diagrams;
  const view = headerView(diagrams.headers.value, diagrams.details.value);
  if (view === null) {
    return null;
  }
  if (view.kind === "diagram") {
    return (
      <header class="stage-header stage-header-diagram">
        <span class="stage-header-current">{view.title}</span>
        {view.part === null ? null : <span>{view.part}</span>}
      </header>
    );
  }
  const last = view.crumbs.length - 1;
  return (
    <nav class="stage-header stage-header-detail" aria-label="Detail path">
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
    </nav>
  );
}
