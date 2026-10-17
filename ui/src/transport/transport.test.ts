import assert from "node:assert/strict";
import { test } from "node:test";
import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { FakeTransport } from "./fake.ts";
import { chaptersOf, parseReplayLog, REPLAY_TAIL_MS } from "./log.ts";
import { ReplayTransport } from "./replay.ts";
import { type TauriBridge, TauriTransport } from "./tauri.ts";
import { type BatchSink, isSessionId, SessionClock } from "./transport.ts";

const orb = (state: "Idle" | "Listening" | "Drawing"): UiEvent => ({
  type: "Orb",
  state,
  level_percent: 0,
});
const line = (t: number, event: unknown, extra: Record<string, unknown> = {}): string =>
  JSON.stringify({ v: 1, t, k: "ui", event, ...extra });

class RecordingSink implements BatchSink {
  readonly batches: UiBatch[] = [];
  resets = 0;
  batch(batch: UiBatch): void {
    this.batches.push(batch);
  }
  reset(): void {
    this.resets += 1;
    this.batches.length = 0;
  }
}

test("a replay log groups events by time and skips bad lines without crashing", () => {
  const text = [
    line(0, orb("Idle")),
    "",
    "{not json",
    line(100, { type: "Teleport", where: "moon" }),
    line(100, orb("Listening")),
    JSON.stringify({ v: 1, t: 120, k: "input", input: { VoiceStart: null } }),
    JSON.stringify({ v: 2, t: 150, k: "ui", event: orb("Idle") }),
    line(-5, orb("Idle")),
    line(100, { type: "Caption", who: "User", text: "hi", is_final: false }),
    line(200, { type: "Orb", state: "Dancing", level_percent: 0 }),
    line(200, { type: "Orb", state: "Idle", level_percent: 101 }),
    line(300, { type: "Acknowledge" }),
    "[1,2,3]",
  ].join("\n");
  const log = parseReplayLog(text);
  assert.deepEqual(
    log.batches.map((batch) => [batch.at, batch.events.map((event) => event.type)]),
    [
      [0, ["Orb"]],
      [100, ["Orb", "Caption"]],
      [300, ["Acknowledge"]],
    ],
  );
  assert.deepEqual(log.skipped, { malformed: 7, otherKind: 1, overLimit: 0, rows: 0 });
  assert.equal(log.durationMs, 300 + REPLAY_TAIL_MS);
});

test("an out-of-order log is sorted by time, keeping file order for ties", () => {
  const log = parseReplayLog(
    [line(50, orb("Drawing")), line(10, orb("Idle")), line(50, { type: "Acknowledge" })].join("\n"),
  );
  assert.deepEqual(
    log.batches.map((batch) => [batch.at, batch.events.map((event) => event.type)]),
    [
      [10, ["Orb"]],
      [50, ["Orb", "Acknowledge"]],
    ],
  );
});

test("an empty or garbage file is an empty replay, not an error", () => {
  assert.deepEqual(parseReplayLog("").batches, []);
  const garbage = parseReplayLog("\u0000\u0001\n\n}{");
  assert.equal(garbage.batches.length, 0);
  assert.equal(garbage.skipped.malformed, 2);
});

test("chapters follow what the session is doing", () => {
  const batches: UiBatch[] = [
    { at: 2000, events: [orb("Listening")] },
    {
      at: 7000,
      events: [
        {
          type: "OpStart",
          layer: 0,
          budget_ms: 500,
          op: { kind: "Trace", id: "t", op: 1, points: [] },
        },
      ],
    },
    { at: 20_000, events: [{ type: "LayerPush", layer: 1, title: "cache-aside", question: null }] },
    { at: 33_000, events: [{ type: "Caption", who: "User", text: "Note", is_final: false }] },
    { at: 34_000, events: [{ type: "Caption", who: "User", text: "Note that", is_final: true }] },
    { at: 36_000, events: [{ type: "Chip", kind: "Note", text: "x", source: null }] },
    { at: 36_100, events: [{ type: "Chip", kind: "Task", text: "y", source: null }] },
    { at: 37_000, events: [{ type: "LayerPop", layer: 1 }] },
  ];
  assert.deepEqual(
    chaptersOf(batches).map((chapter) => `${chapter.at}:${chapter.kind}`),
    [
      "0:Idle",
      "2000:Listening",
      "7000:Explaining",
      "20000:Detour",
      "33000:Capture",
      "37000:Resume",
    ],
  );
});

function demoReplay(): ReplayTransport {
  return new ReplayTransport(
    parseReplayLog(
      [line(0, orb("Idle")), line(1000, orb("Listening")), line(3000, orb("Drawing"))].join("\n"),
    ),
  );
}

