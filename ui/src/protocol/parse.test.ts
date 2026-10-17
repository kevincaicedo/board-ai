import assert from "node:assert/strict";
import { test } from "node:test";
import type { DrawOp } from "./generated/DrawOp.ts";
import type { UiEvent } from "./generated/UiEvent.ts";
import type { View } from "./generated/View.ts";
import { parseLogLine, parseUiBatch, parseUiEvent, parseView, type RowSkips } from "./parse.ts";

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
            link: null,
          },
        ],
      },
      { title: "cache-aside", ops: [] },
    ],
    ops_left_out: 2,
  },
  { kind: "Replay", session: { ...CARD, cover: [] } },
];

/** The diagram ops (D-120), as the Rust side serializes them. */
const DIAGRAM_OPS: DrawOp[] = [
  {
    kind: "Node",
    id: "n3",
    op: 8,
    shape: "Circle",
    label: "3",
    rect: { x: -88, y: 154, w: 56, h: 56 },
    stack: 1,
    fill: null,
    link: { start: { x: 100, y: 98 }, control: null, end: { x: -60, y: 154 } },
  },
  {
    kind: "Participant",
    id: "c",
    op: 9,
    label: "Client",
    rect: { x: 47, y: 46, w: 105, h: 48 },
    lifeline_bottom: 242,
  },
  {
    kind: "Message",
    id: "m:10",
    op: 10,
    from: "c",
    to: "s",
    points: [
      { x: 99, y: 242 },
      { x: 300, y: 242 },
    ],
    label: "ACK",
    style: "Dashed",
    extend: [{ participant: "c", x: 99, top: 242, bottom: 346 }],
  },
  {
    kind: "Cells",
    id: "arr",
    op: 11,
    label: "",
    cells: [
      { id: "arr.0", rect: { x: 0, y: 0, w: 48, h: 48 }, value: "3", index: "0" },
      { id: "arr.1", rect: { x: 48, y: 0, w: 48, h: 48 }, value: "", index: null },
    ],
    vertical: false,
  },
  { kind: "Write", op: 12, target: "arr.1", label: "7", rect: { x: 48, y: 0, w: 48, h: 48 } },
  {
    kind: "Annotate",
    id: "a:13",
    op: 13,
    target: "arr.0",
    mark: "Highlight",
    rect: { x: 0, y: 0, w: 48, h: 48 },
    text: "i",
    fill: "Mint",
  },
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
      link: null,
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
      link: null,
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
      fill: null,
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
  { type: "Chip", kind: "Task", text: "Check TTLs", source: null },
  {
    type: "Chip",
    kind: "Source",
    text: "internal/user/service.go:43",
    source: { path: "internal/user/service.go", line: 43 },
  },
  ...DIAGRAM_OPS.map((op): UiEvent => ({ type: "OpStart", layer: 0, budget_ms: 600, op })),
  { type: "Problem", spoken: "I lost the connection.", detail: "timeout" },
  { type: "Suggestion", text: "Pick up Raft log compaction where we left off?" },
  { type: "Suggestion", text: null },
  { type: "SessionStart", session: "2026-10-15-143210" },
  { type: "SessionStart", session: null },
  ...EVERY_VIEW.map((view): UiEvent => ({ type: "ViewShow", view, today: "2026-10-15" })),
  { type: "ViewDismiss" },
  { type: "OpenSettings" },
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
        link: null,
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
        link: null,
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
        link: null,
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
        link: null,
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
        link: null,
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
    },
  });
  assert.ok(
    node?.type === "OpStart" &&
      node.op.kind === "Node" &&
      node.op.fill === null &&
      node.op.link === null,
  );
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

test("v3's diagram fields read a missing key as null, and malformed diagram ops are rejected", () => {
  const annotate = parseUiEvent({
    type: "OpStart",
    layer: 0,
    budget_ms: 300,
    op: {
      kind: "Annotate",
      id: "a",
      op: 1,
      target: "x",
      mark: "Circle",
      rect: { x: 0, y: 0, w: 1, h: 1 },
      text: null,
    },
  });
  assert.ok(
    annotate?.type === "OpStart" && annotate.op.kind === "Annotate" && annotate.op.fill === null,
  );
  assert.deepEqual(parseUiEvent({ type: "Chip", kind: "Source", text: "service.go" }), {
    type: "Chip",
    kind: "Source",
    text: "service.go",
    source: null,
  });
  const op = (value: Record<string, unknown>): unknown => ({
    type: "OpStart",
    layer: 0,
    budget_ms: 300,
    op: value,
  });
  const message = DIAGRAM_OPS[2] ?? {};
  const cells = DIAGRAM_OPS[3] ?? {};
  for (const bad of [
    op({ ...message, points: [{ x: 0, y: 0 }] }),
    op({ ...message, extend: [{ participant: "has space", x: 0, top: 0, bottom: 1 }] }),
    op({ ...cells, cells: [] }),
    op({ ...cells, vertical: "yes" }),
    op({ kind: "Write", op: 1, target: "a", rect: { x: 0, y: 0, w: 1, h: 1 } }),
    op({ ...(DIAGRAM_OPS[0] ?? {}), shape: "Hexagon" }),
    { type: "Chip", kind: "Source", text: "a.go:0", source: { path: "a.go", line: 0 } },
    { type: "Chip", kind: "Source", text: "a.go:3", source: { path: " ", line: 3 } },
  ]) {
    assert.equal(parseUiEvent(bad), null, JSON.stringify(bad));
  }
});

