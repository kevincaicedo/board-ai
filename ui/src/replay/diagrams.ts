// A fixture that draws each diagram type the core lays out (PRODUCT §7.1, D-150): a sequence
// diagram whose lifelines grow with its messages, an array filled, highlighted and corrected, a
// binary search tree growing, and a hash table with a collision chain; then a source chip that
// opens its line. The ops are the core's own output for these narrations (the scenes of
// crates/core/tests/sim_diagrams.rs, drawn through the simulator), each scene moved to its own
// place on the board and framed in turn, so a screenshot shows one diagram at its natural size.

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import { say } from "./speech.ts";

/** One op as the core scheduled it: its start in ms from the scene's first op, and its budget. */
type Step = readonly [atMs: number, budgetMs: number, op: DrawOp];

const SEQUENCE: readonly Step[] = [
  [
    0,
    900,
    {
      kind: "Participant",
      id: "c",
      op: 0,
      label: "Client",
      rect: { x: 47, y: 46, w: 105, h: 48 },
      lifeline_bottom: 242,
    },
  ],
  [
    900,
    1800,
    {
      kind: "Participant",
      id: "s",
      op: 1,
      label: "Server",
      rect: { x: 244, y: 46, w: 112, h: 48 },
      lifeline_bottom: 242,
    },
  ],
  [
    2700,
    1440,
    {
      kind: "Message",
      id: "m:2",
      op: 2,
      from: "c",
      to: "s",
      points: [
        { x: 99, y: 138 },
        { x: 300, y: 138 },
      ],
      label: "SYN",
      style: "Solid",
      extend: [],
    },
  ],
  [
    4140,
    1380,
    {
      kind: "Message",
      id: "m:3",
      op: 3,
      from: "s",
      to: "c",
      points: [
        { x: 300, y: 190 },
        { x: 99, y: 190 },
      ],
      label: "SYN-ACK",
      style: "Dashed",
      extend: [],
    },
  ],
  [
    5520,
    1380,
    {
      kind: "Message",
      id: "m:4",
      op: 4,
      from: "c",
      to: "s",
      points: [
        { x: 99, y: 242 },
        { x: 300, y: 242 },
      ],
      label: "ACK",
      style: "Solid",
      extend: [
        { participant: "c", x: 99, top: 242, bottom: 346 },
        { participant: "s", x: 300, top: 242, bottom: 346 },
      ],
    },
  ],
  [
    6900,
    1080,
    {
      kind: "Message",
      id: "m:5",
      op: 5,
      from: "c",
      to: "s",
      points: [
        { x: 99, y: 294 },
        { x: 300, y: 294 },
      ],
      label: "GET /",
      style: "Solid",
      extend: [],
    },
  ],
  [
    7980,
    900,
    {
      kind: "Message",
      id: "m:6",
      op: 6,
      from: "s",
      to: "s",
      points: [
        { x: 300, y: 346 },
        { x: 344, y: 346 },
        { x: 344, y: 372 },
        { x: 300, y: 372 },
      ],
      label: "parse",
      style: "Solid",
      extend: [
        { participant: "c", x: 99, top: 346, bottom: 476 },
        { participant: "s", x: 300, top: 346, bottom: 476 },
      ],
    },
  ],
  [
    8880,
    480,
    {
      kind: "Message",
      id: "m:7",
      op: 7,
      from: "s",
      to: "c",
      points: [
        { x: 300, y: 424 },
        { x: 99, y: 424 },
      ],
      label: "200 OK",
      style: "Dashed",
      extend: [],
    },
  ],
];

