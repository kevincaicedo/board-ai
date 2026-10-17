// Drawing together as protocol events (13 · You draw): you ask, BoardAI starts answering, you cut
// it off with "Let me draw what I mean.", BoardAI says its one line and watches; you draw artboard
// 13's Client → API → Postgres with a cache under the API, labelling them as you go; "Does this
// make sense?" finishes the drawing, and BoardAI answers by marking your API and joining your
// cache to it.
//
// Your shapes are what the core echoes for those gestures (crates/core board::user): ids `u1`…,
// arrows connected to the shapes their ends land on (an arrow drawn before its target connects
// when the target is drawn), pen strokes kept as drawn, the lid of the Postgres cylinder joined to
// its body. World units are the artboard's SVG coordinates plus its 80 px offset; with no camera
// frame the main layer draws at identity, so they land where the artboard draws them. Regenerate
// with `node src/replay/write-demo.ts`.

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Point } from "../protocol/generated/Point.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import type { UserShape } from "../protocol/generated/UserShape.ts";
import type { UserShapeKind } from "../protocol/generated/UserShapeKind.ts";
import { say, type Utterance } from "./speech.ts";

/** The artboard's drawing sits 80 px down the stage (its SVG's `top: 80px`). */
const DY = 80;

/** The core's line when you start drawing (engine::text::DRAW_GO_AHEAD_LINE). */
export const GO_AHEAD_LINE =
  "Go ahead. Say what each part is as you draw, and “done” when you're ready.";

interface Timed {
  readonly at: number;
  readonly event: UiEvent;
}

function at(x: number, y: number): Point {
  return { x, y: y + DY };
}

function box(x: number, y: number, w: number, h: number): Rect {
  return { x, y: y + DY, w, h };
}

/** Points along a cubic Bézier, as a pen sampled at a steady pace would leave them. */
function cubic(from: Point, c1: Point, c2: Point, to: Point, steps: number): Point[] {
  const points: Point[] = [];
  for (let step = 1; step <= steps; step += 1) {
    const t = step / steps;
    const u = 1 - t;
    points.push({
      x: Math.round(
        u * u * u * from.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t ** 3 * to.x,
      ),
      y: Math.round(
        u * u * u * from.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t ** 3 * to.y,
      ),
    });
  }
  return points;
}

/** The cylinder's body as one pen stroke: the top's back arc, the right side, the base, back up. */
const BODY: Point[] = [
  at(852, 246),
  ...cubic(at(852, 246), at(852, 236), at(940, 234), at(942, 246), 10),
  at(943, 282),
  at(943, 318),
  ...cubic(at(943, 318), at(942, 330), at(853, 331), at(852, 318), 10),
  at(852, 282),
  at(852, 247),
];

/** The cylinder's lid, drawn second over the body, so the core joins it to the same shape. */
const LID: Point[] = [
  at(852, 246),
  ...cubic(at(852, 246), at(852, 258), at(942, 258), at(942, 246), 10),
];

function boundsOf(strokes: readonly (readonly Point[])[]): Rect {
  const all = strokes.flat();
  const xs = all.map((point) => point.x);
  const ys = all.map((point) => point.y);
  const left = Math.min(...xs);
  const top = Math.min(...ys);
  return { x: left, y: top, w: Math.max(...xs) - left, h: Math.max(...ys) - top };
}

function shape(
  id: string,
  kind: UserShapeKind,
  label: string | null = null,
  note: string | null = null,
): UiEvent {
  const echoed: UserShape = { id, kind, label, note };
  return { type: "UserShape", layer: 0, shape: echoed };
}

const CLIENT: UserShapeKind = { kind: "Box", rect: box(262, 246, 120, 72) };
const API: UserShapeKind = { kind: "Box", rect: box(530, 238, 142, 88) };
const CACHE: UserShapeKind = { kind: "Box", rect: box(548, 430, 116, 60) };

