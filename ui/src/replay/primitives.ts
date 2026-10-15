// A fixture that exercises every op kind and surface the live demo doesn't: database, queue,
// stacked service, note, dashed and labelled connectors, all four pen marks, a correction,
// compare and recap cards, memory and source chips, depth of field, an acknowledgement and a
// spoken problem. It mirrors the design's canvas primitives page (Components.dc.html).

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import { say } from "./speech.ts";

const rect = (x: number, y: number, w: number, h: number): Rect => ({ x, y, w, h });

let opNumber = 0;
function op<T extends DrawOp>(make: (op: number) => T): T {
  opNumber += 1;
  return make(opNumber);
}

const SHAPES: readonly [number, DrawOp][] = [
  [
    900,
    op((n) => ({
      kind: "Node",
      id: "auth",
      op: n,
      shape: "Service",
      label: "auth",
      rect: rect(40, 150, 140, 76),
      stack: 1,
    })),
  ],
  [
    2000,
    op((n) => ({
      kind: "Node",
      id: "users",
      op: n,
      shape: "Db",
      label: "users",
      rect: rect(230, 136, 128, 102),
      stack: 1,
    })),
  ],
  [
    3400,
    op((n) => ({
      kind: "Node",
      id: "jobs",
      op: n,
      shape: "Queue",
      label: "jobs",
      rect: rect(420, 170, 180, 50),
      stack: 1,
    })),
  ],
  [
    4500,
    op((n) => ({
      kind: "Node",
      id: "client",
      op: n,
      shape: "Actor",
      label: "Client",
      rect: rect(100, 320, 42, 86),
      stack: 1,
    })),
  ],
  [
    5600,
    op((n) => ({
      kind: "Node",
      id: "pod",
      op: n,
      shape: "Service",
      label: "API pod",
      rect: rect(200, 340, 140, 70),
      stack: 3,
    })),
  ],
  [
    7000,
    op((n) => ({
      kind: "Node",
      id: "bus",
      op: n,
      shape: "Note",
      label: "SQS",
      rect: rect(500, 355, 80, 30),
      stack: 1,
    })),
  ],
  [
    7800,
    op((n) => ({
      kind: "Strike",
      op: n,
      target: "bus",
      label: "Kafka",
      rect: rect(510, 355, 60, 30),
    })),
  ],
  [
    8600,
    op((n) => ({
      kind: "Edge",
      id: "client>pod",
      op: n,
      from: "client",
      to: "pod",
      path: { start: { x: 60, y: 510 }, control: null, end: { x: 250, y: 510 } },
      label: "hit",
      style: "Solid",
    })),
  ],
  [
    9400,
    op((n) => ({
      kind: "Edge",
      id: "pod>jobs",
      op: n,
      from: "pod",
      to: "jobs",
      path: { start: { x: 300, y: 510 }, control: { x: 400, y: 570 }, end: { x: 500, y: 510 } },
      label: "async",
      style: "Dashed",
    })),
  ],
  [
    10_300,
    op((n) => ({
      kind: "Annotate",
      id: "circle",
      op: n,
      target: "auth",
      mark: "Circle",
      rect: rect(88, 176, 44, 24),
      text: null,
    })),
  ],
  [
    11_000,
    op((n) => ({
      kind: "Annotate",
      id: "underline",
      op: n,
      target: "jobs",
      mark: "Underline",
      rect: rect(488, 141, 54, 16),
      text: null,
    })),
  ],
  [
    11_600,
    op((n) => ({
      kind: "Annotate",
      id: "arrow",
      op: n,
      target: "pod",
      mark: "Arrow",
      rect: rect(200, 340, 140, 70),
      text: "×3",
    })),
  ],
  [
    12_400,
    op((n) => ({
      kind: "Annotate",
      id: "bracket",
      op: n,
      target: "jobs",
      mark: "Bracket",
      rect: rect(420, 170, 180, 50),
      text: "3 left",
    })),
  ],
];