test("a batch keeps its good events and counts the rest", () => {
  const parsed = parseUiBatch({ at: 1200, events: [{ type: "Acknowledge" }, { type: "Nope" }, 7] });
  assert.deepEqual(parsed, {
    batch: { at: 1200, events: [{ type: "Acknowledge" }] },
    skipped: 2,
    skippedRows: 0,
  });
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

// A bad row is skipped (the tests above); a view whose own fields don't parse is rejected.
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
    show({ kind: "Library", sessions: [], more_sessions: 0 }, "15/10/2026"),
    { type: "ViewShow", view: { kind: "Library", sessions: [], more_sessions: 0 } },
    show({ kind: "Gallery" }),
    show({ kind: "Library", sessions: "none", more_sessions: 0 }),
    show({ kind: "Library", sessions: [card] }),
    show({ kind: "Tasks", project: 3, tasks: [], more_tasks: 0, notes: [] }),
    show({ kind: "Tasks", project: null, tasks: [], more_tasks: -1, notes: [] }),
    show({ kind: "Memory", about_you: [], systems: [], notes: [] }),
    show({ kind: "Memory", about_you: null, systems: [], notes: [], latest: null }),
    show({
      kind: "PastSession",
      session: { ...card, id: "" },
      recap: "",
      layers: [],
      ops_left_out: 0,
    }),
    show({ kind: "PastSession", session: card, recap: "", layers: {}, ops_left_out: 0 }),
    show({ kind: "Replay" }),
    show({ kind: "Replay", session: { ...card, title: 9 } }),
  ];
  for (const value of bad) {
    assert.equal(parseUiEvent(value), null, JSON.stringify(value));
  }
  assert.equal(parseView(null), null);
  assert.equal(parseView({ kind: "__proto__" }), null);
});

// Review finding 22 (UI side): the views list what the vault holds, hand-written files included,
// so one row the parser can't read (a note dated 15/10/2026) must not drop the whole view.
test("one hand-made note does not drop the whole Tasks view", () => {
  const skips: RowSkips = { rows: 0 };
  const event = parseUiEvent(
    {
      type: "ViewShow",
      view: {
        kind: "Tasks",
        project: null,
        tasks: [{ id: "t1", title: "Check Redis TTLs", project: "Harness", from: null }],
        more_tasks: 0,
        notes: [
          { id: "notes/handmade.md", text: "Kafka", date: "15/10/2026", from: null, links: [] },
          { id: "n2", text: "Cache-aside", date: "2026-10-15", from: null, links: ["a.go:4", 7] },
        ],
      },
      today: "2026-10-15",
    },
    skips,
  );
  assert.deepEqual(event, {
    type: "ViewShow",
    view: {
      kind: "Tasks",
      project: null,
      tasks: [{ id: "t1", title: "Check Redis TTLs", project: "Harness", from: null }],
      more_tasks: 0,
      notes: [{ id: "n2", text: "Cache-aside", date: "2026-10-15", from: null, links: ["a.go:4"] }],
    },
    today: "2026-10-15",
  });
  assert.equal(skips.rows, 2, "the bad note and the bad link are counted");
});

test("a view with an unknown day is shown; its dates read in full", () => {
  const event = {
    type: "ViewShow",
    view: {
      kind: "Memory",
      about_you: [{ id: "c0", text: "I prefer Go" }],
      systems: [],
      notes: [],
      latest: null,
    },
    today: "",
  };
  assert.notEqual(parseUiEvent(event), null, "the whole ViewShow was dropped");
});

test("every view keeps its good rows and counts the bad ones", () => {
  const card = { ...CARD, cover: [] };
  const count = (view: unknown): { rows: number; kept: View | null } => {
    const skips: RowSkips = { rows: 0 };
    return { kept: parseView(view, skips), rows: skips.rows };
  };
  const library = count({
    kind: "Library",
    sessions: [
      card,
      { ...card, id: "" },
      { ...card, date: "Oct 12" },
      { ...card, cover: [{ rect: { x: 0, y: 0, w: 1, h: 1 }, shape: "Blob" }] },
    ],
    more_sessions: 0,
  });
  assert.equal(library.rows, 3);
  assert.deepEqual(library.kept, {
    kind: "Library",
    sessions: [card, card],
    more_sessions: 0,
  });
  const memory = count({
    kind: "Memory",
    about_you: [
      { id: "", text: "x" },
      { id: "m1", text: "Go" },
    ],
    systems: [7],
    notes: [],
    latest: { session: card, notes: 1, tasks: 1 },
  });
  assert.equal(memory.rows, 3, "a row, a system and the unreadable digest");
  assert.deepEqual(memory.kept, {
    kind: "Memory",
    about_you: [{ id: "m1", text: "Go" }],
    systems: [],
    notes: [],
    latest: null,
  });
  const past = count({
    kind: "PastSession",
    session: card,
    recap: "",
    layers: [{ title: null, ops: [{ kind: "Node", id: "a b" }] }, { ops: [] }],
    ops_left_out: 0,
  });
  assert.equal(past.rows, 2, "an op and a layer");
  assert.deepEqual(past.kept, {
    kind: "PastSession",
    session: card,
    recap: "",
    layers: [{ title: null, ops: [] }],
    ops_left_out: 0,
  });
});

test("batches and log lines report the rows they skipped", () => {
  const note = { id: "n", text: "x", date: "15/10/2026", from: null, links: [] };
  const view = { kind: "Tasks", project: null, tasks: [], more_tasks: 0, notes: [note] };
  const show = { type: "ViewShow", view, today: "2026-10-15" };
  const batch = parseUiBatch({ at: 5, events: [show] });
  assert.equal(batch?.batch.events.length, 1);
  assert.equal(batch?.skippedRows, 1);
  const line = parseLogLine(JSON.stringify({ v: 1, t: 5, k: "ui", event: show }));
  assert.equal(line.kind === "ui" ? line.skippedRows : -1, 1);
});
