import assert from "node:assert/strict";
import { test } from "node:test";
import type { UiEvent } from "./generated/UiEvent.ts";
import type { View } from "./generated/View.ts";
import { parseLogLine, parseUiBatch, parseUiEvent, parseView } from "./parse.ts";

const CARD = {
  id: "2026-10-12-160200",
  title: "Raft · log compaction",
  date: "2026-10-12",
  duration_ms: 1_860_000,
  cover: [{ rect: { x: 0, y: 0, w: 150, h: 64 }, shape: "Service" }],
} as const;

/** One well-formed example of every view, as the Rust side serializes them. */
const EVERY_VIEW: View[] = [
  { kind: "Library", sessions: [{ ...CARD, cover: [...CARD.cover] }], more_sessions: 3 },
  {
    kind: "Tasks",
    project: "Harness",
    tasks: [
      {
        id: "t27",
        title: "Check Redis TTLs",
        project: "Harness",
        from: { id: "2026-10-15-091200", title: "Request path", date: "2026-10-15" },
      },
      { id: "projects/inbox.md#4", title: "Hand-written", project: "Inbox", from: null },
    ],
    more_tasks: 0,
    notes: [{ id: "n3", text: "Cache-aside.", date: "", from: null, links: ["service.go:43"] }],
  },
  { kind: "Tasks", project: null, tasks: [], more_tasks: 0, notes: [] },
  {
    kind: "Memory",
    about_you: [{ id: "m1", text: "Diagrams before code" }],
    systems: [{ id: "m2", text: "We use Kafka" }],
    notes: [],
    latest: { session: { ...CARD, cover: [] }, notes: 2, tasks: 1, open_questions: 0 },
  },
  { kind: "Memory", about_you: [], systems: [], notes: [], latest: null },
  {
    kind: "PastSession",
    session: { ...CARD, cover: [] },
    recap: "",
    layers: [
      {
        title: null,
        ops: [
          {
            kind: "Node",
            id: "leader",
            op: 1,
            shape: "Service",
            label: "leader",
            rect: { x: 120, y: 250, w: 150, h: 64 },
            stack: 1,
            fill: null,
          },
        ],
      },
      { title: "cache-aside", ops: [] },
    ],
    ops_left_out: 2,
  },
  { kind: "Replay", session: { ...CARD, cover: [] } },
];

