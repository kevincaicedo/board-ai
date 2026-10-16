// The views summoned by voice, as protocol events: the canvas's 8 · Library, 9 · Brain (the Now
// list for the harness project), Brain · memories and notes, and the recall example (the Raft
// snapshot board brought back), then "close that". Also the past sessions a library card can
// replay outside Tauri. Regenerate with `node src/replay/write-demo.ts` after changing this file.

import type { CoverShape } from "../protocol/generated/CoverShape.ts";
import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { SessionCard } from "../protocol/generated/SessionCard.ts";
import type { SessionRef } from "../protocol/generated/SessionRef.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import type { View } from "../protocol/generated/View.ts";
import { demoLines } from "./demo.ts";
import { say, type Utterance } from "./speech.ts";

interface Timed {
  readonly at: number;
  readonly event: UiEvent;
}

const rect = (x: number, y: number, w: number, h: number): Rect => ({ x, y, w, h });
const MINUTE = 60_000;

/** The day the views were read, and this session's id. */
export const TODAY = "2026-10-15";
export const THIS_SESSION = "2026-10-15-143210";

const REQUEST_PATH_COVER: CoverShape[] = [
  { rect: rect(80, 292, 42, 86), shape: "Actor" },
  { rect: rect(240, 296, 132, 76), shape: "Service" },
  { rect: rect(480, 296, 150, 76), shape: "Service" },
  { rect: rect(720, 296, 132, 76), shape: "Service" },
  { rect: rect(960, 280, 112, 108), shape: "Db" },
  { rect: rect(960, 460, 112, 108), shape: "Db" },
];

const VFS_COVER: CoverShape[] = [
  { rect: rect(260, 120, 640, 130), shape: "Service" },
  { rect: rect(260, 360, 640, 130), shape: "Service" },
  { rect: rect(260, 600, 640, 130), shape: "Service" },
];

/** The recall script's board (Session.dc.html, script 3), as the core would have drawn it. */
export const RAFT_BOARD: readonly DrawOp[] = [
  {
    kind: "Node",
    id: "leader",
    op: 1,
    shape: "Service",
    label: "leader",
    rect: rect(120, 250, 150, 64),
    stack: 1,
    fill: null,
    link: null,
  },
  {
    kind: "Node",
    id: "log",
    op: 2,
    shape: "Queue",
    label: "log",
    rect: rect(120, 400, 480, 56),
    stack: 1,
    fill: null,
    link: null,
  },
  {
    kind: "Annotate",
    id: "snapshot-mark",
    op: 3,
    target: "log",
    mark: "Underline",
    rect: rect(120, 400, 296, 56),
    text: "snapshot",
    fill: null,
  },
  {
    kind: "Edge",
    id: "log>snap",
    op: 4,
    from: "log",
    to: "snap",
    path: { start: { x: 268, y: 498 }, control: null, end: { x: 268, y: 552 } },
    label: null,
    style: "Solid",
  },
  {
    kind: "Node",
    id: "snap",
    op: 5,
    shape: "Service",
    label: "lastIncludedIndex 5",
    rect: rect(140, 560, 260, 58),
    stack: 1,
    fill: null,
    link: null,
  },
  {
    kind: "Node",
    id: "follower",
    op: 6,
    shape: "Service",
    label: "follower",
    rect: rect(600, 560, 150, 58),
    stack: 1,
    fill: null,
    link: null,
  },
  {
    kind: "Edge",
    id: "snap>follower",
    op: 7,
    from: "snap",
    to: "follower",
    path: { start: { x: 408, y: 589 }, control: null, end: { x: 590, y: 589 } },
    label: "InstallSnapshot",
    style: "Solid",
  },
];

function coverOf(ops: readonly DrawOp[]): CoverShape[] {
  return ops.flatMap((op) => (op.kind === "Node" ? [{ rect: op.rect, shape: op.shape }] : []));
}

export const SESSIONS: readonly SessionCard[] = [
  {
    id: "2026-10-14-091500",
    title: "Request path · API",
    date: "2026-10-14",
    duration_ms: 18 * MINUTE,
    cover: REQUEST_PATH_COVER,
  },
  {
    id: "2026-10-12-160200",
    title: "Raft · log compaction",
    date: "2026-10-12",
    duration_ms: 31 * MINUTE,
    cover: coverOf(RAFT_BOARD),
  },
  {
    id: "2026-10-09-110000",
    title: "Linux VFS",
    date: "2026-10-09",
    duration_ms: 24 * MINUTE,
    cover: VFS_COVER,
  },
];

