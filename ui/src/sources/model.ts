// The Sources tray (14 · Sources) and the drop overlay (14b) as the UI shows them. The core sends
// the tray whole on every change (D-184), so this store keeps only the last one, when it opened
// or closed (to animate on session time, like every panel), and which tab is showing: UI-local
// state, like a scroll position. The words on each row come from pure functions tested here.

import { signal } from "@preact/signals";
import { easeOut, ramp } from "../canvas/math.ts";
import type { DropHalf } from "../protocol/generated/DropHalf.ts";
import type { DropOverlay } from "../protocol/generated/DropOverlay.ts";
import type { SourceKind } from "../protocol/generated/SourceKind.ts";
import type { SourceRow } from "../protocol/generated/SourceRow.ts";
import type { SourceState } from "../protocol/generated/SourceState.ts";
import type { SourcesTray } from "../protocol/generated/SourcesTray.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";

export type TrayTab = "session" | "project";

/** A badge's tone, one of the canvas's tints (`--tray-tint-*`). */
export type Tint = "code" | "kept" | "pdf" | "markdown" | "text";

/** A state's dot (`--state-*`). */
export type Tone = "reading" | "ready" | "partial" | "failed" | "waiting";

export class SourcesStore {
  readonly tray = signal<SourcesTray | null>(null);
  /** When the tray last opened or closed, in session time: its panel eases from there. */
  readonly openAt = signal<number | null>(null);
  readonly closedAt = signal<number | null>(null);
  readonly drop = signal<DropOverlay | null>(null);
  readonly tab = signal<TrayTab>("session");

  reset(): void {
    this.tray.value = null;
    this.openAt.value = null;
    this.closedAt.value = null;
    this.drop.value = null;
  }

  apply(event: UiEvent, at: number): void {
    if (event.type === "Sources") {
      const wasOpen = this.tray.value?.open ?? false;
      if (event.tray.open && !wasOpen) {
        this.openAt.value = at;
        this.closedAt.value = null;
        this.tab.value = firstTab(event.tray);
      } else if (!event.tray.open && wasOpen) {
        this.closedAt.value = at;
      }
      this.tray.value = event.tray;
    } else if (event.type === "SourceDrop") {
      this.drop.value = event.drop;
    } else if (event.type === "SessionEnd") {
      if (this.tray.value?.open === true) {
        this.closedAt.value = at;
      }
      this.drop.value = null;
    }
  }
}

/** The tab that opens first: this session's when it has anything, else the project's. */
export function firstTab(tray: SourcesTray): TrayTab {
  return tray.session.length === 0 && (tray.project?.sources.length ?? 0) > 0
    ? "project"
    : "session";
}

/** The tabs' words, with their counts: "This session · 2", "infinitydb project · 3". */
export function tabLabels(tray: SourcesTray): { session: string; project: string | null } {
  const project = tray.project;
  return {
    session: `This session · ${tray.session.length}`,
    project:
      project === null
        ? null
        : `${project.name} ${project.proposed ? "project?" : "project"} · ${project.sources.length}`,
  };
}

export function rowsOf(tray: SourcesTray, tab: TrayTab): readonly SourceRow[] {
  return tab === "session" ? tray.session : (tray.project?.sources ?? []);
}

/**
 * A row's state line: "Claude Code is reading · 23 of ~40 files" for code (the walk's count is an
 * estimate), "BoardAI is reading · 3 of 14 pages" for a document (its page count is exact).
 */
export function stateLine(state: SourceState, kind: SourceKind): string {
  switch (state.state) {
    case "Reading": {
      if (state.files_read === 0) {
        return `${state.reader} is reading`;
      }
      const document = kind.kind === "Document";
      const unit = document ? "page" : "file";
      const count = state.files_read === 1 ? `1 ${unit}` : `${state.files_read} ${unit}s`;
      const of =
        state.files_estimate === null
          ? count
          : `${state.files_read} of ${document ? "" : "~"}${state.files_estimate} ${unit}s`;
      return `${state.reader} is reading · ${of}`;
    }
    case "Ready":
      return state.detail;
    case "Partial":
    case "Failed":
    case "Waiting":
      return state.reason;
  }
}

export function toneOf(state: SourceState): Tone {
  switch (state.state) {
    case "Reading":
      return "reading";
    case "Ready":
      return "ready";
    case "Partial":
      return "partial";
    case "Failed":
      return "failed";
    case "Waiting":
      return "waiting";
  }
}

/** The badge on a row: `{ }` for code, the format for a document; kept code takes the sky tint. */
export function badgeOf(kind: SourceKind, kept: boolean): { text: string; tint: Tint } {
  switch (kind.kind) {
    case "CodeFolder":
      return { text: "{ }", tint: kept ? "kept" : "code" };
    case "CodeFile":
      return { text: "</>", tint: kept ? "kept" : "code" };
    case "Document":
      switch (kind.format) {
        case "Pdf":
          return { text: "PDF", tint: "pdf" };
        case "Markdown":
          return { text: "MD", tint: "markdown" };
        case "Text":
          return { text: "TXT", tint: "text" };
      }
  }
}

/** The pin's label: what pressing it does. */
export function pinLabel(kept: boolean, project: string | null): string {
  const name = project === null ? "a new project" : `the ${project} project`;
  return kept ? `Kept in ${name}` : `Keep in ${name}`;
}