test("replay delivers batches as the playhead passes them, at the chosen speed", async () => {
  const replay = demoReplay();
  const sink = new RecordingSink();
  await replay.connect(sink);
  assert.equal(replay.sessionTime(0), 0);
  assert.equal(sink.batches.length, 1);
  replay.play(0);
  assert.equal(replay.sessionTime(999), 999);
  assert.equal(sink.batches.length, 1);
  replay.sessionTime(1000);
  assert.equal(sink.batches.length, 2);
  replay.setSpeed(2, 1000);
  assert.equal(replay.sessionTime(2000), 3000);
  assert.equal(sink.batches.length, 3);
});

test("pause freezes the playhead and seeking back replays from a reset", async () => {
  const replay = demoReplay();
  const sink = new RecordingSink();
  await replay.connect(sink);
  replay.play(0);
  replay.sessionTime(3500);
  replay.pause(3500);
  assert.equal(replay.sessionTime(9000), 3500);
  replay.seek(1500, 9000);
  assert.equal(sink.resets, 1);
  assert.deepEqual(
    sink.batches.map((batch) => batch.at),
    [0, 1000],
  );
  replay.seek(3200, 9000);
  assert.equal(sink.resets, 1, "seeking forward never resets");
  assert.deepEqual(
    sink.batches.map((batch) => batch.at),
    [0, 1000, 3000],
  );
});

test("replay stops at the end and plays again from the start", async () => {
  const replay = demoReplay();
  const sink = new RecordingSink();
  await replay.connect(sink);
  let changes = 0;
  replay.subscribe(() => {
    changes += 1;
  });
  replay.play(0);
  const end = replay.log.durationMs;
  assert.equal(replay.sessionTime(end + 5000), end);
  assert.equal(replay.state.playing, false);
  replay.play(end + 6000);
  assert.equal(replay.sessionTime(end + 6000), 0);
  assert.equal(sink.resets, 1);
  assert.ok(changes >= 3);
});

test("a replay that plays to its end is finished; seeking or playing again is not", async () => {
  const replay = demoReplay();
  await replay.connect(new RecordingSink());
  const end = replay.log.durationMs;
  replay.seek(end, 0);
  replay.sessionTime(10);
  assert.equal(replay.finished, false, "a seek to the end did not play there");
  replay.play(100);
  assert.equal(replay.finished, false);
  replay.sessionTime(100 + end + 50);
  assert.equal(replay.finished, true);
  replay.seek(end / 2, 200 + end);
  assert.equal(replay.finished, false);
  replay.play(300 + end);
  replay.sessionTime(300 + 2 * end);
  assert.equal(replay.finished, true);
  replay.play(400 + 2 * end);
  assert.equal(replay.finished, false, "playing again starts over");
});

test("the session clock follows batch stamps and never runs backwards", () => {
  const clock = new SessionClock();
  assert.equal(clock.now(500), 0);
  clock.observe(1000, 10_000);
  assert.equal(clock.now(10_250), 1250);
  // A late batch (stamped before UI time) does not pull the clock back.
  clock.observe(1100, 10_400);
  assert.equal(clock.now(10_400), 1400);
  // An early batch (the UI fell behind) snaps it forward.
  clock.observe(2000, 10_500);
  assert.equal(clock.now(10_500), 2000);
});

test("the fake transport records commands and delivers pushed batches", async () => {
  const fake = new FakeTransport();
  const sink = new RecordingSink();
  assert.throws(() => fake.push({ at: 0, events: [] }));
  await fake.connect(sink);
  fake.push({ at: 40, events: [orb("Idle")] });
  fake.send({ type: "Pointer", target: null });
  assert.equal(sink.batches.length, 1);
  assert.equal(fake.sessionTime(0), 40);
  assert.deepEqual(fake.sent, [{ type: "Pointer", target: null }]);
});