function arrow(start: Point, end: Point, from: string | null, to: string | null): UserShapeKind {
  return { kind: "Arrow", start, end, from, to };
}

const TO_API = (to: string | null): UserShapeKind => arrow(at(382, 284), at(522, 282), "u1", to);
const TO_POSTGRES = (to: string | null): UserShapeKind =>
  arrow(at(672, 283), at(842, 283), "u3", to);
const POSTGRES = (strokes: Point[][]): UserShapeKind => ({
  kind: "Ink",
  strokes,
  bounds: boundsOf(strokes),
});

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

/** A label you say while you draw: the hint under the shape while you speak, then the label. */
function spokenLabel(utterance: Utterance, target: string, labelled: UiEvent): Timed[] {
  const timed = captions(utterance);
  utterance.words.forEach((_, index) => {
    timed.push({
      at: utterance.wordAt[index] ?? utterance.startAt,
      event: {
        type: "LabelHint",
        hint: { target, words: utterance.words.slice(0, index + 1).join(" ") },
      },
    });
  });
  const end = utterance.endAt + END_OF_TURN_MS;
  timed.push({ at: end, event: { type: "LabelHint", hint: null } });
  timed.push({ at: end, event: labelled });
  return timed;
}

/** Silence after a finished-sounding final before the core ends the turn (LLD §2.2). */
const END_OF_TURN_MS = 250;

function orb(at: number, state: Extract<UiEvent, { type: "Orb" }>["state"], level = 0): Timed {
  return { at, event: { type: "Orb", state, level_percent: level } };
}

/** Moments of the flow, in session ms. */
const ASK = say("User", 600, "How would you put a cache in front of our API?");
const ANSWER = say("Assistant", ASK.endAt + 900, "A cache usually sits beside the API, so it—", {
  cutAfterMs: 150,
});
const LET_ME = say("User", ANSWER.endAt - 60, "Let me draw what I mean.");
const DRAW_AT = LET_ME.endAt + END_OF_TURN_MS;
const GO_AHEAD = say("Assistant", DRAW_AT + 450, GO_AHEAD_LINE);
const T = GO_AHEAD.endAt + 400;
const CLIENT_SAID = say("User", T + 900, "This is the client.");
const API_SAID = say("User", CLIENT_SAID.endAt + 2600, "Then the API.");
const POSTGRES_SAID = say("User", API_SAID.endAt + 3800, "And this is Postgres,");
const NOTE_SAID = say("User", POSTGRES_SAID.endAt + 900, "the source of truth.");
const CACHE_AT = NOTE_SAID.endAt + 1800;
const CACHE_SAID = say("User", CACHE_AT + 900, "And this one is the cache.");
const QUESTION = say("User", CACHE_SAID.endAt + 1600, "Does this make sense?");
const DONE_AT = QUESTION.endAt + END_OF_TURN_MS;
const REPLY = say(
  "Assistant",
  DONE_AT + 1100,
  "Yes. Your API checks the cache first, and only goes to Postgres on a miss.",
);

/**
 * The artboard's moment: the cache box is on the board and you are saying "…and this one is the
 * cache", the hint under it.
 */
export const DRAWING_ARTBOARD_MS =
  (CACHE_SAID.wordAt[CACHE_SAID.wordAt.length - 1] ?? CACHE_SAID.startAt) + 120;
/** BoardAI's answer: its mark on your API and the line from your cache, both drawn. */
export const DRAWING_ANSWER_MS = REPLY.endAt;