const ARRAY: readonly Step[] = [
  [
    0,
    1920,
    {
      kind: "Cells",
      id: "arr",
      op: 0,
      label: "nums",
      cells: [
        { id: "arr.0", rect: { x: 86, y: 31, w: 48, h: 48 }, value: "", index: "0" },
        { id: "arr.1", rect: { x: 134, y: 31, w: 48, h: 48 }, value: "", index: "1" },
        { id: "arr.2", rect: { x: 182, y: 31, w: 48, h: 48 }, value: "", index: "2" },
        { id: "arr.3", rect: { x: 230, y: 31, w: 48, h: 48 }, value: "", index: "3" },
        { id: "arr.4", rect: { x: 278, y: 31, w: 48, h: 48 }, value: "", index: "4" },
        { id: "arr.5", rect: { x: 326, y: 31, w: 48, h: 48 }, value: "", index: "5" },
      ],
      vertical: false,
    },
  ],
  [
    2100,
    780,
    { kind: "Write", op: 1, target: "arr.0", label: "3", rect: { x: 86, y: 31, w: 48, h: 48 } },
  ],
  [
    2880,
    300,
    { kind: "Write", op: 2, target: "arr.1", label: "1", rect: { x: 134, y: 31, w: 48, h: 48 } },
  ],
  [
    3180,
    540,
    { kind: "Write", op: 3, target: "arr.2", label: "4", rect: { x: 182, y: 31, w: 48, h: 48 } },
  ],
  [
    3720,
    900,
    { kind: "Write", op: 4, target: "arr.3", label: "1", rect: { x: 230, y: 31, w: 48, h: 48 } },
  ],
  [
    4620,
    1500,
    {
      kind: "Annotate",
      id: "a:5",
      op: 5,
      target: "arr.2",
      mark: "Highlight",
      rect: { x: 182, y: 31, w: 48, h: 48 },
      text: "i",
      fill: "Mint",
    },
  ],
  [
    6120,
    1440,
    { kind: "Strike", op: 6, target: "arr.1", label: "5", rect: { x: 134, y: 31, w: 48, h: 48 } },
  ],
];

