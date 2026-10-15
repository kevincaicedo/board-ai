import assert from "node:assert/strict";
import { test } from "node:test";
import type { UiEvent } from "./generated/UiEvent.ts";
import { parseLogLine, parseUiBatch, parseUiEvent } from "./parse.ts";

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
  { type: "LayerPush", layer: 1, title: "cache-aside" },
  { type: "LayerPop", layer: 1 },
  { type: "Camera", layer: 0, frame: { x: 0, y: 0, w: 1440, h: 900 }, focus: "redis" },
  {
    type: "SurfaceShow",
    surface: { kind: "Code", id: 1, path: "a.go", first_line: 42, lines: ["x"], highlight: 42 },
  },
  {
    type: "SurfaceShow",
    surface: { kind: "Compare", id: 2, options: ["a", "b"], criteria: ["c"] },
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
  { type: "SurfaceDismiss", id: 1 },
  { type: "Chip", kind: "Task", text: "Check TTLs" },
  { type: "Problem", spoken: "I lost the connection.", detail: "timeout" },
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
  ];
  for (const value of bad) {
    assert.equal(parseUiEvent(value), null, JSON.stringify(value));
  }
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