function timed(): { at: number; event: UiEvent }[] {
  const events: { at: number; event: UiEvent }[] = [
    { at: 0, event: { type: "Camera", layer: 0, frame: rect(0, 0, 1440, 900), focus: null } },
    { at: 0, event: { type: "Orb", state: "Drawing", level_percent: 60 } },
    {
      at: 300,
      event: {
        type: "Thread",
        steps: ["Shapes", "Edges", "Marks", "Surfaces"],
        current: 0,
        detours: [],
      },
    },
    {
      at: 8600,
      event: {
        type: "Thread",
        steps: ["Shapes", "Edges", "Marks", "Surfaces"],
        current: 1,
        detours: [],
      },
    },
    {
      at: 10_300,
      event: {
        type: "Thread",
        steps: ["Shapes", "Edges", "Marks", "Surfaces"],
        current: 2,
        detours: [],
      },
    },
    {
      at: 13_000,
      event: {
        type: "Thread",
        steps: ["Shapes", "Edges", "Marks", "Surfaces"],
        current: 3,
        detours: [],
      },
    },
    { at: 9000, event: { type: "Acknowledge" } },
    {
      at: 13_000,
      event: {
        type: "SurfaceShow",
        surface: {
          kind: "Compare",
          id: 1,
          options: ["cache-aside", "write-through"],
          criteria: ["Easy to add", "Writes stay fast", "Reads always fresh"],
        },
      },
    },
    {
      at: 13_700,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 0, col: 0, mark: { kind: "Yes" } },
      },
    },
    {
      at: 13_920,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 1, col: 0, mark: { kind: "No" } },
      },
    },
    {
      at: 14_140,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 0, col: 1, mark: { kind: "Yes" } },
      },
    },
    {
      at: 14_360,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 1, col: 1, mark: { kind: "Depends" } },
      },
    },
    {
      at: 14_580,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 0, col: 2, mark: { kind: "No" } },
      },
    },
    {
      at: 14_800,
      event: {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Cell", row: 1, col: 2, mark: { kind: "Text", text: "always" } },
      },
    },
    {
      at: 15_600,
      event: {
        type: "SurfaceShow",
        surface: {
          kind: "Recap",
          id: 2,
          bullets: [
            "Reads try Redis first",
            "A miss reads Postgres and writes back",
            "TTLs bound staleness",
          ],
        },
      },
    },
    { at: 16_400, event: { type: "Chip", kind: "Memory", text: "We cache users for ten minutes" } },
    { at: 16_700, event: { type: "Chip", kind: "Source", text: "internal/user/service.go" } },
    {
      at: 17_500,
      event: { type: "Camera", layer: 0, frame: rect(0, 0, 1440, 900), focus: "users" },
    },
    { at: 19_000, event: { type: "Orb", state: "Speaking", level_percent: 60 } },
    {
      at: 21_500,
      event: {
        type: "Problem",
        spoken: "I lost the connection to Claude. Want me to switch to the local model?",
        detail: "Network: the request timed out after 10 s",
      },
    },
    { at: 21_500, event: { type: "Orb", state: "Idle", level_percent: 0 } },
  ];
  const utterance = say("Assistant", 17_200, "Got it. I'll remember that.");
  utterance.words.forEach((_, index) => {
    events.push({
      at: utterance.wordAt[index] ?? 0,
      event: {
        type: "Caption",
        who: "Assistant",
        text: utterance.words.slice(0, index + 1).join(" "),
        is_final: false,
      },
    });
  });
  events.push({
    at: utterance.endAt,
    event: { type: "Caption", who: "Assistant", text: utterance.words.join(" "), is_final: true },
  });
  SHAPES.forEach(([at, drawOp], index) => {
    const next = SHAPES[index + 1];
    events.push({
      at,
      event: {
        type: "OpStart",
        layer: 0,
        op: drawOp,
        budget_ms: next === undefined ? 1500 : Math.min(2500, next[0] - at),
      },
    });
  });
  return events
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

export function primitivesLines(): UiLogLine[] {
  return timed().map(({ at, event }) => ({ v: 1, t: at, k: "ui", event }));
}

export function primitivesJsonl(): string {
  return `${primitivesLines()
    .map((line) => JSON.stringify(line))
    .join("\n")}\n`;
}