const MORNING: SessionRef = { id: "2026-10-15-091200", title: "Request path · API", date: TODAY };
const RAFT_REF: SessionRef = {
  id: "2026-10-12-160200",
  title: "Raft · log compaction",
  date: "2026-10-12",
};
const REQUEST_REF: SessionRef = {
  id: "2026-10-14-091500",
  title: "Request path · API",
  date: "2026-10-14",
};
const VFS_REF: SessionRef = { id: "2026-10-09-110000", title: "Linux VFS", date: "2026-10-09" };
const SSD_REF: SessionRef = { id: "2026-10-15-120500", title: "SSD flash", date: TODAY };

const CACHE_NOTE =
  "Cache-aside: Redis first, Postgres on a miss, then write back with a 10-minute TTL.";
const SNAPSHOT_NOTE = "A snapshot replaces the log up to lastIncludedIndex.";
const DENTRY_NOTE = "The dentry cache maps path components to inodes.";

export const LIBRARY: View = { kind: "Library", sessions: [...SESSIONS], more_sessions: 0 };

export const TASKS: View = {
  kind: "Tasks",
  project: "Harness",
  tasks: [
    { id: "t27", title: "Check Redis TTLs", project: "Harness", from: MORNING },
    { id: "t28", title: "Read Raft §5.3", project: "Harness", from: RAFT_REF },
    { id: "t24", title: "Spike Tauri audio", project: "Harness", from: null },
  ],
  more_tasks: 0,
  notes: [
    { id: "n31", text: CACHE_NOTE, date: TODAY, from: MORNING, links: ["service.go:43"] },
    { id: "n22", text: SNAPSHOT_NOTE, date: "2026-10-12", from: RAFT_REF, links: [] },
    { id: "n17", text: DENTRY_NOTE, date: "2026-10-09", from: VFS_REF, links: [] },
  ],
};

export const MEMORY: View = {
  kind: "Memory",
  about_you: [
    { id: "m3", text: "Diagrams before code" },
    { id: "m4", text: "Examples in Go" },
    { id: "m9", text: "Unhurried, check in at boundaries" },
  ],
  systems: [
    { id: "m11", text: "Claude asks; the harness runs tools" },
    { id: "m12", text: "We use Kafka for the event bus" },
  ],
  notes: [
    {
      id: "n31",
      text: CACHE_NOTE,
      date: "2026-10-14",
      from: REQUEST_REF,
      links: ["service.go:43"],
    },
    {
      id: "n32",
      text: "Flash erases whole blocks, so new data goes to a fresh page and GC cleans up.",
      date: TODAY,
      from: SSD_REF,
      links: [],
    },
    { id: "n22", text: SNAPSHOT_NOTE, date: "2026-10-12", from: RAFT_REF, links: ["Raft §7"] },
    { id: "n17", text: DENTRY_NOTE, date: "2026-10-09", from: VFS_REF, links: ["fs/dcache.c"] },
  ],
  latest: {
    session: { ...MORNING, duration_ms: 22 * MINUTE, cover: REQUEST_PATH_COVER.slice(1, 5) },
    notes: 2,
    tasks: 1,
    open_questions: 1,
  },
};

/** The past board comes without a cover of its own: the view derives it from the ops. */
export const PAST_SESSION: View = {
  kind: "PastSession",
  session: { ...RAFT_REF, duration_ms: 31 * MINUTE, cover: [] },
  recap:
    "A snapshot replaces the log up to lastIncludedIndex. A follower that falls too far behind gets the snapshot instead of old entries.",
  layers: [{ title: null, ops: [...RAFT_BOARD] }],
  ops_left_out: 0,
};

/** Key moments, in session ms: when each view is fully in and its answer is under way. */
export const VIEW_FRAMES_MS = {
  library: 6_400,
  tasks: 14_800,
  memory: 23_500,
  recall: 33_500,
  closed: 41_000,
  replay: 52_000,
} as const;

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

type OrbName = Extract<UiEvent, { type: "Orb" }>["state"];

function orb(at: number, state: OrbName): Timed {
  return { at, event: { type: "Orb", state, level_percent: 0 } };
}

/** One spoken exchange: you ask, the orb listens and thinks, the view opens, BoardAI answers. */
function exchange(at: number, question: string, view: View | null, answers: string[]): Timed[] {
  const asked = say("User", at + 100, question);
  const answerAt = asked.endAt + 900;
  const timed: Timed[] = [
    orb(at, "Listening"),
    ...captions(asked),
    orb(asked.endAt + 200, "Thinking"),
  ];
  if (view !== null) {
    timed.push({ at: answerAt - 100, event: { type: "ViewShow", view, today: TODAY } });
  }
  let start = answerAt;
  timed.push(orb(start, "Speaking"));
  for (const answer of answers) {
    const said = say("Assistant", start, answer);
    timed.push(...captions(said));
    start = said.endAt + 150;
  }
  timed.push(orb(start + 200, "Idle"));
  return timed;
}