test("the Tauri transport connects once with a channel and counts malformed batches", async () => {
  const calls: [string, Record<string, unknown>][] = [];
  let deliver: (message: unknown) => void = () => undefined;
  const channel = { id: "events" };
  const bridge: TauriBridge = {
    invoke: async (command, args) => {
      calls.push([command, args]);
      if (args.command !== undefined && (args.command as { type: string }).type === "Mic") {
        throw new Error("shell refused");
      }
    },
    channel: (onMessage) => {
      deliver = onMessage;
      return channel;
    },
  };
  let pageNow = 5000;
  const transport = new TauriTransport(bridge, () => pageNow);
  const sink = new RecordingSink();
  await transport.connect(sink);
  assert.deepEqual(calls, [["connect", { events: channel }]]);
  deliver({ at: 1200, events: [orb("Idle"), { type: "Nope" }] });
  deliver({ at: "soon", events: [] });
  deliver(null);
  assert.equal(sink.batches.length, 1);
  assert.equal(sink.batches[0]?.events.length, 1);
  assert.equal(transport.skippedEvents, 1);
  assert.equal(transport.malformedBatches, 2);
  pageNow = 5100;
  assert.equal(transport.sessionTime(pageNow), 1300);
  transport.send({ type: "Pointer", target: { kind: "Element", layer: 0, id: "lb" } });
  transport.send({ type: "Mic", action: "Mute" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls[1], [
    "command",
    { command: { type: "Pointer", target: { kind: "Element", layer: 0, id: "lb" } } },
  ]);
  assert.equal(transport.failedCommands, 1);
});

function tauri(answer: (command: string, args: Record<string, unknown>) => Promise<unknown>) {
  const calls: [string, Record<string, unknown>][] = [];
  let deliver: (message: unknown) => void = () => undefined;
  const bridge: TauriBridge = {
    invoke: (command, args) => {
      calls.push([command, args]);
      return answer(command, args);
    },
    channel: (onMessage) => {
      deliver = onMessage;
      return {};
    },
  };
  return { calls, bridge, deliver: (message: unknown) => deliver(message) };
}

test("a new session resets the stage and the clock, after the old session's events", async () => {
  const { bridge, deliver } = tauri(async () => undefined);
  let pageNow = 50_000;
  const transport = new TauriTransport(bridge, () => pageNow);
  const sink = new RecordingSink();
  await transport.connect(sink);
  deliver({ at: 1_800_000, events: [orb("Listening")] });
  assert.equal(transport.sessionTime(pageNow), 1_800_000);
  pageNow = 51_000;
  deliver({
    at: 40,
    events: [{ type: "ViewDismiss" }, { type: "SessionStart", session: "s2" }, orb("Idle")],
  });
  // The old session's last event went to the old stage, which was then reset.
  assert.equal(sink.resets, 1);
  assert.deepEqual(
    sink.batches.map((batch) => batch.events.map((event) => event.type)),
    [["SessionStart", "Orb"]],
  );
  assert.equal(transport.sessionsStarted, 1);
  // Session time restarted: the new session's stamps set the clock, though they are smaller.
  assert.equal(transport.sessionTime(51_000), 40);
  pageNow = 51_100;
  assert.equal(transport.sessionTime(pageNow), 140);
});

test("a batch without a new session never resets the stage", async () => {
  const { bridge, deliver } = tauri(async () => undefined);
  const transport = new TauriTransport(bridge, () => 0);
  const sink = new RecordingSink();
  await transport.connect(sink);
  deliver({ at: 10, events: [orb("Idle"), { type: "ViewDismiss" }] });
  assert.equal(sink.resets, 0);
  assert.equal(sink.batches.length, 1);
});

test("session logs come from the shell by id; failures are counted, never thrown", async () => {
  const { calls, bridge } = tauri(async (command, args) => {
    if (command !== "session_log") {
      return undefined;
    }
    if (args.id === "2026-10-12-160200") {
      return "{}\n";
    }
    if (args.id === "2026-10-12-160201") {
      return 42;
    }
    throw new Error("no such session");
  });
  const transport = new TauriTransport(bridge, () => 0);
  assert.equal(await transport.sessionLog("2026-10-12-160200"), "{}\n");
  assert.equal(await transport.sessionLog("2026-10-12-160201"), null);
  assert.equal(await transport.sessionLog("2026-01-01-000000"), null);
  assert.equal(await transport.sessionLog("../../etc/passwd"), null);
  assert.equal(transport.failedSessionLogs, 3);
  assert.deepEqual(
    calls.map(([command, args]) => [command, args.id]),
    [
      ["session_log", "2026-10-12-160200"],
      ["session_log", "2026-10-12-160201"],
      ["session_log", "2026-01-01-000000"],
    ],
    "an id that isn't one never reaches the shell",
  );
});

test("replay and fake transports load session logs by id too", async () => {
  const asked: string[] = [];
  const replay = new ReplayTransport(parseReplayLog(""), async (id) => {
    asked.push(id);
    return id === "a-1" ? "log" : null;
  });
  assert.equal(await replay.sessionLog("a-1"), "log");
  assert.equal(await replay.sessionLog("b-2"), null);
  assert.equal(await replay.sessionLog("a/../b"), null);
  assert.deepEqual(asked, ["a-1", "b-2"]);
  const fake = new FakeTransport();
  fake.logs.set("s", "text");
  assert.equal(await fake.sessionLog("s"), "text");
  assert.equal(await fake.sessionLog("t"), null);
});

test("session ids are checked before they name a file or reach the shell", () => {
  for (const good of ["2026-10-15-143210", "2026-10-15-143210-2", "s1", "a.b_c-d"]) {
    assert.ok(isSessionId(good), good);
  }
  for (const bad of ["", "..", "a/b", "a\\b", "-x", ".hidden", "a..b", "x".repeat(97), "é"]) {
    assert.ok(!isSessionId(bad), bad);
  }
});

test("a reset clock takes the next stamp as it comes", () => {
  const clock = new SessionClock();
  clock.observe(90_000, 1000);
  clock.reset();
  assert.equal(clock.now(2000), 0);
  clock.observe(10, 2000);
  assert.equal(clock.now(2500), 510);
});