/** One well-formed example of every event, as the Rust side serializes them. */
const EVERY_EVENT: UiEvent[] = [
  { type: "Orb", state: "Speaking", level_percent: 40 },
  { type: "Acknowledge" },
  { type: "Caption", who: "Assistant", text: "Sure", is_final: false },
  { type: "Thread", steps: ["Client", "Edge"], current: null, detours: [] },
  {
    type: "OpStart",
    layer: 0,
    budget_ms: 900,
    op: {
      kind: "Node",
      id: "lb",
      op: 3,
      shape: "Service",
      label: "Load balancer",
      rect: { x: 0, y: 0, w: 160, h: 64 },
      stack: 1,
      fill: "Lilac",
    },
  },
  {
    type: "OpStart",
    layer: 0,
    budget_ms: 900,
    op: {
      kind: "Node",
      id: "client",
      op: 2,
      shape: "Actor",
      label: "Client",
      rect: { x: 0, y: 0, w: 42, h: 86 },
      stack: 1,
      fill: null,
    },
  },
  {
    type: "OpStart",
    layer: 0,
    budget_ms: 400,
    op: {
      kind: "Edge",
      id: "cdn>lb",
      op: 4,
      from: "cdn",
      to: "lb",
      path: { start: { x: 0, y: 0 }, control: null, end: { x: 60, y: 0 } },
      label: null,
      style: "Dashed",
    },
  },
  {
    type: "OpStart",
    layer: 1,
    budget_ms: 300,
    op: {
      kind: "Annotate",
      id: "ttl",
      op: 5,
      target: "redis",
      mark: "Circle",
      rect: { x: -4, y: 2, w: 10, h: 10 },
      text: null,
    },
  },
  {
    type: "OpStart",
    layer: 0,
    budget_ms: 300,
    op: {
      kind: "Strike",
      op: 6,
      target: "sqs",
      label: "Kafka",
      rect: { x: 0, y: 0, w: 40, h: 26 },
    },
  },
  {
    type: "OpStart",
    layer: 1,
    budget_ms: 1300,
    op: { kind: "Trace", id: "t", op: 7, points: [{ x: 1, y: 2 }] },
  },
  { type: "Freeze", layer: 0 },
  { type: "Resume", layer: 0, budget_ms: 800 },
  { type: "LayerPush", layer: 1, title: "cache-aside", question: "why Redis first?" },
  { type: "LayerPush", layer: 2, title: "ttl", question: null },
  { type: "LayerPop", layer: 1 },
  { type: "Camera", layer: 0, frame: { x: 0, y: 0, w: 1440, h: 900 }, focus: "redis" },
  {
    type: "SurfaceShow",
    surface: { kind: "Code", id: 1, path: "a.go", first_line: 42, lines: ["x"], highlight: 42 },
  },
  {
    type: "SurfaceShow",
    surface: { kind: "Compare", id: 2, options: ["a", "b"], criteria: ["c"], title: "a vs b" },
  },
  {
    type: "SurfaceShow",
    surface: { kind: "Compare", id: 4, options: ["a"], criteria: ["c"], title: null },
  },
  { type: "SurfaceShow", surface: { kind: "Recap", id: 3, bullets: ["one"] } },
  { type: "SurfaceUpdate", id: 1, update: { kind: "Highlight", line: 43 } },
  {
    type: "SurfaceUpdate",
    id: 2,
    update: { kind: "Cell", row: 0, col: 0, mark: { kind: "Text", text: "4" } },
  },
  {
    type: "SurfaceUpdate",
    id: 2,
    update: { kind: "Cell", row: 1, col: 0, mark: { kind: "Depends" } },
  },
  {
    type: "SurfaceUpdate",
    id: 1,
    update: { kind: "Mark", line: 43, columns: { begin: 17, end: 29 }, mark: "Underline" },
  },
  { type: "SurfaceDismiss", id: 1 },
  { type: "Chip", kind: "Task", text: "Check TTLs" },
  { type: "Problem", spoken: "I lost the connection.", detail: "timeout" },
  { type: "Suggestion", text: "Pick up Raft log compaction where we left off?" },
  { type: "Suggestion", text: null },
  { type: "SessionStart", session: "2026-10-15-143210" },
  { type: "SessionStart", session: null },
  ...EVERY_VIEW.map((view): UiEvent => ({ type: "ViewShow", view, today: "2026-10-15" })),
  { type: "ViewDismiss" },
];

test("every protocol event round-trips through the parser unchanged", () => {
  for (const event of EVERY_EVENT) {
    assert.deepEqual(parseUiEvent(JSON.parse(JSON.stringify(event))), event, event.type);
  }
});