function timeline(): Timed[] {
  const timed: Timed[] = [
    { at: 0, event: { type: "SessionStart", session: THIS_SESSION } },
    orb(0, "Idle"),
    ...exchange(1200, "Show me my sessions.", LIBRARY, [
      "Here are your last three. Raft is where you left off.",
    ]),
    ...exchange(9500, "What’s on my plate for the harness?", TASKS, [
      "Three things. The TTL check is from today.",
    ]),
    ...exchange(17_500, "What do you remember about how I learn?", MEMORY, [
      "Diagrams first, examples in Go, and an unhurried pace.",
    ]),
    ...exchange(26_500, "What did we say about Raft snapshots?", PAST_SESSION, [
      "On October 12. Here’s the board.",
      "A snapshot replaces the log up to lastIncludedIndex, and a follower that falls behind gets the snapshot instead.",
    ]),
  ];
  const close = say("User", 38_600, "Close that.");
  timed.push(orb(38_500, "Listening"), ...captions(close));
  timed.push({ at: close.endAt + 400, event: { type: "ViewDismiss" } });
  timed.push({ at: close.endAt + 400, event: { type: "Acknowledge" } });
  timed.push(orb(close.endAt + 500, "Idle"));
  const raft = SESSIONS[1];
  if (raft !== undefined) {
    timed.push(
      ...exchange(42_000, "Play the Raft one.", { kind: "Replay", session: raft }, [
        "Playing it from the start.",
      ]),
    );
  }
  return timed
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

function jsonl(lines: readonly UiLogLine[]): string {
  return `${lines.map((line) => JSON.stringify(line)).join("\n")}\n`;
}

function lines(timed: readonly Timed[]): UiLogLine[] {
  return timed.map(({ at, event }) => ({ v: 1, t: at, k: "ui", event }));
}

export function viewsJsonl(): string {
  return jsonl(lines(timeline()));
}

/** The Raft session's own log: how the board on the recall card was first drawn. */
function raftSession(): Timed[] {
  const question = say("User", 1300, "What happens when the Raft log gets too long?");
  const first = say("Assistant", 4200, "The leader takes a snapshot of its state,");
  const second = say(
    "Assistant",
    first.endAt + 150,
    "which replaces the log up to lastIncludedIndex.",
  );
  const third = say(
    "Assistant",
    second.endAt + 150,
    "A follower that falls behind gets the snapshot instead.",
  );
  const draws: readonly [number, number][] = [
    [first.startAt + 300, 700],
    [second.wordAt[1] ?? second.startAt, 1100],
    [second.wordAt[3] ?? second.startAt, 700],
    [second.wordAt[4] ?? second.startAt, 300],
    [second.wordAt[5] ?? second.startAt, 1000],
    [third.wordAt[1] ?? third.startAt, 800],
    [third.wordAt[5] ?? third.startAt, 900],
  ];
  const timed: Timed[] = [
    { at: 0, event: { type: "SessionStart", session: SESSIONS[1]?.id ?? null } },
    { at: 0, event: { type: "Camera", layer: 0, frame: rect(0, 0, 1440, 900), focus: null } },
    orb(0, "Idle"),
    orb(1200, "Listening"),
    ...captions(question),
    orb(question.endAt + 200, "Thinking"),
    orb(4200, "Drawing"),
    ...captions(first),
    ...captions(second),
    ...captions(third),
    orb(third.endAt + 1200, "Idle"),
  ];
  RAFT_BOARD.forEach((op, index) => {
    const [at, budget] = draws[index] ?? [third.endAt, 600];
    timed.push({ at, event: { type: "OpStart", layer: 0, op, budget_ms: budget } });
  });
  return timed
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

/** The replayable past sessions served outside Tauri, by id (`/fixtures/sessions/<id>.…`). */
export function sessionFixtures(): [string, string][] {
  const requestPath = SESSIONS[0]?.id ?? "";
  const raft = SESSIONS[1]?.id ?? "";
  const start: UiLogLine = {
    v: 1,
    t: 0,
    k: "ui",
    event: { type: "SessionStart", session: requestPath },
  };
  return [
    [requestPath, jsonl([start, ...demoLines()])],
    [raft, jsonl(lines(raftSession()))],
  ];
}