/** The footer: where a source's text goes. */
export function footerLine(tray: SourcesTray): string {
  const research = tray.harness === null ? "" : ` and ${tray.harness} (research)`;
  return `Read on this Mac. Text goes only to ${tray.talker} (talking)${research}.`;
}

/** The drop overlay's pills, the one under the pointer marked. */
export function dropPills(overlay: DropOverlay): { half: DropHalf; label: string; on: boolean }[] {
  return [
    { half: "Session", label: "This session", on: overlay.half === "Session" },
    { half: "Project", label: `Keep in ${overlay.project}`, on: overlay.half === "Project" },
  ];
}

export function dropTitle(overlay: DropOverlay): string {
  return overlay.refused ?? `Drop to share ${overlay.title}`;
}

/** Sizes the tray's layout needs (from tokens.css). */
export interface TrayTokens {
  readonly trayLeft: number;
  readonly trayWidth: number;
  readonly trayWidthMin: number;
  readonly trayColumnGap: number;
  readonly surfaceWidth: number;
  readonly surfaceRight: number;
}

/**
 * The tray's width on this stage, or `null` when it must stay hidden: it never sits over the
 * column (LLD §4). With cards in the column it narrows to the room left of the column, and
 * hides below its minimum; with a view open it hides.
 */
export function trayWidth(
  stageWidth: number,
  columnOpen: boolean,
  viewOpen: boolean,
  tokens: TrayTokens,
): number | null {
  if (viewOpen) {
    return null;
  }
  const room = columnOpen
    ? stageWidth -
      tokens.surfaceRight -
      tokens.surfaceWidth -
      tokens.trayColumnGap -
      tokens.trayLeft
    : stageWidth - 2 * tokens.trayLeft;
  const width = Math.min(tokens.trayWidth, room);
  return width >= tokens.trayWidthMin ? width : null;
}

/** 0 → 1 as the tray opens, 1 → 0 as it closes, on session time. */
export function trayPresence(
  openAt: number | null,
  closedAt: number | null,
  at: number,
  enterMs: number,
  exitMs: number,
): number {
  if (openAt === null) {
    return 0;
  }
  const enter = easeOut(ramp(at, openAt, openAt + enterMs));
  if (closedAt === null || closedAt < openAt) {
    return enter;
  }
  return enter * (1 - easeOut(ramp(at, closedAt, closedAt + exitMs)));
}

/** What holds the stage's top-left corner this frame: the tray, its button, or neither. */
export interface TrayCorner {
  /** The tray button shows (live, with the tray closed). */
  readonly buttonShown: boolean;
  /** The tray's width on this stage, or `null` while it is hidden (`trayWidth`). */
  readonly width: number | null;
  /** 0 → 1 as the tray opens, back as it closes (`trayPresence`). */
  readonly presence: number;
}

/** What the tray's corner depends on, from the stage and the sources store. */
export interface TrayCornerInput {
  readonly open: boolean;
  readonly openAt: number | null;
  readonly closedAt: number | null;
  readonly at: number;
  readonly stageWidth: number;
  readonly columnOpen: boolean;
  readonly viewOpen: boolean;
  /** A replay has no tray button. */
  readonly replay: boolean;
}

export interface TrayCornerTokens extends TrayTokens {
  readonly durationSceneMs: number;
  readonly durationStandardMs: number;
}

/** The tray's place this frame, as the tray itself draws it. */
export function trayCorner(input: TrayCornerInput, tokens: TrayCornerTokens): TrayCorner {
  return {
    buttonShown: !input.replay && !input.open,
    width: trayWidth(input.stageWidth, input.columnOpen, input.viewOpen, tokens),
    presence: trayPresence(
      input.openAt,
      input.closedAt,
      input.at,
      tokens.durationSceneMs,
      tokens.durationStandardMs,
    ),
  };
}

/** Sizes the stage header's place needs (from tokens.css). */
export interface HeaderTokens {
  readonly trayLeft: number;
  readonly sizeControl: number;
  readonly stageHeaderLeft: number;
  readonly stageHeaderTrayGap: number;
  /** How far into the tray's entrance the header is already beside it (0–1). */
  readonly stageHeaderTrayLead: number;
}

/**
 * Where the stage header starts (15's diagram title, 15b's breadcrumbs): at its own place, top
 * left, unless the Sources tray or its button holds that corner (14); then right of them by the
 * gap. The tray never moves for the header. While the tray eases in or out the header goes
 * ahead of it: beside the tray by `stageHeaderTrayLead` of its entrance, and back only in the
 * last part of its exit, so the two never overlap once the tray is more than faintly there.
 */
export function headerLeft(corner: TrayCorner, tokens: HeaderTokens): number {
  const own = tokens.stageHeaderLeft;
  const button = corner.buttonShown
    ? Math.max(own, tokens.trayLeft + tokens.sizeControl + tokens.stageHeaderTrayGap)
    : own;
  if (corner.width === null || corner.presence <= 0) {
    return button;
  }
  const beside = Math.max(button, tokens.trayLeft + corner.width + tokens.stageHeaderTrayGap);
  const lead = tokens.stageHeaderTrayLead > 0 ? tokens.stageHeaderTrayLead : 1;
  const ahead = Math.min(1, corner.presence / lead);
  return button + (beside - button) * ahead;
}
