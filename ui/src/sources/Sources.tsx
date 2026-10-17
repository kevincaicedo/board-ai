// The Sources tray (14 · Sources), its button, the ghost file names at the edge while the
// harness reads (PRODUCT §8.6), and the drop overlay (14b). The tray is the core's (D-184):
// every row, its state and whether it is open come from `Sources`; the UI only picks the tab.
// It sits at the left, clear of the column and never over a view (LLD §4).

import { useMemo, useRef, useState } from "preact/hooks";
import { styleWriter, useFrame, useRuntime } from "../app/hooks.ts";
import type { Runtime } from "../app/runtime.ts";
import { viewVisible } from "../app/views.ts";
import type { ChooseWhat } from "../protocol/generated/ChooseWhat.ts";
import type { SourceAction } from "../protocol/generated/SourceAction.ts";
import type { SourceRow } from "../protocol/generated/SourceRow.ts";
import type { SourcesTray } from "../protocol/generated/SourcesTray.ts";
import {
  badgeOf,
  dropPills,
  dropTitle,
  footerLine,
  pinLabel,
  rowsOf,
  stateLine,
  type TrayCorner,
  type TrayTab,
  tabLabels,
  toneOf,
  trayCorner,
} from "./model.ts";

/** Where the tray is this frame: the stage header keeps clear of it (LLD §4). */
export function trayCornerAt(runtime: Runtime, at: number): TrayCorner {
  const store = runtime.scene.sources;
  return trayCorner(
    {
      open: store.tray.value?.open ?? false,
      openAt: store.openAt.value,
      closedAt: store.closedAt.value,
      at,
      stageWidth: runtime.stage.width,
      columnOpen: runtime.scene.store.column.value.some((item) => item.leftAt === null),
      viewOpen: runtime.scene.views.entries.value.some((entry) =>
        viewVisible(entry, at, runtime.viewDurations),
      ),
      replay: runtime.replay !== null,
    },
    runtime.tokens,
  );
}

function useSend(): (action: SourceAction) => void {
  const runtime = useRuntime();
  return (action) => runtime.send({ type: "Sources", action });
}

function PinGlyph({ kept }: { kept: boolean }) {
  return (
    <svg
      aria-hidden="true"
      width="15"
      height="15"
      viewBox="0 0 24 24"
      class={kept ? "tray-pin-glyph tray-pin-kept" : "tray-pin-glyph"}
    >
      <path d="M9 3h6l-1 6 4 4H6l4-4z" />
      <path d="M12 13v8" class="tray-pin-stem" />
    </svg>
  );
}

function Row({ row, kept, project }: { row: SourceRow; kept: boolean; project: string | null }) {
  const send = useSend();
  const badge = badgeOf(row.kind, kept);
  const label = pinLabel(kept, project);
  const tone = toneOf(row.state);
  return (
    <div class="tray-row">
      <div class="tray-row-head">
        <span class={`tray-badge tray-tint-${badge.tint}`} aria-hidden="true">
          {badge.text}
        </span>
        <span class="tray-row-names">
          <span class="tray-row-title">{row.title}</span>
          <span class="tray-row-path">{row.path}</span>
        </span>
        <button
          type="button"
          class="tray-remove"
          aria-label={`Stop sharing ${row.title}`}
          title="Stop sharing"
          onClick={() => send({ kind: "Remove", source: row.id })}
        >
          <svg aria-hidden="true" width="12" height="12" viewBox="0 0 16 16">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
        <button
          type="button"
          class="tray-pin"
          aria-label={label}
          title={label}
          aria-pressed={kept}
          onClick={() =>
            send(kept ? { kind: "Unkeep", source: row.id } : { kind: "Keep", source: row.id })
          }
        >
          <PinGlyph kept={kept} />
        </button>
      </div>
      <div class="tray-state">
        <span class={`tray-dot tray-dot-${tone}`} aria-hidden="true" />
        <span>{stateLine(row.state)}</span>
      </div>
      {tone === "reading" ? <div class="tray-bar" aria-hidden="true" /> : null}
    </div>
  );
}

function Tabs({
  tray,
  tab,
  onTab,
}: {
  tray: SourcesTray;
  tab: TrayTab;
  onTab: (tab: TrayTab) => void;
}) {
  const labels = tabLabels(tray);
  const one = (which: TrayTab, label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={tab === which}
      class={tab === which ? "tray-tab tray-tab-on" : "tray-tab"}
      onClick={() => onTab(which)}
    >
      {label}
    </button>
  );
  return (
    <div class="tray-tabs" role="tablist" aria-label="Scope">
      {one("session", labels.session)}
      {labels.project === null ? null : one("project", labels.project)}
    </div>
  );
}

