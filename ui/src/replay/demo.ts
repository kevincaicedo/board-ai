// The design canvas's live demo (Main.dc.html, frames 1→7) as protocol events: the request path
// for a user API, the "wait — why Redis first?" detour with its code walkthrough and trace, a
// captured note and task, and the resume that finishes the frozen Redis box.
//
// Times are the design scene's own (its buildScene, in ms), so the replay can be checked side by
// side with the canvas. An op starts at the design's first stroke time minus the pen's travel to
// it; budgets run to the next op, as the core computes them (LLD §2.4). Regenerate the fixture
// with `node src/replay/write-demo.ts` after changing this file.

import type { DrawOp } from "../protocol/generated/DrawOp.ts";
import type { Point } from "../protocol/generated/Point.ts";
import type { Rect } from "../protocol/generated/Rect.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { UiLogLine } from "../protocol/generated/UiLogLine.ts";
import { say, type Utterance } from "./speech.ts";

/** The core clamps budgets to this range (LLD §2.4). */
const BUDGET_MIN_MS = 150;
const BUDGET_MAX_MS = 2500;

const STEPS = ["Client", "Edge", "LB", "Service", "Postgres", "Response"];

/** Key moments of the design's scene, in session ms. */
const AT = {
  listen: 2050,
  think: 4780,
  speak: 5580,
  condense: 6651,
  thread: 6751,
  draw: 7771,
  step1: 9046,
  step2: 10550,
  step3: 13019,
  interrupt: 20169,
  detour: 20229,
  branch: 20669,
  code: 27827,
  line43: 28974,
  line44: 31112,
  trace: 31212,
  note: 33842,
  task: 35477,
  resume: 37409,
  resumeDraw: 38009,
  step4: 39512,
  home: 42812,
} as const;

const SPEECH: readonly Utterance[] = [
  say("User", 2300, "Walk me through the request path for our user API."),
  say("Assistant", 5580, "Sure, let me sketch it."),
  say("Assistant", 7721, "A request starts at the client and hits the CDN."),
  say("Assistant", 10385, "Misses go on to the load balancer,"),
  say("Assistant", 12176, "which spreads them across three API pods."),
  say("Assistant", 15797, "To find a user, before touching Postgres, the service checks Redis.", {
    endPauseMs: 100,
  }),
  say("Assistant", 19159, "If it hits, we sk—", { cutAfterMs: 130 }),
  say("User", 20269, "Wait — why Redis first?"),
  say("Assistant", 21990, "Cache-aside. Redis answers from memory, so it goes first."),
  say("Assistant", 25195, "On a miss, read Postgres, then write it back."),
  say("Assistant", 28277, "In the code, line 43 asks Redis first."),
  say("Assistant", 30544, "On a hit, line 44 returns right away."),
  say("User", 33278, "Note that, and add a task to check our TTLs."),
  say("Assistant", 35927, "Noted. Task added."),
  say("Assistant", 37709, "So, back to the request. On a miss, it goes to Postgres."),
];

const CODE_LINES = [
  "func (s *UserService) GetUser(ctx context.Context, id string) (*User, error) {",
  "    if u, err := s.cache.Get(ctx, userKey(id)); err == nil {",
  "        return u, nil // cache hit",
  "    }",
  "    u, err := s.db.FindUser(ctx, id)",
  "    if err != nil {",
  "        return nil, err",
  "    }",
  "    s.cache.Set(ctx, userKey(id), u, 10*time.Minute)",
  "    return u, nil",
  "}",
];

const rect = (x: number, y: number, w: number, h: number): Rect => ({ x, y, w, h });
const point = (x: number, y: number): Point => ({ x, y });

let opNumber = 0;
function node(
  id: string,
  shape: Extract<DrawOp, { kind: "Node" }>["shape"],
  label: string,
  at: Rect,
): DrawOp {
  opNumber += 1;
  return { kind: "Node", id, op: opNumber, shape, label, rect: at, stack: 1 };
}