test("unknown types and malformed fields are rejected, never thrown", () => {
  const bad: unknown[] = [
    null,
    42,
    "Orb",
    [],
    {},
    { type: "Teleport" },
    { type: "toString" },
    { type: "__proto__" },
    { type: "Orb", state: "Idle", level_percent: 101 },
    { type: "Orb", state: "Idle", level_percent: 1.5 },
    { type: "Caption", who: "Robot", text: "hi", is_final: false },
    { type: "Freeze", layer: 256 },
    { type: "Freeze", layer: -1 },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: {
        kind: "Node",
        id: "",
        op: 1,
        shape: "Service",
        label: "x",
        rect: { x: 0, y: 0, w: 1, h: 1 },
        stack: 1,
        fill: null,
      },
    },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: {
        kind: "Node",
        id: "a b",
        op: 1,
        shape: "Service",
        label: "x",
        rect: { x: 0, y: 0, w: 1, h: 1 },
        stack: 1,
        fill: null,
      },
    },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: {
        kind: "Node",
        id: "a",
        op: 1,
        shape: "Blob",
        label: "x",
        rect: { x: 0, y: 0, w: 1, h: 1 },
        stack: 1,
        fill: null,
      },
    },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: {
        kind: "Node",
        id: "a",
        op: 1,
        shape: "Service",
        label: "x",
        rect: { x: 0, y: 0, w: -1, h: 1 },
        stack: 1,
        fill: null,
      },
    },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: { kind: "Trace", id: "t", op: 1, points: [{ x: 1 }] },
    },
    {
      type: "SurfaceShow",
      surface: { kind: "Code", id: 1, path: "a", first_line: 1, lines: [1], highlight: null },
    },
    {
      type: "SurfaceUpdate",
      id: 1,
      update: { kind: "Cell", row: 0, col: 0, mark: { kind: "Maybe" } },
    },
    { type: "Camera", layer: 0, frame: { x: 0, y: 0, w: 1, h: 1 } },
    {
      type: "OpStart",
      layer: 0,
      budget_ms: 5,
      op: {
        kind: "Node",
        id: "a",
        op: 1,
        shape: "Service",
        label: "x",
        rect: { x: 0, y: 0, w: 1, h: 1 },
        stack: 1,
        fill: "Plaid",
      },
    },
    { type: "LayerPush", layer: 1, title: "t", question: 7 },
    {
      type: "SurfaceShow",
      surface: { kind: "Compare", id: 2, options: ["a"], criteria: ["c"], title: false },
    },
    {
      type: "SurfaceUpdate",
      id: 1,
      update: { kind: "Mark", line: 43, columns: { begin: 5, end: 5 }, mark: "Underline" },
    },
    {
      type: "SurfaceUpdate",
      id: 1,
      update: { kind: "Mark", line: 43, columns: { begin: 9, end: 2 }, mark: "Underline" },
    },
    {
      type: "SurfaceUpdate",
      id: 1,
      update: { kind: "Mark", line: 43, columns: { begin: 1, end: 2 }, mark: "Zigzag" },
    },
    { type: "Suggestion" },
    { type: "Suggestion", text: 3 },
  ];
  for (const value of bad) {
    assert.equal(parseUiEvent(value), null, JSON.stringify(value));
  }
});

test("protocol v1 logs still replay: v2's optional fields read a missing key as null", () => {
  const node = parseUiEvent({
    type: "OpStart",
    layer: 0,
    budget_ms: 900,
    op: {
      kind: "Node",
      id: "lb",
      op: 3,
      shape: "Service",
      label: "LB",
      rect: { x: 0, y: 0, w: 160, h: 64 },
      stack: 1,
      fill: null,
    },
  });
  assert.ok(node?.type === "OpStart" && node.op.kind === "Node" && node.op.fill === null);
  assert.deepEqual(parseUiEvent({ type: "LayerPush", layer: 1, title: "t" }), {
    type: "LayerPush",
    layer: 1,
    title: "t",
    question: null,
  });
  const compare = parseUiEvent({
    type: "SurfaceShow",
    surface: { kind: "Compare", id: 2, options: ["a"], criteria: ["c"] },
  });
  assert.ok(
    compare?.type === "SurfaceShow" &&
      compare.surface.kind === "Compare" &&
      compare.surface.title === null,
  );
});

test("a batch keeps its good events and counts the rest", () => {
  const parsed = parseUiBatch({ at: 1200, events: [{ type: "Acknowledge" }, { type: "Nope" }, 7] });
  assert.deepEqual(parsed, { batch: { at: 1200, events: [{ type: "Acknowledge" }] }, skipped: 2 });
  assert.equal(parseUiBatch({ at: -1, events: [] }), null);
  assert.equal(parseUiBatch({ at: 5 }), null);
  assert.equal(parseUiBatch("batch"), null);
});