function Choose({ keep }: { keep: boolean }) {
  const runtime = useRuntime();
  const [problem, setProblem] = useState<string | null>(null);
  const choose = (what: ChooseWhat): void => {
    setProblem(null);
    void runtime.transport.chooseSource(what, keep).then(setProblem);
  };
  return (
    <div class="tray-drop">
      <span class="tray-drop-title">Drop a folder, file or PDF</span>
      <span class="tray-drop-line">
        or choose{" "}
        <button type="button" class="tray-link" onClick={() => choose("folder")}>
          a folder
        </button>{" "}
        or{" "}
        <button type="button" class="tray-link" onClick={() => choose("file")}>
          a file
        </button>
        , or say “use the inf-alloc crate”
      </span>
      {problem === null ? null : <span class="tray-drop-problem">{problem}</span>}
    </div>
  );
}

function TrayPanel({ tray }: { tray: SourcesTray }) {
  const runtime = useRuntime();
  const store = runtime.scene.sources;
  const send = useSend();
  const tab = tray.project === null ? "session" : store.tab.value;
  const rows = rowsOf(tray, tab);
  const kept = tab === "project";
  const project = tray.project?.name ?? null;
  const panelRef = useRef<HTMLElement>(null);
  const write = useMemo(styleWriter, []);
  useFrame((at) => {
    const { presence, width } = trayCornerAt(runtime, at);
    const panel = panelRef.current;
    write(panel, "--tray-presence", presence.toFixed(3));
    write(panel, "--tray-live-width", `${width ?? runtime.tokens.trayWidth}px`);
    write(panel, "visibility", width === null || presence <= 0 ? "hidden" : "visible");
  });
  return (
    <section class="tray" ref={panelRef} aria-label="Sources">
      <div class="tray-head">
        <span class="tray-label">Sources</span>
        <h2 class="tray-title">What BoardAI can read</h2>
        <button
          type="button"
          class="tray-close"
          aria-label="Close sources"
          onClick={() => send({ kind: "Close" })}
        >
          <svg aria-hidden="true" width="14" height="14" viewBox="0 0 16 16">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </div>
      <Tabs
        tray={tray}
        tab={tab}
        onTab={(next) => {
          store.tab.value = next;
        }}
      />
      <div class="tray-rows">
        {rows.length === 0 ? (
          <p class="tray-empty">
            Nothing shared {kept ? "in this project" : "for this session"} yet.
          </p>
        ) : (
          rows.map((row) => <Row key={row.id} row={row} kept={kept} project={project} />)
        )}
      </div>
      <div class="tray-foot">
        <Choose keep={kept} />
        <span class="tray-privacy">{footerLine(tray)}</span>
      </div>
    </section>
  );
}

/** Opens the tray; it shows the count of what is shared, and hides while the tray is open. */
function TrayButton({ tray }: { tray: SourcesTray | null }) {
  const runtime = useRuntime();
  if (runtime.replay !== null) {
    return null;
  }
  const count = (tray?.session.length ?? 0) + (tray?.project?.sources.length ?? 0);
  const open = tray?.open ?? false;
  return (
    <button
      type="button"
      class={open ? "tray-button tray-button-hidden" : "tray-button"}
      aria-label={count === 0 ? "Sources" : `Sources, ${count} shared`}
      title="Sources"
      onClick={() => runtime.send({ type: "Sources", action: { kind: "Open" } })}
    >
      <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24">
        <path d="M4 7h6l2 2h8v9a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18z" />
      </svg>
      {count > 0 ? <span class="tray-button-count">{count}</span> : null}
    </button>
  );
}

/** The files the harness is reading, drifting up at the right edge while it reads. */
function Ghosts({ names }: { names: readonly string[] }) {
  if (names.length === 0) {
    return null;
  }
  return (
    <div class="ghosts" aria-hidden="true">
      {names.map((name, index) => (
        <span key={name} class="ghost" style={{ "--ghost-index": index }}>
          {name}
        </span>
      ))}
    </div>
  );
}

function DropOverlayView() {
  const runtime = useRuntime();
  const drop = runtime.scene.sources.drop.value;
  if (drop === null) {
    return null;
  }
  return (
    <div class={drop.refused === null ? "drop" : "drop drop-refused"} role="status">
      <div class={`drop-half drop-half-${drop.half === "Session" ? "left" : "right"}`} />
      <div class="drop-body">
        <span class="drop-title">{dropTitle(drop)}</span>
        {drop.refused === null ? (
          <div class="drop-pills">
            {dropPills(drop).map((pill) => (
              <span key={pill.half} class={pill.on ? "drop-pill drop-pill-on" : "drop-pill"}>
                {pill.label}
              </span>
            ))}
          </div>
        ) : null}
        <span class="drop-hint">
          Drop on a half to choose. Folders, code files, Markdown, text and PDF.
        </span>
      </div>
    </div>
  );
}

export function Sources() {
  const runtime = useRuntime();
  const tray = runtime.scene.sources.tray.value;
  return (
    <>
      <TrayButton tray={tray} />
      {tray === null ? null : <TrayPanel tray={tray} />}
      <Ghosts names={tray?.reading ?? []} />
      <DropOverlayView />
    </>
  );
}
