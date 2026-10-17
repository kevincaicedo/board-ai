// The Sources tray and the drop overlay at the wire's boundary (14 · Sources, 14b, D-184). A
// tray row that doesn't parse is skipped and counted, like a view's row, and the rest of the
// tray shows; lists are held to the core's own bounds.

import {
  fields,
  flag,
  nullable,
  oneOf,
  type RowSkips,
  rows,
  type Tagged,
  tagged,
  text,
  u32,
} from "./fields.ts";
import type { DocumentFormat } from "./generated/DocumentFormat.ts";
import type { DropHalf } from "./generated/DropHalf.ts";
import type { DropOverlay } from "./generated/DropOverlay.ts";
import type { ProjectTab } from "./generated/ProjectTab.ts";
import type { SourceKind } from "./generated/SourceKind.ts";
import type { SourceRow } from "./generated/SourceRow.ts";
import type { SourceState } from "./generated/SourceState.ts";
import type { SourcesTray } from "./generated/SourcesTray.ts";

/** The core's `SESSION_SOURCES_MAX` and `PROJECT_SOURCES_MAX`: a longer list is another build's. */
export const SESSION_ROWS_MAX = 16;
export const PROJECT_ROWS_MAX = 32;
/** The core's `READING_GHOSTS_MAX`. */
export const GHOSTS_MAX = 4;

const FORMATS: readonly DocumentFormat[] = ["Markdown", "Text", "Pdf"];
const HALVES: readonly DropHalf[] = ["Session", "Project"];

const KINDS: Tagged<SourceKind, "kind"> = {
  CodeFolder: () => ({ kind: "CodeFolder" }),
  CodeFile: () => ({ kind: "CodeFile" }),
  Document: (f) => {
    const format = oneOf(f.format, FORMATS);
    return format === null ? null : { kind: "Document", format };
  },
};

const STATES: Tagged<SourceState, "state"> = {
  Reading: (f) => {
    const reader = text(f.reader);
    const read = u32(f.files_read);
    const estimate = nullable(f.files_estimate, u32);
    return reader === null || read === null || estimate === undefined
      ? null
      : { state: "Reading", reader, files_read: read, files_estimate: estimate };
  },
  Ready: (f) => {
    const detail = text(f.detail);
    return detail === null ? null : { state: "Ready", detail };
  },
  Partial: (f) => {
    const reason = text(f.reason);
    return reason === null ? null : { state: "Partial", reason };
  },
  Failed: (f) => {
    const reason = text(f.reason);
    return reason === null ? null : { state: "Failed", reason };
  },
  Waiting: (f) => {
    const reason = text(f.reason);
    return reason === null ? null : { state: "Waiting", reason };
  },
};

function sourceRow(value: unknown): SourceRow | null {
  const f = fields(value);
  const id = u32(f?.id);
  const kind = tagged<SourceKind>(KINDS, "kind", f?.kind);
  const title = text(f?.title);
  const path = text(f?.path);
  const state = tagged<SourceState>(STATES, "state", f?.state);
  return id === null || kind === null || title === null || path === null || state === null
    ? null
    : { id, kind, title, path, state };
}

function bounded(value: unknown, max: number, skips: RowSkips): SourceRow[] | null {
  return Array.isArray(value) && value.length <= max ? rows(value, sourceRow, skips) : null;
}

function projectTab(value: unknown, skips: RowSkips): ProjectTab | null {
  const f = fields(value);
  const name = text(f?.name);
  const proposed = flag(f?.proposed);
  const sources = bounded(f?.sources, PROJECT_ROWS_MAX, skips);
  return name === null || proposed === null || sources === null
    ? null
    : { name, proposed, sources };
}

export function sourcesTray(value: unknown, skips: RowSkips): SourcesTray | null {
  const f = fields(value);
  const open = flag(f?.open);
  const session = bounded(f?.session, SESSION_ROWS_MAX, skips);
  const project = nullable(f?.project, (tab) => projectTab(tab, skips));
  const talker = text(f?.talker);
  const harness = nullable(f?.harness, text);
  const reading =
    Array.isArray(f?.reading) && f.reading.length <= GHOSTS_MAX
      ? rows(f.reading, text, skips)
      : null;
  return open === null ||
    session === null ||
    project === undefined ||
    talker === null ||
    harness === undefined ||
    reading === null
    ? null
    : { open, session, project, talker, harness, reading };
}

export function dropOverlay(value: unknown): DropOverlay | null {
  const f = fields(value);
  const title = text(f?.title);
  const refused = nullable(f?.refused, text);
  const half = oneOf(f?.half, HALVES);
  const project = text(f?.project);
  return title === null || refused === undefined || half === null || project === null
    ? null
    : { title, refused, half, project };
}