test("log lines: UI lines parse, input lines are set aside, bad lines say why", () => {
  assert.equal(parseLogLine('{"v":1,"t":5,"k":"ui","event":{"type":"Acknowledge"}}').kind, "ui");
  assert.equal(parseLogLine('{"v":1,"t":5,"k":"input","input":"Tick"}').kind, "other");
  const reasons = [
    "{",
    '"just a string"',
    '{"v":9,"t":5,"k":"ui","event":{"type":"Acknowledge"}}',
    '{"v":1,"t":1.5,"k":"ui","event":{"type":"Acknowledge"}}',
    '{"v":1,"t":5,"event":{"type":"Acknowledge"}}',
    '{"v":1,"t":5,"k":"ui","event":{"type":"Nope"}}',
  ].map((line) => {
    const result = parseLogLine(line);
    return result.kind === "malformed" ? result.reason : result.kind;
  });
  assert.deepEqual(reasons, [
    "not JSON",
    "not an object",
    "unsupported version",
    "bad time",
    "missing kind",
    "unknown or malformed event",
  ]);
});

test("every view round-trips through the parser unchanged", () => {
  for (const view of EVERY_VIEW) {
    assert.deepEqual(parseView(JSON.parse(JSON.stringify(view))), view, view.kind);
  }
});

test("malformed views and session events are rejected, never thrown", () => {
  const show = (view: unknown, today: unknown = "2026-10-15"): unknown => ({
    type: "ViewShow",
    view,
    today,
  });
  const card = { ...CARD, cover: [] };
  const bad: unknown[] = [
    { type: "SessionStart" },
    { type: "SessionStart", session: "" },
    { type: "SessionStart", session: 7 },
    show({ kind: "Library", sessions: [], more_sessions: 0 }, ""),
    show({ kind: "Library", sessions: [], more_sessions: 0 }, "15/10/2026"),
    { type: "ViewShow", view: { kind: "Library", sessions: [], more_sessions: 0 } },
    show({ kind: "Gallery" }),
    show({ kind: "Library", sessions: [{ ...card, id: "" }], more_sessions: 0 }),
    show({ kind: "Library", sessions: [{ ...card, date: "Oct 12" }], more_sessions: 0 }),
    show({ kind: "Library", sessions: [{ ...card, duration_ms: -1 }], more_sessions: 0 }),
    show({
      kind: "Library",
      sessions: [{ ...card, cover: [{ rect: { x: 0, y: 0, w: 1, h: 1 }, shape: "Blob" }] }],
      more_sessions: 0,
    }),
    show({ kind: "Library", sessions: [card] }),
    show({ kind: "Tasks", project: 3, tasks: [], more_tasks: 0, notes: [] }),
    show({
      kind: "Tasks",
      project: null,
      tasks: [{ id: "t1", title: "x", project: "p" }],
      more_tasks: 0,
      notes: [],
    }),
    show({
      kind: "Tasks",
      project: null,
      tasks: [],
      more_tasks: 0,
      notes: [{ id: "n1", text: "x", date: "", from: null, links: [1] }],
    }),
    show({
      kind: "Memory",
      about_you: [{ id: "", text: "x" }],
      systems: [],
      notes: [],
      latest: null,
    }),
    show({ kind: "Memory", about_you: [], systems: [], notes: [] }),
    show({
      kind: "Memory",
      about_you: [],
      systems: [],
      notes: [],
      latest: { session: card, notes: 1, tasks: 1 },
    }),
    show({ kind: "PastSession", session: card, recap: "", layers: [{ ops: [] }], ops_left_out: 0 }),
    show({
      kind: "PastSession",
      session: card,
      recap: "",
      layers: [{ title: null, ops: [{ kind: "Node", id: "a b" }] }],
      ops_left_out: 0,
    }),
    show({ kind: "Replay" }),
    show({ kind: "Replay", session: { ...card, title: 9 } }),
  ];
  for (const value of bad) {
    assert.equal(parseUiEvent(value), null, JSON.stringify(value));
  }
  assert.equal(parseView(null), null);
  assert.equal(parseView({ kind: "__proto__" }), null);
});