function edge(
  from: string,
  to: string,
  start: Point,
  control: Point | null,
  end: Point,
  label: string | null,
): DrawOp {
  opNumber += 1;
  return {
    kind: "Edge",
    id: `${from}>${to}`,
    op: opNumber,
    from,
    to,
    path: { start, control, end },
    label,
    style: "Solid",
  };
}

/** Points along the "hit" connector, for the trace dot. */
function hitPath(): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= 12; index += 1) {
    const u = index / 12;
    const a = (1 - u) * (1 - u);
    const b = 2 * (1 - u) * u;
    const c = u * u;
    points.push(
      point(Math.round(a * 176 + b * 288 + c * 404), Math.round(a * 158 + b * 96 + c * 96)),
    );
  }
  return points;
}

interface Timed {
  readonly at: number;
  readonly event: UiEvent;
}

/** Ops on one layer with their start times; each budget runs to the next start. */
function opsOn(layer: number, ops: readonly (readonly [number, DrawOp, number?])[]): Timed[] {
  return ops.map(([at, op, budget], index) => {
    const next = ops[index + 1];
    const until = budget ?? (next === undefined ? BUDGET_MAX_MS : next[0] - at);
    return {
      at,
      event: {
        type: "OpStart",
        layer,
        op,
        budget_ms: Math.min(BUDGET_MAX_MS, Math.max(BUDGET_MIN_MS, until)),
      },
    };
  });
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

/** The orb's state with the voice level: up while someone speaks, down in the pauses. */
function orb(): Timed[] {
  const states: [number, Extract<UiEvent, { type: "Orb" }>["state"]][] = [
    [0, "Idle"],
    [AT.listen, "Listening"],
    [AT.think, "Thinking"],
    [AT.speak, "Speaking"],
    [AT.condense, "Drawing"],
    [AT.interrupt, "Listening"],
    [21990, "Drawing"],
    [28277, "Speaking"],
    [33278, "Listening"],
    [35927, "Speaking"],
    [37709, "Drawing"],
    [AT.home, "Idle"],
  ];
  const stateAt = (at: number): Extract<UiEvent, { type: "Orb" }>["state"] => {
    let state: Extract<UiEvent, { type: "Orb" }>["state"] = "Idle";
    for (const [from, value] of states) {
      if (from <= at) {
        state = value;
      }
    }
    return state;
  };
  const voiced = (at: number): boolean =>
    SPEECH.some((utterance) => utterance.startAt <= at && at < utterance.endAt);
  const changes = new Set<number>(states.map(([at]) => at));
  for (const utterance of SPEECH) {
    changes.add(utterance.startAt);
    changes.add(utterance.endAt);
  }
  return [...changes]
    .sort((a, b) => a - b)
    .map((at) => ({
      at,
      event: { type: "Orb", state: stateAt(at), level_percent: voiced(at) ? 70 : 0 },
    }));
}

function thread(at: number, current: number | null, detours: string[]): Timed {
  return { at, event: { type: "Thread", steps: STEPS, current, detours } };
}

function mainBoard(): Timed[] {
  opNumber = 0;
  return opsOn(0, [
    [7271, node("client", "Actor", "Client", rect(119, 384, 42, 86))],
    [8992, edge("client", "cdn", point(170, 430), null, point(246, 430), null)],
    [9354, node("cdn", "Service", "CDN", rect(260, 392, 132, 76))],
    [10505, edge("cdn", "lb", point(404, 430), null, point(470, 430), null)],
    [11250, node("lb", "Service", "Load balancer", rect(482, 392, 178, 76))],
    [12977, edge("lb", "api", point(672, 430), null, point(736, 430), null)],
    [13817, node("api", "Group", "API pods ×3", rect(750, 304, 164, 252))],
    [15310, node("api-1", "Service", "", rect(770, 322, 124, 54))],
    [16208, node("api-2", "Service", "", rect(770, 403, 124, 54))],
    [17101, node("api-3", "Service", "", rect(770, 484, 124, 54))],
    [17997, edge("api", "redis", point(926, 404), point(970, 352), point(1010, 350), null)],
    // Starts so the interruption lands partway round the box: the stroke the user stops.
    [19549, node("redis", "Service", "Redis", rect(1022, 316, 144, 68)), BUDGET_MAX_MS],
    [39386, edge("api", "postgres", point(926, 458), point(970, 514), point(1010, 516), null)],
    [40250, node("postgres", "Db", "Postgres", rect(1022, 469, 144, 104))],
  ]);
}

function detourBoard(): Timed[] {
  return opsOn(1, [
    [21019, node("api", "Service", "API", rect(44, 140, 120, 60))],
    [21746, edge("api", "redis", point(176, 158), point(288, 96), point(404, 96), "hit")],
    [22177, node("redis", "Service", "Redis", rect(420, 66, 136, 60))],
    [22904, edge("api", "postgres", point(176, 184), point(288, 238), point(404, 236), "miss")],
    [23335, node("postgres", "Db", "Postgres", rect(420, 194, 136, 80))],
    [24510, edge("postgres", "redis", point(528, 190), null, point(528, 138), "write back"), 464],
  ]);
}

function timeline(): Timed[] {
  const timed: Timed[] = [
    { at: 0, event: { type: "Camera", layer: 0, frame: rect(0, 0, 1440, 900), focus: null } },
    ...orb(),
    ...SPEECH.flatMap(captions),
    thread(AT.thread, null, []),
    thread(AT.draw, 0, []),
    thread(AT.step1, 1, []),
    thread(AT.step2, 2, []),
    thread(AT.step3, 3, []),
    ...mainBoard(),
    { at: AT.interrupt, event: { type: "Freeze", layer: 0 } },
    { at: AT.detour, event: { type: "LayerPush", layer: 1, title: "cache-aside" } },
    thread(AT.branch, 3, ["cache-aside"]),
    ...detourBoard(),
    {
      at: AT.code,
      event: {
        type: "SurfaceShow",
        surface: {
          kind: "Code",
          id: 1,
          path: "internal/user/service.go",
          first_line: 42,
          lines: CODE_LINES,
          highlight: null,
        },
      },
    },
    {
      at: AT.line43,
      event: { type: "SurfaceUpdate", id: 1, update: { kind: "Highlight", line: 43 } },
    },
    {
      at: AT.line44,
      event: { type: "SurfaceUpdate", id: 1, update: { kind: "Highlight", line: 44 } },
    },
    {
      at: AT.trace,
      event: {
        type: "OpStart",
        layer: 1,
        op: { kind: "Trace", id: "hit-trace", op: opNumber + 1, points: hitPath() },
        budget_ms: 1300,
      },
    },
    {
      at: AT.note,
      event: { type: "Chip", kind: "Note", text: "Cache-aside: Redis first, Postgres on miss" },
    },
    { at: AT.task, event: { type: "Chip", kind: "Task", text: "Check our Redis TTLs" } },
    { at: AT.resume, event: { type: "LayerPop", layer: 1 } },
    { at: AT.resume, event: { type: "SurfaceDismiss", id: 1 } },
    thread(AT.resume, 3, []),
    { at: AT.resumeDraw, event: { type: "Resume", layer: 0, budget_ms: 39386 - AT.resumeDraw } },
    thread(AT.step4, 4, []),
  ];
  // Stable by time: events at the same moment keep the order above (state before drawing).
  return timed
    .map((entry, order) => ({ entry, order }))
    .sort((a, b) => a.entry.at - b.entry.at || a.order - b.order)
    .map(({ entry }) => entry);
}

export function demoLines(): UiLogLine[] {
  return timeline().map(({ at, event }) => ({ v: 1, t: at, k: "ui", event }));
}

export function demoJsonl(): string {
  return `${demoLines()
    .map((line) => JSON.stringify(line))
    .join("\n")}\n`;
}

/** Frames 1–7 of the design, as session ms: where the canvas freezes its key frames. */
export const DESIGN_FRAMES_MS = [1900, 4700, 19059, 25024, 33162, 36902, 41032] as const;