/** BoardAI's reply ops: a ring around your API and an arrow from your cache to it. */
function reply(): Timed[] {
  const ring = REPLY.wordAt[1] ?? REPLY.startAt;
  const join = REPLY.wordAt[4] ?? REPLY.startAt;
  const apiRing: DrawOp = {
    kind: "Annotate",
    id: "a:0",
    op: 0,
    target: "u3",
    mark: "Circle",
    rect: box(520, 228, 162, 108),
    text: null,
    fill: null,
  };
  const cacheToApi: DrawOp = {
    kind: "Edge",
    id: "u6>u3",
    op: 1,
    from: "u6",
    to: "u3",
    path: { start: at(606, 430), control: null, end: at(601, 326) },
    label: "read first",
    style: "Solid",
  };
  // The core frames the layer's drawing, yours included, with its first stroke (camera follow).
  const frame: Rect = { x: 262 - 60, y: 228 + DY - 60, w: 982 - 262 + 120, h: 490 - 228 + 120 };
  return [
    { at: ring, event: { type: "Camera", layer: 0, frame, focus: null, enlarge: false } },
    { at: ring, event: { type: "OpStart", layer: 0, op: apiRing, budget_ms: join - ring } },
    { at: join, event: { type: "OpStart", layer: 0, op: cacheToApi, budget_ms: 1400 } },
    orb(ring, "Drawing", 70),
    orb(join + 1400 + 800, "Speaking", 70),
  ];
}

function timeline(): Timed[] {
  const body = [BODY];
  const cylinder = [BODY, LID];
  const timed: Timed[] = [
    orb(0, "Idle"),
    orb(ASK.startAt, "Listening", 60),
    ...captions(ASK),
    orb(ASK.endAt + 300, "Thinking"),
    orb(ANSWER.startAt, "Speaking", 70),
    ...captions(ANSWER),
    orb(LET_ME.startAt, "Listening", 60),
    ...captions(LET_ME),
    { at: DRAW_AT, event: { type: "DrawMode", drawing: true } },
    orb(DRAW_AT, "Thinking"),
    orb(GO_AHEAD.startAt, "Speaking", 70),
    ...captions(GO_AHEAD),
    orb(GO_AHEAD.endAt + 200, "Listening"),
    { at: T, event: shape("u1", CLIENT) },
    ...spokenLabel(CLIENT_SAID, "u1", shape("u1", CLIENT, "Client")),
    { at: CLIENT_SAID.endAt + 900, event: shape("u2", TO_API(null)) },
    { at: CLIENT_SAID.endAt + 1900, event: shape("u3", API) },
    { at: CLIENT_SAID.endAt + 1900, event: shape("u2", TO_API("u3")) },
    ...spokenLabel(API_SAID, "u3", shape("u3", API, "API")),
    { at: API_SAID.endAt + 900, event: shape("u4", TO_POSTGRES(null)) },
    { at: API_SAID.endAt + 2200, event: shape("u5", POSTGRES(body)) },
    { at: API_SAID.endAt + 2200, event: shape("u4", TO_POSTGRES("u5")) },
    { at: API_SAID.endAt + 3000, event: shape("u5", POSTGRES(cylinder)) },
    ...spokenLabel(POSTGRES_SAID, "u5", shape("u5", POSTGRES(cylinder), "Postgres")),
    ...spokenLabel(NOTE_SAID, "u5", shape("u5", POSTGRES(cylinder), "Postgres", "source of truth")),
    { at: CACHE_AT, event: shape("u6", CACHE) },
    ...spokenLabel(CACHE_SAID, "u6", shape("u6", CACHE, "cache")),
    ...captions(QUESTION),
    { at: DONE_AT, event: { type: "DrawMode", drawing: false } },
    orb(DONE_AT, "Thinking"),
    orb(REPLY.startAt, "Speaking", 70),
    ...captions(REPLY),
    ...reply(),
    orb(REPLY.endAt + 300, "Idle"),
  ];
  // Stable by time: events at the same moment keep the order above.
  return timed
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

export function drawingLines(): UiLogLine[] {
  return timeline().map(({ at: t, event }) => ({ v: 1, t, k: "ui", event }));
}

export function drawingJsonl(): string {
  return `${drawingLines()
    .map((line) => JSON.stringify(line))
    .join("\n")}\n`;
}
