// Sources as protocol events (14 · Sources, 14b): you ask about inf-alloc, the tray opens while
// Claude Code reads it, its files drift by at the edge, and the overview brief makes it ready
// and stages its diagram (the header names it, with the brief's chip, D-242); then
// hekaton-sigmod13.pdf is dragged over the window, left half then right, and dropped on the
// right: it is kept in the infinitydb project and waits for the document reader, and the tray,
// open again, pushes the header to its right (LLD §4). The rows are artboard 14's. Regenerate
// with `node src/replay/write-demo.ts`.

import type { OrbState } from "../protocol/generated/OrbState.ts";
import type { SourceRow } from "../protocol/generated/SourceRow.ts";
import type { SourcesTray } from "../protocol/generated/SourcesTray.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import { say, type Utterance } from "./speech.ts";

/** The artboard's moment: the tray open, inf-alloc being read. */
export const SOURCES_ARTBOARD_MS = 4_200;
/** The drop overlay over the right half (14b). */
export const SOURCES_DROP_MS = 11_400;
/** The brief's diagram staged, its header showing with the tray closed. */
export const SOURCES_STAGED_MS = 9_000;

interface Timed {
  readonly at: number;
  readonly event: UiEvent;
}

const QUESTION = say("User", 600, "Walk me through how inf-alloc hands out memory.");
const READING_LINE = say(
  "Assistant",
  3_600,
  "Claude Code is reading the allocator, the arena first. I'll draw it as soon as it's mapped.",
);

const FILES = [
  "inf-alloc/src/arena.rs",
  "src/pool/aligned.rs",
  "inf-alloc/src/slot.rs",
  "inf-alloc/src/region.rs",
];

function infAlloc(state: SourceRow["state"]): SourceRow {
  return {
    id: 1,
    kind: { kind: "CodeFolder" },
    title: "inf-alloc",
    path: "~/…/infinitydb/crates/inf-alloc",
    state,
  };
}

const PAPER: SourceRow = {
  id: 2,
  kind: { kind: "Document", format: "Pdf" },
  title: "FoundationDB paper",
  path: "~/Papers/foundationdb-sigmod21.pdf",
  state: { state: "Waiting", reason: "Waiting for the document reader" },
};

const KEPT: SourceRow[] = [
  {
    id: 3,
    kind: { kind: "CodeFolder" },
    title: "infinitydb",
    path: "~/Documents/Projects/database/infinitydb",
    state: { state: "Ready", detail: "Overview ready · 17 parts mapped · 2026-10-14" },
  },
  {
    id: 4,
    kind: { kind: "Document", format: "Markdown" },
    title: "inf-alloc README",
    path: "~/…/infinitydb/crates/inf-alloc/README.md",
    state: { state: "Waiting", reason: "Waiting for the document reader" },
  },
];

const HEKATON: SourceRow = {
  id: 5,
  kind: { kind: "Document", format: "Pdf" },
  title: "hekaton-sigmod13.pdf",
  path: "~/Papers/hekaton-sigmod13.pdf",
  state: { state: "Waiting", reason: "Waiting for the document reader" },
};

function tray(open: boolean, session: SourceRow[], kept: SourceRow[], reading: string[]): UiEvent {
  const shown: SourcesTray = {
    open,
    session,
    project: { name: "infinitydb", proposed: false, sources: kept },
    talker: "OpenRouter",
    harness: "Claude Code",
    reading,
  };
  return { type: "Sources", tray: shown };
}

function reading(files: number): SourceRow {
  return infAlloc({
    state: "Reading",
    reader: "Claude Code",
    files_read: files,
    files_estimate: 40,
  });
}

function orb(at: number, state: OrbState, level = 0): Timed {
  return { at, event: { type: "Orb", state, level_percent: level } };
}

function captions(utterance: Utterance): Timed[] {
  const timed: Timed[] = utterance.words.map((_, index) => ({
    at: utterance.wordAt[index] ?? utterance.startAt,
    event: {
      type: "Caption",
      who: utterance.who,
      text: utterance.words.slice(0, index + 1).join(" "),
      is_final: false,
    },
  }));
  timed.push({
    at: utterance.endAt,
    event: { type: "Caption", who: utterance.who, text: utterance.words.join(" "), is_final: true },
  });
  return timed;
}

function timeline(): Timed[] {
  const timed: Timed[] = [
    { at: 0, event: { type: "SessionStart", session: null } },
    orb(0, "Idle"),
    { at: 200, event: tray(false, [PAPER], KEPT, []) },
    orb(600, "Listening", 40),
    ...captions(QUESTION),
    orb(QUESTION.endAt + 200, "Thinking"),
    // The question asks about inf-alloc: Claude Code reads it, and the tray opens on its own.
    { at: 2_400, event: tray(true, [reading(0), PAPER], KEPT, []) },
    orb(READING_LINE.startAt, "Speaking", 60),
    ...captions(READING_LINE),
    orb(READING_LINE.endAt + 200, "Thinking"),
  ];
  // Each file the harness reads joins the ghosts; the core keeps the newest four.
  for (let index = 0; index < 23; index += 1) {
    const shown = Math.min(FILES.length, index + 1);
    const ghosts = Array.from(
      { length: shown },
      (_, offset) => FILES[(index - shown + 1 + offset) % FILES.length] ?? "",
    );
    const event = tray(true, [reading(index + 1), PAPER], KEPT, ghosts);
    timed.push({ at: 2_600 + index * 150, event });
  }
  const ready = infAlloc({ state: "Ready", detail: "Overview ready · 9 parts mapped" });
  timed.push(
    { at: 8_000, event: tray(false, [ready, PAPER], KEPT, []) },
    {
      at: 8_100,
      event: {
        type: "Diagram",
        layer: 0,
        title: "inf-alloc · memory allocation",
        part: 0,
        parts: 4,
        provenance: { reader: "Claude Code", source: "inf-alloc", files_read: 23 },
      },
    },
    orb(8_200, "Idle"),
    {
      at: 10_000,
      event: {
        type: "SourceDrop",
        drop: {
          title: "hekaton-sigmod13.pdf",
          refused: null,
          half: "Session",
          project: "infinitydb",
        },
      },
    },
    {
      at: 11_000,
      event: {
        type: "SourceDrop",
        drop: {
          title: "hekaton-sigmod13.pdf",
          refused: null,
          half: "Project",
          project: "infinitydb",
        },
      },
    },
    { at: 12_400, event: { type: "SourceDrop", drop: null } },
    { at: 12_400, event: tray(true, [ready, PAPER], [...KEPT, HEKATON], []) },
  );
  return timed
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

export function sourcesLines(): UiLogLine[] {
  return timeline().map(({ at: t, event }) => ({ v: 1, t, k: "ui", event }));
}

export function sourcesJsonl(): string {
  return `${sourcesLines()
    .map((line) => JSON.stringify(line))
    .join("\n")}\n`;
}