const TREE: readonly Step[] = [
  [
    0,
    1080,
    {
      kind: "Node",
      id: "n5",
      op: 0,
      shape: "Circle",
      label: "5",
      rect: { x: 72, y: 42, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: null,
    },
  ],
  [
    1080,
    1080,
    {
      kind: "Node",
      id: "n3",
      op: 1,
      shape: "Circle",
      label: "3",
      rect: { x: -88, y: 154, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: { start: { x: 100, y: 98 }, control: null, end: { x: -60, y: 154 } },
    },
  ],
  [
    2160,
    1200,
    {
      kind: "Node",
      id: "n8",
      op: 2,
      shape: "Circle",
      label: "8",
      rect: { x: 232, y: 154, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: { start: { x: 100, y: 98 }, control: null, end: { x: 260, y: 154 } },
    },
  ],
  [
    3360,
    1020,
    {
      kind: "Node",
      id: "n1",
      op: 3,
      shape: "Circle",
      label: "1",
      rect: { x: -168, y: 266, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: { start: { x: -60, y: 210 }, control: null, end: { x: -140, y: 266 } },
    },
  ],
  [
    4380,
    1140,
    {
      kind: "Node",
      id: "n4",
      op: 4,
      shape: "Circle",
      label: "4",
      rect: { x: -8, y: 266, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: { start: { x: -60, y: 210 }, control: null, end: { x: 20, y: 266 } },
    },
  ],
  [
    5520,
    1980,
    {
      kind: "Node",
      id: "n9",
      op: 5,
      shape: "Circle",
      label: "9",
      rect: { x: 312, y: 266, w: 56, h: 56 },
      stack: 1,
      fill: null,
      link: { start: { x: 260, y: 210 }, control: null, end: { x: 340, y: 266 } },
    },
  ],
  [
    7500,
    1320,
    {
      kind: "Annotate",
      id: "a:6",
      op: 6,
      target: "n4",
      mark: "Highlight",
      rect: { x: -8, y: 266, w: 56, h: 56 },
      text: null,
      fill: "Sky",
    },
  ],
];

const HASH: readonly Step[] = [
  [
    0,
    1740,
    {
      kind: "Cells",
      id: "ht",
      op: 0,
      label: "buckets",
      cells: [
        { id: "ht.0", rect: { x: 80, y: 60, w: 48, h: 48 }, value: "", index: "0" },
        { id: "ht.1", rect: { x: 80, y: 108, w: 48, h: 48 }, value: "", index: "1" },
        { id: "ht.2", rect: { x: 80, y: 156, w: 48, h: 48 }, value: "", index: "2" },
        { id: "ht.3", rect: { x: 80, y: 204, w: 48, h: 48 }, value: "", index: "3" },
      ],
      vertical: true,
    },
  ],
  [
    1740,
    150,
    {
      kind: "Node",
      id: "apple",
      op: 1,
      shape: "Service",
      label: "apple: 3",
      rect: { x: 192, y: 156, w: 125, h: 48 },
      stack: 1,
      fill: "Sky",
      link: null,
    },
  ],
  [
    1860,
    2220,
    {
      kind: "Edge",
      id: "ht.2>apple",
      op: 2,
      from: "ht.2",
      to: "apple",
      path: { start: { x: 128, y: 180 }, control: null, end: { x: 192, y: 180 } },
      label: null,
      style: "Solid",
    },
  ],
  [
    4080,
    150,
    {
      kind: "Node",
      id: "grape",
      op: 3,
      shape: "Service",
      label: "grape: 7",
      rect: { x: 381, y: 156, w: 128, h: 48 },
      stack: 1,
      fill: "Lilac",
      link: null,
    },
  ],
  [
    4200,
    2160,
    {
      kind: "Edge",
      id: "apple>grape",
      op: 4,
      from: "apple",
      to: "grape",
      path: { start: { x: 317, y: 180 }, control: null, end: { x: 381, y: 180 } },
      label: null,
      style: "Solid",
    },
  ],
  [
    6360,
    150,
    {
      kind: "Node",
      id: "kiwi",
      op: 5,
      shape: "Service",
      label: "kiwi: 1",
      rect: { x: 192, y: 60, w: 120, h: 48 },
      stack: 1,
      fill: "Mint",
      link: null,
    },
  ],
  [
    6480,
    600,
    {
      kind: "Edge",
      id: "ht.0>kiwi",
      op: 6,
      from: "ht.0",
      to: "kiwi",
      path: { start: { x: 128, y: 84 }, control: null, end: { x: 192, y: 84 } },
      label: null,
      style: "Solid",
    },
  ],
];

interface Scene {
  readonly title: string;
  readonly narration: string;
  readonly steps: readonly Step[];
}

const SCENES: readonly Scene[] = [
  {
    title: "Sequence",
    narration:
      "The client and the server shake hands. The client sends a SYN, the server answers, and the client acknowledges. Then data flows, the server parses it, and replies.",
    steps: SEQUENCE,
  },
  {
    title: "Array",
    narration:
      "An array of six slots starts empty. We store three, one, four and one. The cursor sits on slot two, and we overwrite one with five.",
    steps: ARRAY,
  },
  {
    title: "Tree",
    narration:
      "Insert five as the root, three to its left, eight to its right, one under three, four beside it, and nine under eight. Looking up four goes left, then right.",
    steps: TREE,
  },
  {
    title: "Hash table",
    narration:
      "The table has four buckets. Apple hashes to bucket two, grape collides there, so it chains behind apple, and kiwi goes to bucket zero.",
    steps: HASH,
  },
];

/** How far apart the scenes sit on the board, so each one's frame holds only itself. */
const SCENE_SPACING = 1400;
/** Room the camera keeps around a scene (the core's `CAMERA_MARGIN_UNITS`), labels included. */
const FRAME_MARGIN = 60;
const LABEL_ROOM = 40;
const SCENE_GAP_MS = 2500;
const FIRST_SCENE_MS = 1000;
const OPS_PER_SCENE = 100;

type Timed = { at: number; event: UiEvent };

const point = (x: number, y: number, dx: number) => ({ x: x + dx, y });

/** The op moved `dx` across, renumbered after the scenes before it. */
function moved(op: DrawOp, dx: number, base: number): DrawOp {
  const shift = (rect: Rect): Rect => ({ ...rect, x: rect.x + dx });
  const opNumber = base + op.op;
  switch (op.kind) {
    case "Node":
      return {
        ...op,
        op: opNumber,
        rect: shift(op.rect),
        link:
          op.link === null
            ? null
            : {
                start: point(op.link.start.x, op.link.start.y, dx),
                control: null,
                end: point(op.link.end.x, op.link.end.y, dx),
              },
      };
    case "Edge":
      return {
        ...op,
        op: opNumber,
        path: {
          start: point(op.path.start.x, op.path.start.y, dx),
          control: null,
          end: point(op.path.end.x, op.path.end.y, dx),
        },
      };
    case "Annotate":
    case "Strike":
    case "Write":
    case "Participant":
      return { ...op, op: opNumber, rect: shift(op.rect) };
    case "Trace":
      return { ...op, op: opNumber, points: op.points.map((p) => point(p.x, p.y, dx)) };
    case "Message":
      return {
        ...op,
        op: opNumber,
        points: op.points.map((p) => point(p.x, p.y, dx)),
        extend: op.extend.map((stretch) => ({ ...stretch, x: stretch.x + dx })),
      };
    case "Cells":
      return {
        ...op,
        op: opNumber,
        cells: op.cells.map((cell) => ({ ...cell, rect: shift(cell.rect) })),
      };
  }
}

/** What an op covers, roughly: enough for a camera frame with its labels inside. */
function area(op: DrawOp): Rect {
  const boxes: Rect[] = [];
  switch (op.kind) {
    case "Node":
    case "Annotate":
    case "Strike":
    case "Write":
      boxes.push(op.rect);
      break;
    case "Participant":
      boxes.push(op.rect, {
        x: op.rect.x,
        y: op.rect.y,
        w: op.rect.w,
        h: op.lifeline_bottom - op.rect.y,
      });
      break;
    case "Edge":
      boxes.push(span(op.path.start, op.path.end));
      break;
    case "Trace":
    case "Message":
      for (const p of op.points) {
        boxes.push({ x: p.x, y: p.y, w: 1, h: 1 });
      }
      break;
    case "Cells":
      boxes.push(...op.cells.map((cell) => cell.rect));
      break;
  }
  return boxes.reduce(union);
}

function span(a: { x: number; y: number }, b: { x: number; y: number }): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  };
}

function union(a: Rect, b: Rect): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, w: Math.max(a.x + a.w, b.x + b.w) - x, h: Math.max(a.y + a.h, b.y + b.h) - y };
}

function grow(rect: Rect, by: number): Rect {
  return { x: rect.x - by, y: rect.y - by, w: rect.w + 2 * by, h: rect.h + 2 * by };
}

/** When each scene starts, in session ms: the replay's chapters and the screenshots' times. */
export function sceneStartsMs(): number[] {
  const starts: number[] = [];
  let at = FIRST_SCENE_MS;
  for (const scene of SCENES) {
    starts.push(at);
    const last = scene.steps[scene.steps.length - 1];
    at += (last === undefined ? 0 : last[0] + last[1]) + SCENE_GAP_MS;
  }
  return starts;
}

function timed(): Timed[] {
  const events: Timed[] = [
    { at: 0, event: { type: "Orb", state: "Idle", level_percent: 0 } },
    {
      at: 200,
      event: {
        type: "Thread",
        steps: SCENES.map((scene) => scene.title),
        current: null,
        detours: [],
      },
    },
  ];
  const starts = sceneStartsMs();
  SCENES.forEach((scene, index) => {
    const start = starts[index] ?? 0;
    const dx = index * SCENE_SPACING;
    const ops = scene.steps.map(
      ([at, budget, op]) => [at, budget, moved(op, dx, index * OPS_PER_SCENE)] as const,
    );
    const frame = grow(ops.map(([, , op]) => area(op)).reduce(union), FRAME_MARGIN + LABEL_ROOM);
    events.push(
      { at: start - 300, event: { type: "Orb", state: "Speaking", level_percent: 60 } },
      {
        at: start - 200,
        event: { type: "Thread", steps: SCENES.map((s) => s.title), current: index, detours: [] },
      },
      { at: start, event: { type: "Camera", layer: 0, frame, focus: null } },
    );
    for (const [at, budget, op] of ops) {
      events.push({ at: start + at, event: { type: "OpStart", layer: 0, op, budget_ms: budget } });
    }
    const utterance = say("Assistant", start - 100, scene.narration);
    utterance.words.forEach((_, word) => {
      events.push({
        at: utterance.wordAt[word] ?? start,
        event: {
          type: "Caption",
          who: "Assistant",
          text: utterance.words.slice(Math.max(0, word - 11), word + 1).join(" "),
          is_final: false,
        },
      });
    });
  });
  const end = (starts[starts.length - 1] ?? 0) + 9000;
  events.push(
    {
      at: end,
      event: {
        type: "Chip",
        kind: "Source",
        text: "internal/net/tcp.go:88",
        source: { path: "internal/net/tcp.go", line: 88 },
      },
    },
    { at: end + 2500, event: { type: "Orb", state: "Idle", level_percent: 0 } },
  );
  return events
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

export function diagramsLines(): UiLogLine[] {
  return timed().map(({ at, event }) => ({ v: 1, t: at, k: "ui", event }));
}

export function diagramsJsonl(): string {
  return `${diagramsLines()
    .map((line) => JSON.stringify(line))
    .join("\n")}\n`;
}
